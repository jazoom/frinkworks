use std::sync::Arc;
use std::time::Duration;

use hypergraft::{PatchSet, PatchStatus};
use tokio::sync::mpsc;

use crate::{
    agents::DirectoryPolicy,
    conversations::{ConversationId, ConversationRecord, MessageStatus},
    execution::{AgentOutcome, AgentRunSpec, ToolLocation},
    providers::{AssistantReply, ChatTurn, ProviderConnection},
    sessions::{Job, JobStatus, SessionId},
    state::AppState,
};

use super::page::{ConversationObserveContents, MessageBody};

const OBSERVE_WAIT: Duration = Duration::from_secs(20);
const OBSERVE_SEGMENT_MAX: Duration = Duration::from_secs(25);

pub(super) async fn run(
    state: AppState,
    session: SessionId,
    conversation: ConversationId,
    record: ConversationRecord,
    connection: ProviderConnection,
    job: Arc<Job>,
) {
    let language = state.sessions.language(&session);
    let mut instructions = instructions(&state, &record);
    if let Some(language) = &language {
        language.append_instructions(&mut instructions);
    }
    let secret = match connection.auth {
        crate::providers::AuthMethod::ApiKey => Some(connection.api_key.expose().to_owned()),
        crate::providers::AuthMethod::Plan => None,
    };
    let secret = secret.as_deref();
    let context = history_with_review(&state, &record, secret).map(|history| (history, Vec::new()));
    let (reply, outcome, error, budget) = match context {
        Ok((history, sources)) => {
            let spec = AgentRunSpec {
                agent_id: None,
                revision: record.revision,
                preamble: instructions,
                context_prefix_len: 0,
                tools: Vec::new(),
                tool_ids: Vec::new(),
                policy: DirectoryPolicy::from_grants(Vec::new(), String::new()),
                connection,
                location: ToolLocation::Sandbox,
                sandbox: None,
                host: None,
                output_drafts: None,
                required_outputs: Vec::new(),
                evidence: None,
                output_scope: None,
                conversation: Some(conversation),
                steering_session: Some(session),
                budget: crate::execution::BudgetPolicy::ordinary(),
                sources,
                advertised: Vec::new(),
            };
            let ended =
                crate::execution::run_agent_action(&state, spec, history, job.clone()).await;
            (ended.reply, ended.outcome, ended.error, ended.budget)
        }
        Err(error) => (
            AssistantReply::default(),
            AgentOutcome::ProviderFailure,
            Some(error.to_owned()),
            None,
        ),
    };
    if outcome == AgentOutcome::BudgetExhausted {
        crate::execution::conversation::settle_pause(
            &state,
            session,
            &state.conversations.get(&conversation).unwrap_or(record),
            &job,
            None,
            reply,
            budget,
        );
        return;
    }
    let (status, message_status) = match outcome {
        AgentOutcome::Completed => (JobStatus::Completed, MessageStatus::Complete),
        AgentOutcome::Cancelled => (JobStatus::Cancelled, MessageStatus::Interrupted),
        AgentOutcome::ProviderFailure
        | AgentOutcome::ToolFailure
        | AgentOutcome::AuthorityFailure
        | AgentOutcome::PersistenceFailure
        | AgentOutcome::UncertainEffect
        | AgentOutcome::BudgetExhausted
        | AgentOutcome::ContextBlocked => (JobStatus::Failed, MessageStatus::Failed),
    };
    let error = error
        .and_then(|text| crate::providers::sanitise_detail(&crate::tools::redact(&text, secret)));
    let settlement = state.conversations.settle_message(
        &conversation,
        job.id(),
        reply,
        message_status,
        error.clone(),
    );
    if settlement.is_ok() {
        crate::conversations::titles::start(&state, conversation, language);
    }
    if settlement.is_ok() || settlement == Err(crate::conversations::ConversationError::Conflict) {
        job.finish(status, error.as_deref());
        state
            .sessions
            .finish_conversation_job(&session, conversation, job.id());
    } else {
        job.finish(
            JobStatus::Failed,
            Some("Frinkworks could not store the reply. Try again."),
        );
    }
}

fn instructions(_state: &AppState, record: &ConversationRecord) -> String {
    record
        .model
        .as_ref()
        .map_or_else(String::new, |model| model.settings.instructions.clone())
}

pub(crate) fn history_with_review(
    state: &AppState,
    record: &ConversationRecord,
    _secret: Option<&str>,
) -> Result<Vec<ChatTurn>, &'static str> {
    let selection = record.model.as_ref().map(|model| &model.settings.model);
    let history = crate::conversations::compaction::project_with_attachments(
        &record.messages,
        selection,
        record.compaction.as_ref(),
        Some(state.conversations.attachment_store()),
    )
    .map_err(|error| error.message())?;
    Ok(history)
}

