#[cfg(test)]
mod tests;

use askama::Template;
use axum::extract::State;
use hypergraft::{
    PatchSet, PatchStatus,
    live::{LiveProjection, LiveReject, ProjectionError},
};

use crate::{error::AppResult, sessions::SessionId, state::AppState};

#[derive(Template)]
#[template(source = "{% if count > 0 %}{{ count }}{% endif %}", ext = "html")]
struct AttentionCount {
    count: usize,
}

pub(crate) fn status_dot(status: &str) -> &'static str {
    match status {
        "Needs your review" | "Needs command approval" | "Needs recovery" | "Paused" => "attention",
        "In progress" | "Active" | "Awaiting decision" => "active",
        _ => "quiet",
    }
}

pub(crate) fn conversation_status(
    state: &AppState,
    record: &crate::conversations::ConversationMetadata,
) -> &'static str {
    let active = state.workflow_runs.active_runs();
    if active.iter().any(|run| {
        run.conversation_id == Some(record.id)
            && run
                .gates
                .iter()
                .any(|gate| gate.state == crate::workflows::gates::HumanGateState::AwaitingDecision)
    }) {
        "Needs your review"
    } else if record
        .active_job
        .is_some_and(|job| state.host_approvals.pending_for(record.id, job).is_some())
    {
        "Needs command approval"
    } else if record.active_job.is_some_and(|job| {
        state
            .conversations
            .pending_question(record.id, job)
            .is_some()
    }) {
        "Needs an answer"
    } else if let Some(job) = record.active_job {
        if state
            .sessions
            .conversation_job(record.id, job)
            .is_some_and(|job| job.snapshot().status == crate::sessions::JobStatus::Failed)
        {
            "Needs recovery"
        } else {
            "In progress"
        }
    } else if record.continuation {
        "Paused"
    } else {
        let latest = state
            .workflow_runs
            .for_conversation(&record.id)
            .into_iter()
            .next();
        if let Some(run) = latest {
            super::page::workflow_progress(&run).state
        } else {
            idle_status(record.last_message_status)
        }
    }
}

pub(crate) fn conversation_meta(
    state: &AppState,
    record: &crate::conversations::ConversationMetadata,
) -> String {
    let directory = record
        .model
        .iter()
        .flat_map(|model| &model.settings.directories)
        .next()
        .map_or_else(
            || "No directory access".to_owned(),
            |grant| grant.host_path.display().to_string(),
        );
    let process = state
        .workflow_runs
        .for_conversation(&record.id)
        .into_iter()
        .next()
        .map(|run| run.pinned.definition.name().to_owned())
        .unwrap_or_else(|| "No runs yet".to_owned());
    format!("{directory} · {process}")
}

/// Idle status for records without active work or runs. Saved records
/// without a first message are drafts. Responsive idle records stay ready.
fn idle_status(last: Option<crate::conversations::MessageStatus>) -> &'static str {
    match last {
        None => "Draft",
        Some(crate::conversations::MessageStatus::Failed) => "Response failed",
        Some(crate::conversations::MessageStatus::Interrupted) => "Interrupted",
        Some(crate::conversations::MessageStatus::Pending) => "In progress",
        Some(crate::conversations::MessageStatus::Complete) => "Ready",
    }
}

fn patches(state: &AppState) -> Result<PatchSet, hypergraft::PatchBuildError> {
    PatchSet::new().with_children(
        "attention-count",
        &AttentionCount {
            count: crate::slices::attention::page::AttentionPage::count(state),
        },
    )
}

pub(super) fn response(state: &AppState) -> AppResult<axum::response::Response> {
    Ok(patches(state)?.respond(PatchStatus::Ok)?)
}

pub(super) async fn live(
    State(state): State<AppState>,
) -> Result<LiveProjection<SessionId>, LiveReject> {
    // Subscribe before Hypergraft reads the initial snapshot so concurrent changes stay visible.
    let changes = futures_util::stream::select_all(
        [
            state.conversations.changes.subscribe(),
            state.workflow_runs.changes.subscribe(),
            state.host_approvals.changes.subscribe(),
        ]
        .into_iter()
        .map(|receiver| Box::pin(hypergraft::live::broadcast_invalidations(receiver))),
    );
    Ok(LiveProjection::new(changes, move |_| {
        let state = state.clone();
        async move { patches(&state).map_err(|_| ProjectionError::Retire) }
    }))
}
