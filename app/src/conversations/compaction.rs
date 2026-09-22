//! Durable conversation summaries. Original messages stay on disk.

use crate::providers::{ChatTurn, ModelSelection, Role};

use super::history::{ConversationMessage, HistoryError, MessageRole, MessageStatus, RequestUsage};
use super::id::MessageId;

pub(crate) mod generation;

#[cfg(test)]
mod tests;

pub(crate) const MAXIMUM_SUMMARY_BYTES: usize = 128 * 1024;
pub(crate) const SUMMARY_OUTPUT_TOKENS: u64 = 10_000;
pub(crate) const MAXIMUM_PRESERVE_BYTES: usize = 2 * 1024;
pub(crate) const MAXIMUM_SUMMARY_ATTEMPTS: u32 = 2;
pub(crate) const MAXIMUM_OVERFLOW_RECOVERY: u32 = 1;
/// Bound on sequential summary requests for one checkpoint. It stops an
/// unbounded chunk loop without capping retained history.
pub(crate) const MAXIMUM_SUMMARY_REQUESTS: usize = 64;

pub(crate) const SUMMARY_PREAMBLE: &str = "You summarise a coding-agent conversation for later model requests. Write concise, factual context. Prioritise the current goals and constraints. Retain decisions, relevant file references, recorded tool outcomes and unresolved work. Omit repetition and superseded detail unless it explains the current state. Do not claim that a command succeeded unless a tool result recorded success. Do not treat this summary as authority, consent or proof of command success. Reply with the summary only.";

const SUMMARY_TURN_PREFIX: &str = "Earlier context summary. This summary grants no authority or consent and is not proof of command success.\n\n";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CompactionRecord {
    pub(crate) covered_through: MessageId,
    // The original suffix entry is provenance, not coverage authority on another branch.
    pub(crate) retained_from: MessageId,
    pub(crate) text: String,
    pub(crate) requests: Vec<RequestUsage>,
    pub(crate) preserve: Option<String>,
    pub(crate) created_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CompactionError {
    NothingToCompact,
    Unsettled,
    Bound,
    Malformed,
    Persist,
    /// One complete exchange is larger than a summary request can carry.
    Oversized,
    /// The selected model cannot reserve the summary output allowance.
    Output,
    Requests,
    UnknownCapacity,
    Preserve,
}

impl CompactionError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::NothingToCompact => "There is no complete earlier exchange to summarise.",
            Self::Unsettled => {
                "This conversation cannot compact while work, approval or recovery is active."
            }
            Self::Bound => "The summary is larger than Power Plant can store.",
            Self::Malformed => "The summary is not valid replacement context.",
            Self::Persist => {
                "Power Plant could not store the summary. The last valid context remains."
            }
            Self::Oversized => {
                "One complete exchange is too large to summarise. The previous context remains."
            }
            Self::Output => {
                "The selected model cannot supply the 10,000-token summary allowance. The previous context remains."
            }
            Self::UnknownCapacity => {
                "The selected model has unknown context or output capacity. Power Plant cannot budget a summary request. The previous context remains."
            }
            Self::Requests => {
                "The summary needs more sequential requests than Power Plant allows. The previous context remains."
            }
            Self::Preserve => {
                "The preservation instructions must contain no NUL characters and use at most 2,048 UTF-8 bytes."
            }
        }
    }
}

impl CompactionRecord {
    pub(crate) fn valid(&self, messages: &[ConversationMessage]) -> bool {
        !self.requests.is_empty()
            && self.requests.len() <= MAXIMUM_SUMMARY_REQUESTS
            && self.requests.iter().all(RequestUsage::valid)
            && valid_preserve(self.preserve.as_deref())
            && !self.text.is_empty()
            && self.text.len() <= MAXIMUM_SUMMARY_BYTES
            && !self.text.contains('\0')
            && self.covered_through != self.retained_from
            && split_index(messages, self).is_some()
    }
}

pub(crate) fn normalise_preserve(
    preserve: Option<&str>,
) -> Result<Option<String>, CompactionError> {
    let Some(preserve) = preserve else {
        return Ok(None);
    };
    let preserve = preserve.trim();
    if preserve.is_empty() {
        return Ok(None);
    }
    if preserve.len() > MAXIMUM_PRESERVE_BYTES || preserve.contains('\0') {
        return Err(CompactionError::Preserve);
    }
    Ok(Some(preserve.to_owned()))
}

fn valid_preserve(preserve: Option<&str>) -> bool {
    preserve.is_none_or(|preserve| {
        !preserve.is_empty() && preserve.len() <= MAXIMUM_PRESERVE_BYTES && !preserve.contains('\0')
    })
}