pub(super) fn observe_response(
    state: AppState,
    conversation: ConversationId,
    session: SessionId,
    job: Arc<Job>,
    cursor: u64,
    historical: bool,
) -> axum::response::Response {
    let (tx, rx) = mpsc::channel(4);
    tokio::spawn(observe_segment(
        tx,
        state,
        conversation,
        session,
        job,
        cursor,
        historical,
    ));
    hypergraft::outcome::stream_response(futures_util::stream::unfold(rx, |mut rx| async {
        rx.recv().await.map(|frame| (frame, rx))
    }))
}

async fn observe_segment(
    tx: mpsc::Sender<hypergraft::StreamFrame>,
    state: AppState,
    conversation: ConversationId,
    session: SessionId,
    job: Arc<Job>,
    cursor: u64,
    historical: bool,
) {
    let mut cursor = cursor;
    let mut budget = hypergraft::StreamBudget::new();
    let job_id = job.id().as_hex();
    job.wait_after(cursor, OBSERVE_WAIT).await;
    let started = std::time::Instant::now();
    while job.latest_seq() > cursor {
        let snapshot = job.snapshot();
        let output = job.output_up_to(snapshot.latest_seq);
        if snapshot.status == JobStatus::Running || !output.is_empty() || snapshot.retry.is_some() {
            let frame = if historical {
                historical_status_frame(
                    &conversation,
                    &job_id,
                    snapshot.latest_seq,
                    historical,
                    if snapshot.retry.is_some() {
                        "Retrying the provider."
                    } else {
                        "Work is in progress."
                    },
                    &mut budget,
                )
            } else if let Some((anchor, committed)) = active_response(&state, &conversation, &job) {
                progress_frame(
                    &conversation,
                    &job_id,
                    anchor,
                    &committed,
                    snapshot.latest_seq,
                    &output,
                    snapshot.compacting,
                    snapshot
                        .retry
                        .as_ref()
                        .map(|retry| super::page::retry_status_message(retry.attempt, retry.delay)),
                    historical,
                    &mut budget,
                )
            } else {
                None
            };
            if let Some(frame) = frame
                && tx.send(frame).await.is_err()
            {
                return;
            }
        }
        cursor = snapshot.latest_seq;
        if !job.is_running() || started.elapsed() >= OBSERVE_SEGMENT_MAX {
            break;
        }
        job.wait_after(cursor, OBSERVE_WAIT).await;
    }
    let _ = tx
        .send(final_frame(
            &state,
            &conversation,
            session,
            &job,
            cursor,
            historical,
        ))
        .await;
}

/// The logical response for a live job: its stable anchor and every settled
/// phase before the active one. The live reply always represents the pending
/// phase, so it is never included twice.
fn active_response(
    state: &AppState,
    conversation: &ConversationId,
    job: &Job,
) -> Option<(
    crate::conversations::MessageId,
    Vec<crate::conversations::ConversationMessage>,
)> {
    let (record, _) = state
        .conversations
        .transcript_window(conversation, None, None)
        .ok()
        .flatten()?;
    let anchor = record
        .messages
        .iter()
        .rev()
        .find(|message| {
            message.role == crate::conversations::MessageRole::Assistant
                && message.request == Some(job.id())
        })
        .map(|message| message.response.unwrap_or(message.id))?;
    let committed = record
        .messages
        .iter()
        .filter(|message| {
            message.role == crate::conversations::MessageRole::Assistant
                && message.response.unwrap_or(message.id) == anchor
                && message.status != MessageStatus::Pending
        })
        .cloned()
        .collect();
    Some((anchor, committed))
}

#[allow(clippy::too_many_arguments)]
fn progress_frame(
    conversation: &ConversationId,
    job_id: &str,
    anchor: crate::conversations::MessageId,
    committed: &[crate::conversations::ConversationMessage],
    cursor: u64,
    reply: &AssistantReply,
    compacting: bool,
    retry_message: Option<String>,
    historical: bool,
    budget: &mut hypergraft::StreamBudget,
) -> Option<hypergraft::StreamFrame> {
    let phases: Vec<&crate::conversations::ConversationMessage> = committed.iter().collect();
    let mut message =
        super::page::response_view(conversation, anchor, &phases, Some((reply, true)));
    if let Some(retry_message) = retry_message {
        message.status = "Retrying the provider";
        message.retry_message = retry_message;
    }
    if compacting {
        message.status = "Compacting context";
    }
    let mut patches = PatchSet::new();
    patches
        .children(&message.id, &MessageBody { message: &message })
        .ok()?;
    patches
        .children(
            "conversation-observe",
            &ConversationObserveContents {
                id: &conversation.as_hex(),
                job_id,
                cursor,
                active: true,
                historical,
            },
        )
        .ok()?;
    let frame = patches.encode_progress().ok()?;
    budget.try_progress(&frame).ok()?;
    Some(frame)
}

