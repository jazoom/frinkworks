//! Bounded, session-bound fork drafts.
//!
//! A fork copies durable history up to one settled boundary. It never copies a
//! queue, a pending question, an execution checkpoint or runtime consent. The
//! destination conversation exists only after the user sends the draft.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::execution::{OutputKey, OutputScope, OutputStore};
use crate::providers::{AssistantActivity, ToolOutput};
use crate::sessions::{JobId, SessionId};

use super::history::{ConversationMessage, MessageRole, MessageStatus};
use super::id::{ConversationId, MessageId};
use super::store::{ConversationModelConfiguration, ConversationRecord};

pub(crate) const MAXIMUM_FORK_DRAFTS: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ForkError {
    Random,
    Missing,
    Boundary,
    Uncertain,
    Bound,
}

impl ForkError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Random => "Power Plant could not create a fork draft. Try again.",
            Self::Missing => "That message is no longer in the conversation.",
            Self::Boundary => {
                "Fork only between complete exchanges. Pick an earlier settled message."
            }
            Self::Uncertain => {
                "That part of the conversation has an uncertain command outcome. Fork before it."
            }
            Self::Bound => "That fork has no settled history to copy.",
        }
    }
}

impl std::fmt::Display for ForkError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for ForkError {}

/// Durable provenance for a forked conversation. It names the source boundary
/// without holding a mutable alias to source history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ForkProvenance {
    pub(crate) source: ConversationId,
    pub(crate) source_revision: u32,
    pub(crate) boundary: MessageId,
    /// The source was a candidate review. The destination keeps immutable
    /// evidence restrictions and receives no filesystem authority.
    pub(crate) candidate_review: bool,
}

#[derive(Clone)]
pub(crate) struct ForkSnapshot {
    pub(crate) source: ConversationId,
    pub(crate) source_revision: u32,
    pub(crate) boundary: MessageId,
    pub(crate) source_title: String,
    pub(crate) messages: Vec<ConversationMessage>,
    pub(crate) model: Option<ConversationModelConfiguration>,
    /// The source was a candidate review, so the fork keeps no filesystem
    /// authority and no candidate ownership.
    pub(crate) candidate_review: bool,
    pub(crate) review_context: Option<super::store::CandidateReviewContext>,
}

pub(crate) struct ForkClaim<'a> {
    drafts: &'a ForkDrafts,
    key: (SessionId, String),
    snapshot: Option<ForkSnapshot>,
}

impl ForkClaim<'_> {
    pub(crate) fn commit(mut self) {
        self.snapshot = None;
    }
}

impl Drop for ForkClaim<'_> {
    fn drop(&mut self) {
        let Some(snapshot) = self.snapshot.take() else {
            return;
        };
        let mut drafts = self
            .drafts
            .drafts
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // A failed send must not replace a newer draft from the same session.
        if drafts.len() < MAXIMUM_FORK_DRAFTS
            && !drafts.keys().any(|(session, _)| *session == self.key.0)
        {
            drafts.insert(self.key.clone(), snapshot);
        }
    }
}

#[derive(Default)]
pub(crate) struct ForkDrafts {
    drafts: Mutex<HashMap<(SessionId, String), ForkSnapshot>>,
}

impl ForkDrafts {
    pub(crate) fn insert(
        &self,
        session: SessionId,
        nonce: String,
        draft: ForkSnapshot,
        sessions: &crate::sessions::SessionStore,
    ) -> Result<(), &'static str> {
        let mut drafts = self
            .drafts
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        drafts.retain(|(owner, _), _| *owner != session && sessions.contains_live(owner));
        if drafts.len() >= MAXIMUM_FORK_DRAFTS {
            return Err(
                "Too many fork drafts are open. Close a browser session before another fork.",
            );
        }
        drafts.insert((session, nonce), draft);
        Ok(())
    }

    pub(crate) fn get(&self, session: SessionId, nonce: &str) -> Option<ForkSnapshot> {
        self.drafts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&(session, nonce.to_owned()))
            .cloned()
    }

    pub(crate) fn claim(&self, session: SessionId, nonce: &str) -> Option<ForkClaim<'_>> {
        let key = (session, nonce.to_owned());
        let snapshot = self
            .drafts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&key)?;
        Some(ForkClaim {
            drafts: self,
            key,
            snapshot: Some(snapshot),
        })
    }
}