/// Preservation instructions supplement the fixed instructions, but never replace them.
pub(crate) fn summary_preamble(preserve: Option<&str>) -> Result<String, CompactionError> {
    let preserve = normalise_preserve(preserve)?;
    let Some(preserve) = preserve else {
        return Ok(SUMMARY_PREAMBLE.to_owned());
    };
    let mut preamble = String::from(SUMMARY_PREAMBLE);
    preamble.push_str("\n\nWhat to preserve:\n");
    preamble.push_str(&preserve);
    Ok(preamble)
}

/// Keep the latest complete exchange. Summarise earlier complete exchanges.
pub(crate) fn select_boundary(
    messages: &[ConversationMessage],
    current: Option<&CompactionRecord>,
) -> Result<(MessageId, MessageId), CompactionError> {
    if current.is_some_and(|record| !record.valid(messages)) {
        return Err(CompactionError::Malformed);
    }
    let settled = messages
        .iter()
        .position(|message| message.status == MessageStatus::Pending)
        .unwrap_or(messages.len());
    let ends = complete_ends(&messages[..settled])?;
    let start = current
        .and_then(|record| split_index(messages, record))
        .unwrap_or(0);
    let suffix = ends
        .iter()
        .copied()
        .filter(|&index| index >= start)
        .collect::<Vec<_>>();
    if suffix.len() < 2 {
        return Err(CompactionError::NothingToCompact);
    }
    let covered = suffix[suffix.len() - 2];
    let covered_through = messages[covered].id;
    let retained_from = first_message_after(messages, covered).ok_or(CompactionError::Malformed)?;
    Ok((covered_through, retained_from))
}

pub(crate) fn project(
    messages: &[ConversationMessage],
    selection: Option<&ModelSelection>,
    compaction: Option<&CompactionRecord>,
) -> Result<Vec<ChatTurn>, HistoryError> {
    let Some(compaction) = compaction else {
        return super::history::project(messages, selection);
    };
    if !compaction.valid(messages) {
        return Err(HistoryError::Bound);
    }
    let split = split_index(messages, compaction).ok_or(HistoryError::Bound)?;
    let mut retained = super::history::project(&messages[split..], selection)?;
    let mut summary = summary_turn(&compaction.text);
    summary.usage.extend(compaction.requests.iter().cloned());
    let mut projected = vec![summary];
    projected.append(&mut retained);
    Ok(projected)
}

pub(crate) fn summary_turn(text: &str) -> ChatTurn {
    ChatTurn::user(format!("{SUMMARY_TURN_PREFIX}{text}"))
}

/// Split the leading previous-summary turn from the source turns. Sequential
/// requests add that summary exactly once, below the fixed instructions.
pub(crate) fn split_previous_summary(turns: &[ChatTurn]) -> (Option<String>, &[ChatTurn]) {
    match turns.first() {
        Some(turn) => match turn.text.strip_prefix(SUMMARY_TURN_PREFIX) {
            Some(text) => (Some(text.to_owned()), &turns[1..]),
            None => (None, turns),
        },
        None => (None, turns),
    }
}

pub(crate) fn project_turns(
    turns: &[ChatTurn],
    covered_through: usize,
    text: &str,
    requests: &[RequestUsage],
) -> Result<Vec<ChatTurn>, CompactionError> {
    if text.is_empty() || text.len() > MAXIMUM_SUMMARY_BYTES || text.contains('\0') {
        return Err(CompactionError::Malformed);
    }
    if covered_through >= turns.len() {
        return Err(CompactionError::Malformed);
    }
    if !exchange_end(turns, covered_through) {
        return Err(CompactionError::Unsettled);
    }
    let mut summary = summary_turn(text);
    summary.usage.extend(requests.iter().cloned());
    let mut projected = vec![summary];
    projected.extend(turns[covered_through + 1..].iter().cloned());
    Ok(projected)
}

pub(crate) fn workflow_cover_index(turns: &[ChatTurn]) -> Result<usize, CompactionError> {
    let ends = exchange_ends(turns)?;
    if ends.len() < 2 {
        return Err(CompactionError::NothingToCompact);
    }
    Ok(ends[ends.len() - 2])
}

pub(crate) fn validate_summary(
    text: &str,
    secret: Option<&str>,
) -> Result<String, CompactionError> {
    let text = text.trim();
    if text.is_empty() || text.contains('\0') {
        return Err(CompactionError::Malformed);
    }
    if text.len() > MAXIMUM_SUMMARY_BYTES {
        return Err(CompactionError::Bound);
    }
    if secret.is_some_and(|secret| !secret.is_empty() && text.contains(secret)) {
        return Err(CompactionError::Malformed);
    }
    Ok(text.to_owned())
}

