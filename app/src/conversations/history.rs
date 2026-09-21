//! Durable, provider-independent conversation history.
//!
//! The transcript and the model projection are separate projections of these
//! entries. A tool result always identifies the call that produced it, and
//! opaque provider continuation data stays separate from visible text.

use serde::{Deserialize, Serialize};

use crate::execution::BudgetSnapshot;
use crate::providers::{
    AssistantActivity, ChatToolCall, ChatTurn, CompletionReason, ModelSelection, ToolOutput,
};
use crate::sessions::JobId;
use crate::workflows::{AttemptId, RunId};

use super::id::{CheckpointId, MessageId};

pub(crate) const MAXIMUM_ACTIVITY_ITEMS: usize = 256;
pub(crate) const MAXIMUM_ACTIVITY_BYTES: usize = 256 * 1024;
pub(crate) const MAXIMUM_CALL_IDENTIFIER_BYTES: usize = 512;
const MAXIMUM_ARGUMENT_BYTES: usize = 64 * 1024;
pub(crate) const MAXIMUM_CONTINUATION_BLOCKS: usize = 64;
pub(crate) const MAXIMUM_CONTINUATION_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MessageRole {
    User,
    Assistant,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MessageStatus {
    Complete,
    Pending,
    Interrupted,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ConversationMessage {
    pub(crate) id: MessageId,
    pub(crate) role: MessageRole,
    pub(crate) text: String,
    pub(crate) activity: Vec<AssistantActivity>,
    pub(crate) continuation: Vec<ContinuationMetadata>,
    pub(crate) status: MessageStatus,
    pub(crate) error: Option<String>,
    pub(crate) request: Option<JobId>,
    pub(crate) completion: Option<CompletionReason>,
}

/// Opaque blocks require the original provider and model. A provider change
/// retains portable text and tool exchanges without these blocks.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct ContinuationMetadata {
    pub(crate) provider: crate::providers::ProviderKind,
    pub(crate) model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reasoning_id: Option<String>,
    pub(crate) blocks: Vec<ContinuationBlock>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum ContinuationBlock {
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    Encrypted {
        data: String,
    },
    Redacted {
        data: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContinuationCheckpoint {
    pub(crate) id: CheckpointId,
    pub(crate) boundary: MessageId,
    pub(crate) pinned: crate::execution::ExecutionSettings,
    pub(crate) budget: BudgetSnapshot,
    pub(crate) run: Option<RunId>,
    pub(crate) attempt: Option<AttemptId>,
    pub(crate) step: Option<String>,
    pub(crate) drafts: Vec<PausedOutputDraft>,
    pub(crate) created_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PausedOutputDraft {
    pub(crate) key: String,
    pub(crate) kind: String,
    pub(crate) markdown: String,
    pub(crate) verdict: Option<String>,
    pub(crate) outcome: Option<String>,
}

impl ContinuationCheckpoint {
    pub(crate) fn valid(&self, messages: &[ConversationMessage]) -> bool {
        self.budget.valid()
            && self.drafts.len() <= 16
            && self.drafts.iter().all(|draft| {
                !draft.key.is_empty()
                    && draft.key.len() <= 64
                    && !draft.kind.is_empty()
                    && draft.kind.len() <= 32
                    && draft.markdown.len() <= 64 * 1024
                    && !draft.key.contains('\0')
                    && !draft.markdown.contains('\0')
            })
            && self
                .step
                .as_ref()
                .is_none_or(|step| !step.is_empty() && step.len() <= 64 && !step.contains('\0'))
            && self.run.is_some() == self.attempt.is_some()
            && self.step.is_some() == self.run.is_some()
            && messages.iter().any(|message| {
                message.id == self.boundary
                    && message.role == MessageRole::Assistant
                    && message.status == MessageStatus::Complete
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HistoryError {
    MalformedCall,
    DuplicateIdentifier,
    OrphanResult,
    Bound,
    Continuation,
    Unsettled,
}

impl HistoryError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::MalformedCall => "Stored history contains a malformed tool call.",
            Self::DuplicateIdentifier => "Stored history contains a duplicate tool call.",
            Self::OrphanResult => "Stored history contains a tool result without its call.",
            Self::Bound => "Stored history exceeds the conversation limit.",
            Self::Continuation => "Stored model continuation data is unavailable or incompatible.",
            Self::Unsettled => {
                "A stored tool call has no trustworthy outcome. Resolve recovery before further work."
            }
        }
    }
}

impl std::fmt::Display for HistoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for HistoryError {}

/// Validate one assistant activity list against its visible response text.
pub(crate) fn valid_activity(activity: &[AssistantActivity], text: &str) -> bool {
    if activity.is_empty() {
        return true;
    }
    if activity.len() > MAXIMUM_ACTIVITY_ITEMS {
        return false;
    }
    let mut bytes = 0usize;
    let mut response = String::new();
    let mut identifiers: Vec<&str> = Vec::new();
    for item in activity {
        let parts: Vec<&str> = match item {
            AssistantActivity::Response(value) => {
                response.push_str(value);
                vec![value]
            }
            AssistantActivity::Thinking(value) => vec![value],
            AssistantActivity::Tool(tool) => {
                if !valid_command(tool.command.as_ref()) {
                    return false;
                }
                let mut parts = vec![tool.label.as_str(), tool.output.as_str()];
                if let Some(command) = &tool.command {
                    parts.extend(command.chunks.iter().map(|chunk| chunk.text.as_str()));
                }
                parts
            }
            AssistantActivity::ToolCall {
                id,
                name,
                arguments,
                result,
            } => {
                if id.is_empty()
                    || name.is_empty()
                    || id.len() > MAXIMUM_CALL_IDENTIFIER_BYTES
                    || name.len() > MAXIMUM_CALL_IDENTIFIER_BYTES
                    || !arguments.is_object()
                    || serialised_argument_bytes(arguments) > MAXIMUM_ARGUMENT_BYTES
                {
                    return false;
                }
                if identifiers.contains(&id.as_str()) {
                    return false;
                }
                identifiers.push(id);
                if let Some(command) = result.as_ref().and_then(|tool| tool.command.as_ref())
                    && !valid_command(Some(command))
                {
                    return false;
                }
                bytes = bytes.saturating_add(serialised_argument_bytes(arguments));
                let mut parts = vec![id.as_str(), name.as_str()];
                if let Some(tool) = result {
                    parts.extend([tool.label.as_str(), tool.output.as_str()]);
                    if let Some(command) = &tool.command {
                        parts.extend(command.chunks.iter().map(|chunk| chunk.text.as_str()));
                    }
                }
                parts
            }
        };
        for part in parts {
            bytes = bytes.saturating_add(part.len());
            if part.contains('\0') || bytes > MAXIMUM_ACTIVITY_BYTES {
                return false;
            }
        }
    }
    response == text
}

pub(crate) fn valid_command(command: Option<&crate::execution::CommandResult>) -> bool {
    command.is_none_or(crate::execution::CommandResult::is_bounded)
}

pub(crate) fn valid_message_error(status: MessageStatus, error: Option<&str>) -> bool {
    error.is_none_or(|text| {
        status == MessageStatus::Failed
            && !text.trim().is_empty()
            && text.len() <= crate::providers::MAXIMUM_PROVIDER_DETAIL_BYTES
            && !text.chars().any(char::is_control)
    })
}

pub(crate) fn contains_credential(value: &impl Serialize, secret: Option<&str>) -> bool {
    secret.is_some_and(|secret| {
        if secret.is_empty() {
            return false;
        }
        let Ok(encoded_secret) = serde_json::to_string(secret) else {
            return true;
        };
        serde_json::to_string(value).map_or(true, |encoded| {
            encoded.contains(&encoded_secret[1..encoded_secret.len() - 1])
        })
    })
}

pub(crate) fn valid_continuation(metadata: &[ContinuationMetadata]) -> bool {
    if metadata.len() > 8 {
        return false;
    }
    let mut total = 0usize;
    for item in metadata {
        if item.model.is_empty()
            || item.model.len() > crate::providers::MAXIMUM_MODEL_BYTES
            || item.model.chars().any(char::is_control)
            || item.blocks.len() > MAXIMUM_CONTINUATION_BLOCKS
            || item.reasoning_id.as_ref().is_some_and(|id| {
                id.is_empty()
                    || id.len() > MAXIMUM_CALL_IDENTIFIER_BYTES
                    || id.chars().any(char::is_control)
            })
        {
            return false;
        }
        for block in &item.blocks {
            let size = match block {
                ContinuationBlock::Text { text, signature } => text
                    .len()
                    .saturating_add(signature.as_ref().map_or(0, String::len)),
                ContinuationBlock::Encrypted { data } | ContinuationBlock::Redacted { data } => {
                    data.len()
                }
            };
            total = total.saturating_add(size);
            if total > MAXIMUM_CONTINUATION_BYTES {
                return false;
            }
        }
    }
    true
}

/// Build the provider request projection. Incompatible opaque continuation
/// fields are excluded rather than replayed across a provider change.
pub(crate) fn project(
    messages: &[ConversationMessage],
    selection: Option<&ModelSelection>,
) -> Result<Vec<ChatTurn>, HistoryError> {
    validate_exchange(messages)?;
    let mut history = Vec::new();
    for message in messages {
        match message.role {
            MessageRole::User => {
                if message.text.contains('\0') {
                    return Err(HistoryError::Bound);
                }
                history.push(ChatTurn::user(message.text.clone()));
            }
            MessageRole::Assistant => {
                if message.status == MessageStatus::Pending {
                    continue;
                }
                if !valid_activity(&message.activity, &message.text) {
                    return Err(HistoryError::Bound);
                }
                if !valid_continuation(&message.continuation) {
                    return Err(HistoryError::Continuation);
                }
                let (mut text, calls, tools) = assistant_projection(message)?;
                if calls.iter().any(|call| {
                    call.result.as_ref().is_none_or(|result| {
                        result.command.as_ref().is_some_and(|command| {
                            matches!(
                                command.termination,
                                crate::execution::CommandTermination::Unknown
                            )
                        })
                    })
                }) {
                    return Err(HistoryError::Unsettled);
                }
                if message.status != MessageStatus::Complete {
                    text.clear();
                    if calls.is_empty() {
                        continue;
                    }
                }
                let continuation = continuation_for(&message.continuation, selection)?;
                history.push(ChatTurn {
                    role: crate::providers::Role::Assistant,
                    text,
                    thinking: String::new(),
                    tools,
                    activity: message.activity.clone(),
                    usage: None,
                    calls,
                    continuation,
                });
            }
        }
    }
    Ok(history)
}

fn assistant_projection(
    message: &ConversationMessage,
) -> Result<(String, Vec<ChatToolCall>, Vec<ToolOutput>), HistoryError> {
    let mut text = if message.activity.is_empty() {
        message.text.clone()
    } else {
        message
            .activity
            .iter()
            .filter_map(|activity| match activity {
                AssistantActivity::Response(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    if text.is_empty() && !message.text.is_empty() && !message.activity.is_empty() {
        text = message.text.clone();
    }
    let mut calls = Vec::new();
    let mut tools = Vec::new();
    let mut identifiers: Vec<&str> = Vec::new();
    for activity in &message.activity {
        match activity {
            AssistantActivity::ToolCall {
                id,
                name,
                arguments,
                result,
            } => {
                if id.is_empty() || name.is_empty() {
                    return Err(HistoryError::MalformedCall);
                }
                if identifiers.contains(&id.as_str()) {
                    return Err(HistoryError::DuplicateIdentifier);
                }
                identifiers.push(id);
                calls.push(ChatToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                    result: result.clone(),
                });
                if let Some(result) = result {
                    tools.push(result.clone());
                }
            }
            AssistantActivity::Tool(tool) => tools.push(tool.clone()),
            _ => {}
        }
    }
    Ok((text, calls, tools))
}

fn continuation_for(
    metadata: &[ContinuationMetadata],
    selection: Option<&ModelSelection>,
) -> Result<Vec<ContinuationMetadata>, HistoryError> {
    let Some(selection) = selection else {
        return Ok(Vec::new());
    };
    let mut kept = Vec::new();
    for item in metadata {
        if item.provider != selection.provider {
            // A provider change keeps portable text and tool exchanges only.
            continue;
        }
        if item.model != selection.model || !valid_continuation(std::slice::from_ref(item)) {
            return Err(HistoryError::Continuation);
        }
        kept.push(item.clone());
    }
    Ok(kept)
}

fn serialised_argument_bytes(arguments: &serde_json::Value) -> usize {
    serde_json::to_vec(arguments)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX)
}

/// Reject a result that has no originating call and duplicate call identifiers
/// across the whole retained exchange.
pub(crate) fn validate_exchange(messages: &[ConversationMessage]) -> Result<(), HistoryError> {
    let mut known: Vec<&str> = Vec::new();
    for message in messages {
        for activity in &message.activity {
            match activity {
                AssistantActivity::ToolCall { id, .. } => {
                    if id.is_empty() {
                        return Err(HistoryError::MalformedCall);
                    }
                    if known.contains(&id.as_str()) {
                        return Err(HistoryError::DuplicateIdentifier);
                    }
                    known.push(id);
                }
                AssistantActivity::Tool(_) => return Err(HistoryError::OrphanResult),
                _ => {}
            }
        }
    }
    Ok(())
}

pub(crate) fn settle_interrupted_questions(message: &mut ConversationMessage) {
    for activity in &mut message.activity {
        if let AssistantActivity::ToolCall { name, result, .. } = activity
            && name == crate::conversations::questions::ASK_USER
            && result.is_none()
        {
            *result = Some(ToolOutput {
                label: crate::conversations::questions::ASK_USER.to_owned(),
                output: crate::conversations::questions::INTERRUPTED_RESULT.to_owned(),
                command: None,
            });
        }
    }
}

#[cfg(test)]
mod tests;
