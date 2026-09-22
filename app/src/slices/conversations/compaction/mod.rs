mod page;

#[cfg(test)]
mod tests;

pub(super) use page::CompactionView;

use axum::{
    Form,
    extract::{Path, State},
    response::Response,
};
use hypergraft::{PatchGraft, PatchStatus};
use serde::Deserialize;

use super::{
    REVISION_MESSAGE, conversation_busy, detail_view, load_conversation, parse_revision,
    render_detail_command,
};
use crate::{
    conversations::compaction::{self, CompactionError},
    error::AppResult,
    sessions::RequiredSession,
    state::AppState,
};

#[derive(Deserialize)]
pub(super) struct CompactForm {
    revision: String,
    #[serde(default)]
    preserve: String,
}

pub(super) async fn compact(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path(conversation_id): Path<String>,
    Form(form): Form<CompactForm>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(crate::responses::command_navigation("/conversations"));
    };
    let reject = |status, error| {
        render_detail_command(
            graft,
            status,
            detail_view(&state, session.0, &record, &record.title, error),
        )
    };
    if parse_revision(&form.revision) != Some(record.revision) {
        return reject(PatchStatus::Conflict, REVISION_MESSAGE);
    }
    if conversation_busy(&state, &record)
        || record.continuation.is_some()
        || state.workflow_runs.active_runs().iter().any(|run| {
            run.conversation_id == Some(record.id)
                && matches!(
                    run.state,
                    crate::workflows::run::RunState::AwaitingHuman { .. }
                )
        })
    {
        return reject(PatchStatus::Conflict, CompactionError::Unsettled.message());
    }
    let Some(model) = &record.model else {
        return reject(
            PatchStatus::UnprocessableEntity,
            "Choose an available model before you compact context.",
        );
    };
    let Some(connection) = state.vault.connection_for(&model.settings.model) else {
        return reject(
            PatchStatus::UnprocessableEntity,
            "Choose an available model before you compact context.",
        );
    };
    let preserve = match compaction::normalise_preserve(Some(&form.preserve)) {
        Ok(preserve) => preserve,
        Err(error) => return reject(PatchStatus::UnprocessableEntity, error.message()),
    };
    let selection = model.settings.model.clone();
    let instructions = model.settings.instructions.clone();
    let budget = compaction::model_retention_budget(
        &selection,
        &instructions,
        state
            .models_dev
            .context_limit(selection.provider, &selection.model),
        state
            .models_dev
            .output_limit(selection.provider, &selection.model),
        record
            .compaction
            .as_ref()
            .map(|current| current.text.as_str()),
    );
    let (covered_through, retained_from) = match compaction::select_boundary(
        &record.messages,
        record.compaction.as_ref(),
        Some(&selection),
        budget,
    ) {
        Ok(boundary) => boundary,
        Err(error) => return reject(PatchStatus::UnprocessableEntity, error.message()),
    };
    let covered = match compaction::covered_turns(
        &record.messages,
        Some(&model.settings.model),
        covered_through,
        record.compaction.as_ref(),
    ) {
        Ok(turns) => turns,
        Err(error) => return reject(PatchStatus::UnprocessableEntity, error.message()),
    };
    let guard = match state.workflow_execution.acquire() {
        Ok(guard) => guard,
        Err(error) => return reject(PatchStatus::Conflict, error),
    };
    let job = match state.sessions.begin_conversation_job(&session.0, record.id) {
        Ok(job) => job,
        Err(_) => return reject(PatchStatus::Conflict, CompactionError::Unsettled.message()),
    };
    let started = match state
        .conversations
        .begin_compaction(&record.id, record.revision, job.id())
    {
        Ok(started) => started,
        Err(error) => {
            if error == crate::conversations::ConversationError::Unsettled {
                guard.require_recovery();
                job.finish(crate::sessions::JobStatus::Failed, Some(error.message()));
            } else {
                state
                    .sessions
                    .finish_conversation_job(&session.0, record.id, job.id());
            }
            return reject(PatchStatus::Conflict, error.message());
        }
    };
    job.set_compacting();
    let response = render_detail_command(
        graft,
        PatchStatus::Ok,
        detail_view(&state, session.0, &started, &started.title, ""),
    )?;
    tokio::spawn(async move {
        let result = compaction::generation::generate(
            &state,
            &connection,
            &covered,
            preserve.as_deref(),
            &job,
            |request| {
                state
                    .conversations
                    .record_summary_request(&record.id, job.id(), request)
                    .map_err(|error| error.message())
            },
        )
        .await
        .and_then(|outcome| {
            if job.cancel_requested() {
                return Err("Context compaction was cancelled. The previous context remains.");
            }
            let candidate = compaction::CompactionRecord {
                covered_through,
                retained_from,
                text: outcome.text,
                requests: outcome.requests,
                preserve,
                created_at_ms: crate::workflows::now_ms(),
            };
            // The replacement request must fit before the checkpoint commits.
            let replacement =
                compaction::project(&record.messages, Some(&selection), Some(&candidate))
                    .map_err(|error| error.message())?;
            let request = crate::execution::ContextRequest {
                preamble: &instructions,
                tools: &[],
                turns: &replacement,
                extra: &[],
                provider: selection.provider,
                model: &selection.model,
                output_limit: state
                    .models_dev
                    .output_limit(selection.provider, &selection.model),
            };
            crate::execution::context::measure(
                request,
                state
                    .models_dev
                    .context_limit(selection.provider, &selection.model),
            )
            .map_err(|error| error.message())?;
            state
                .conversations
                .record_job_compaction(&record.id, job.id(), candidate)
                .map(|_| ())
                .map_err(|error| error.message())
        });
        job.clear_compacting();
        let error = result.err();
        if state
            .conversations
            .finish_compaction(&record.id, job.id(), error)
            .is_ok()
        {
            job.finish(
                if job.cancel_requested() {
                    crate::sessions::JobStatus::Cancelled
                } else if error.is_some() {
                    crate::sessions::JobStatus::Failed
                } else {
                    crate::sessions::JobStatus::Completed
                },
                error,
            );
            state
                .sessions
                .finish_conversation_job(&session.0, record.id, job.id());
        } else {
            guard.require_recovery();
            job.finish(
                crate::sessions::JobStatus::Failed,
                Some(CompactionError::Persist.message()),
            );
        }
    });
    Ok(response)
}