/// Build a bounded snapshot from a durable boundary on the selected active
/// path. A boundary on an abandoned branch is unavailable. Only a complete
/// prefix with settled tool calls is accepted.
pub(crate) fn snapshot(
    record: &ConversationRecord,
    boundary: MessageId,
) -> Result<ForkSnapshot, ForkError> {
    let Some(index) = record
        .messages
        .iter()
        .position(|message| message.id == boundary)
    else {
        return Err(ForkError::Missing);
    };
    let prefix = &record.messages[..=index];
    validate_prefix(prefix)?;
    let candidate_review = record.source_candidate_review.is_some()
        || record.candidate_review_context.is_some()
        || record
            .forked_from
            .as_ref()
            .is_some_and(|source| source.candidate_review);
    let model = record.model.as_ref().map(|configuration| {
        if candidate_review {
            review_only(configuration)
        } else {
            configuration.clone()
        }
    });
    Ok(ForkSnapshot {
        source: record.id,
        source_revision: record.revision,
        boundary,
        source_title: record.title.clone(),
        messages: prefix.to_vec(),
        model,
        candidate_review,
        review_context: record.candidate_review_context.clone(),
    })
}

/// True when the message at `index` is a safe fork boundary.
pub(crate) fn forkable(messages: &[ConversationMessage], index: usize) -> bool {
    messages
        .get(index)
        .is_some_and(|message| message.status == MessageStatus::Complete)
        && validate_prefix(&messages[..=index]).is_ok()
}

fn validate_prefix(prefix: &[ConversationMessage]) -> Result<(), ForkError> {
    if prefix.is_empty() {
        return Err(ForkError::Bound);
    }
    // A complete exchange ends with a settled assistant response. A trailing
    // user message would split the exchange and duplicate a question.
    if prefix
        .last()
        .is_none_or(|message| message.role != MessageRole::Assistant)
    {
        return Err(ForkError::Boundary);
    }
    for message in prefix {
        if message.status != MessageStatus::Complete {
            return Err(ForkError::Boundary);
        }
        for activity in &message.activity {
            match activity {
                AssistantActivity::ToolCall { result: None, .. } => {
                    return Err(ForkError::Uncertain);
                }
                AssistantActivity::ToolCall {
                    result: Some(tool), ..
                } if tool.command.as_ref().is_some_and(|command| {
                    command.termination == crate::execution::CommandTermination::Unknown
                }) =>
                {
                    return Err(ForkError::Uncertain);
                }
                _ => {}
            }
        }
    }
    super::history::validate_exchange(prefix).map_err(|_| ForkError::Boundary)?;
    Ok(())
}

/// Preserve source message identities for entry provenance within the copied prefix.
/// Retained output
/// references move to the destination scope; unreadable references degrade to
/// the bounded preview without a full-output link.
pub(crate) fn materialise(
    snapshot: &ForkSnapshot,
    destination: ConversationId,
    outputs: &OutputStore,
) -> Result<Vec<ConversationMessage>, ForkError> {
    let source_scope = OutputScope::conversation(snapshot.source);
    let destination_scope = OutputScope::conversation(destination);
    let mut messages = Vec::with_capacity(snapshot.messages.len());
    for message in &snapshot.messages {
        let mut copied = message.clone();
        if copied.role == MessageRole::Assistant {
            let job = JobId::generate().map_err(|_| ForkError::Random)?;
            copied.request = Some(job);
            for activity in &mut copied.activity {
                match activity {
                    AssistantActivity::ToolCall {
                        id,
                        result: Some(tool),
                        ..
                    } => {
                        let call = id.clone();
                        rebind_tool(tool, &call, &source_scope, &destination_scope, job, outputs);
                    }
                    AssistantActivity::Tool(tool) => {
                        let call = copied.id.as_hex();
                        rebind_tool(tool, &call, &source_scope, &destination_scope, job, outputs);
                    }
                    _ => {}
                }
            }
        } else {
            copied.request = None;
        }
        messages.push(copied);
    }
    Ok(messages)
}

fn rebind_tool(
    tool: &mut ToolOutput,
    call: &str,
    source: &OutputScope,
    destination: &OutputScope,
    job: JobId,
    outputs: &OutputStore,
) {
    let Some(command) = tool.command.as_mut() else {
        return;
    };
    let Some(retained) = command.retained.as_ref() else {
        return;
    };
    let reference = retained.reference.clone();
    let key = OutputKey {
        scope: destination.clone(),
        job,
        tool_call: call.to_owned(),
    };
    match outputs.rebind(&reference, source, &key) {
        Ok(rebound) => command.retained = Some(rebound),
        // Storage pressure degrades the link, never the bounded preview.
        Err(_) => command.retained = None,
    }
}

fn review_only(configuration: &ConversationModelConfiguration) -> ConversationModelConfiguration {
    let mut settings = configuration.settings.clone();
    settings.directories.clear();
    settings.tools.clear();
    settings.location = crate::execution::ToolLocation::Sandbox;
    settings.host_approval = crate::execution::HostApprovalPolicy::AskEachTime;
    ConversationModelConfiguration {
        settings,
        preset: None,
    }
}

#[cfg(test)]
mod tests;