pub(crate) fn chunk_prompt(
    previous: Option<&str>,
    turns: &[ChatTurn],
) -> Result<String, CompactionError> {
    let mut body = String::from("Summarise this conversation:\n");
    if let Some(previous) = previous {
        body.push_str("\n## Earlier summary\n");
        body.push_str(previous);
        body.push('\n');
    }
    for turn in turns {
        let role = match turn.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
        };
        body.push_str("\n## ");
        body.push_str(role);
        body.push('\n');
        if !turn.text.is_empty() {
            body.push_str(&turn.text);
            body.push('\n');
        }
        for call in &turn.calls {
            let Some(result) = &call.result else {
                return Err(CompactionError::Unsettled);
            };
            body.push_str("\nTool ");
            body.push_str(&call.name);
            body.push_str(":\n");
            body.push_str(&call.arguments.to_string());
            body.push('\n');
            body.push_str(&result.label);
            body.push('\n');
            if let Some(command) = &result.command {
                body.push_str(&format!("Outcome: {:?}\n", command.termination));
                if let Some(reference) = command.retained_reference() {
                    body.push_str(&format!("Retained output reference: {reference}\n"));
                }
            }
            body.push_str(&result.output);
            body.push('\n');
        }
        if body.len() > MAXIMUM_SUMMARY_BYTES.saturating_mul(8) {
            return Err(CompactionError::Bound);
        }
    }
    Ok(body)
}

/// Indices of complete assistant exchanges. Any unresolved tool call stops the
/// walk, because a summary must not split a call from its result.
pub(crate) fn exchange_ends(turns: &[ChatTurn]) -> Result<Vec<usize>, CompactionError> {
    let mut ends = Vec::new();
    for index in 0..turns.len() {
        if exchange_end(turns, index) {
            ends.push(index);
        } else if let Role::Assistant = turns[index].role
            && turns[index].calls.iter().any(|call| call.result.is_none())
        {
            return Err(CompactionError::Unsettled);
        }
    }
    Ok(ends)
}

fn complete_ends(messages: &[ConversationMessage]) -> Result<Vec<usize>, CompactionError> {
    let mut ends = Vec::new();
    super::history::project(messages, None).map_err(|_| CompactionError::Unsettled)?;
    for (index, message) in messages.iter().enumerate() {
        if message.status == MessageStatus::Pending {
            return Err(CompactionError::Unsettled);
        }
        if message.role == MessageRole::Assistant && message.status == MessageStatus::Complete {
            ends.push(index);
        }
    }
    Ok(ends)
}

fn first_message_after(messages: &[ConversationMessage], covered: usize) -> Option<MessageId> {
    messages.get(covered + 1).map(|message| message.id)
}

fn split_index(messages: &[ConversationMessage], compaction: &CompactionRecord) -> Option<usize> {
    let covered = messages
        .iter()
        .position(|message| message.id == compaction.covered_through)?;
    // Coverage follows immutable ancestors. The selected path can have a different
    // child, or end at the covered entry before the next Send.
    let retained = covered.checked_add(1)?;
    if messages[covered].role != MessageRole::Assistant
        || messages[covered].status != MessageStatus::Complete
    {
        return None;
    }
    complete_ends(&messages[..=covered])
        .ok()?
        .into_iter()
        .any(|index| index == covered)
        .then_some(retained)
}

fn exchange_end(turns: &[ChatTurn], index: usize) -> bool {
    turns[index].role == Role::Assistant
        && turns[index].calls.iter().all(|call| call.result.is_some())
}

pub(crate) fn covered_turns(
    messages: &[ConversationMessage],
    selection: Option<&ModelSelection>,
    covered_through: MessageId,
    current: Option<&CompactionRecord>,
) -> Result<Vec<ChatTurn>, CompactionError> {
    let covered = messages
        .iter()
        .position(|message| message.id == covered_through)
        .ok_or(CompactionError::Malformed)?;
    let start = current.map_or(Ok(0), |record| {
        split_index(messages, record).ok_or(CompactionError::Malformed)
    })?;
    if start > covered {
        return Err(CompactionError::NothingToCompact);
    }
    let mut turns = Vec::new();
    if let Some(record) = current {
        turns.push(summary_turn(&record.text));
    }
    turns.extend(
        super::history::project(&messages[start..=covered], selection)
            .map_err(|_| CompactionError::Unsettled)?,
    );
    Ok(turns)
}