/// A historical window has no live response element. Progress patches only the
/// observation cursor and the status line, never an absent transcript entry.
fn historical_status_frame(
    conversation: &ConversationId,
    job_id: &str,
    cursor: u64,
    historical: bool,
    status: &str,
    budget: &mut hypergraft::StreamBudget,
) -> Option<hypergraft::StreamFrame> {
    let latest_href = format!("/conversations/{}", conversation.as_hex());
    let mut patches = PatchSet::new();
    patches
        .children(
            "conversation-observe",
            &ConversationObserveContents {
                id: &conversation.as_hex(),
                job_id,
                cursor,
                active: true,
                historical,
            },
        )
        .ok()?;
    patches
        .children(
            "conversation-history-status",
            &super::page::HistoryStatusContents {
                history_status: status,
                latest_href: &latest_href,
            },
        )
        .ok()?;
    let frame = patches.encode_progress().ok()?;
    budget.try_progress(&frame).ok()?;
    Some(frame)
}

fn final_frame(
    state: &AppState,
    conversation: &ConversationId,
    session: SessionId,
    job: &Job,
    cursor: u64,
    historical: bool,
) -> hypergraft::StreamFrame {
    let snapshot = job.snapshot();
    let record = if historical {
        None
    } else {
        state
            .conversations
            .transcript_window(conversation, None, None)
            .ok()
            .flatten()
    };
    let mut patches = PatchSet::new();
    // A historical window keeps its position. Settlement patches status only,
    // never the whole detail view or an absent transcript entry.
    if !historical
        && snapshot.status != JobStatus::Running
        && let Some((record, window)) = &record
    {
        let view = super::detail_view_with_transcript(
            state,
            session,
            record,
            &record.title,
            "",
            Some(window),
            None,
        );
        if patches
            .children("conversation-detail", &view.contents())
            .is_ok()
            && let Ok(frame) = patches.encode_final(PatchStatus::Ok)
        {
            return frame;
        }
        patches = PatchSet::new();
    }
    if !historical
        && let Some((record, _)) = record
        && let Some(anchor) = record
            .messages
            .iter()
            .rev()
            .find(|message| {
                message.role == crate::conversations::MessageRole::Assistant
                    && message.request == Some(job.id())
            })
            .map(|message| message.response.unwrap_or(message.id))
    {
        let phases: Vec<&crate::conversations::ConversationMessage> = record
            .messages
            .iter()
            .filter(|message| {
                message.role == crate::conversations::MessageRole::Assistant
                    && message.response.unwrap_or(message.id) == anchor
                    && message.status != MessageStatus::Pending
            })
            .collect();
        let live = (snapshot.status == JobStatus::Running).then_some((&snapshot.output, true));
        let mut message = super::page::response_view(conversation, anchor, &phases, live);
        if snapshot.status == JobStatus::Running
            && let Some(retry) = &snapshot.retry
        {
            message.status = "Retrying the provider";
            message.retry_message = super::page::retry_status_message(retry.attempt, retry.delay);
        }
        if snapshot.status == JobStatus::Running && snapshot.compacting {
            message.status = "Compacting context";
        }
        let _ = patches.children(&message.id, &MessageBody { message: &message });
    }
    let active = snapshot.status == JobStatus::Running;
    let id = conversation.as_hex();
    let job_id = job.id().as_hex();
    if historical {
        let latest_href = format!("/conversations/{id}");
        let _ = patches.children(
            "conversation-history-status",
            &super::page::HistoryStatusContents {
                history_status: if snapshot.status == JobStatus::Running {
                    "Work is in progress."
                } else {
                    "New output is available."
                },
                latest_href: &latest_href,
            },
        );
    }
    let _ = patches.children(
        "conversation-observe",
        &ConversationObserveContents {
            id: &id,
            job_id: &job_id,
            cursor,
            active,
            historical,
        },
    );
    patches
        .encode_final(if snapshot.status == JobStatus::Failed {
            PatchStatus::UnprocessableEntity
        } else {
            PatchStatus::Ok
        })
        .unwrap_or_else(|_| {
            let mut fallback = PatchSet::new();
            fallback
                .children(
                    "conversation-observe",
                    &ConversationObserveContents {
                        id: &id,
                        job_id: &job_id,
                        cursor,
                        active: false,
                        historical,
                    },
                )
                .expect("bounded observation controls");
            fallback
                .encode_final(PatchStatus::UnprocessableEntity)
                .expect("bounded final frame")
        })
}

#[cfg(test)]
mod tests;
