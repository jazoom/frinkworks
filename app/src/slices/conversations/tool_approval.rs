use axum::{
    Form,
    extract::{Path, State},
    response::Response,
};
use hypergraft::{PatchGraft, PatchStatus};
use serde::Deserialize;

use crate::{
    error::AppResult,
    execution::HostCommandDecision,
    responses,
    sessions::{JobId, RequiredSession},
    state::AppState,
};

use super::{
    REVISION_MESSAGE, detail_view, load_conversation, parse_revision, render_detail_command,
};

#[derive(Deserialize)]
pub(super) struct HostCommandForm {
    revision: String,
    job: String,
    request: String,
    command: String,
}

pub(super) async fn approve(
    state: State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    path: Path<String>,
    form: Form<HostCommandForm>,
) -> AppResult<Response> {
    decide(
        state,
        session,
        graft,
        path,
        form,
        HostCommandDecision::Approved,
    )
    .await
}

pub(super) async fn reject(
    state: State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    path: Path<String>,
    form: Form<HostCommandForm>,
) -> AppResult<Response> {
    decide(
        state,
        session,
        graft,
        path,
        form,
        HostCommandDecision::Rejected,
    )
    .await
}

async fn decide(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path(conversation_id): Path<String>,
    Form(form): Form<HostCommandForm>,
    decision: HostCommandDecision,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let Some(revision) = parse_revision(&form.revision) else {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(&state, session.0, &record, &record.title, REVISION_MESSAGE),
        );
    };
    let Some(job_id) = JobId::parse(&form.job) else {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                "That command approval is not valid.",
            ),
        );
    };
    let Some(job) = state.sessions.conversation_job(record.id, job_id) else {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                "This reply is no longer active.",
            ),
        );
    };
    let pending = state.host_approvals.pending_for(record.id, job_id);
    if job.cancel_requested() {
        state.host_approvals.invalidate_job(job_id);
    }
    // JSON escapes preserve shell newlines across HTML form normalisation.
    let command: String = serde_json::from_str(&form.command).unwrap_or_default();
    match state.host_approvals.decide(
        &crate::execution::HostCommandRequest {
            token: form.request,
            session: session.0,
            job: job_id,
            conversation: record.id,
            execution_revision: revision,
            location: pending
                .as_ref()
                .map_or(crate::execution::ToolLocation::Sandbox, |request| {
                    request.location
                }),
            command: command.clone(),
            directory: pending
                .as_ref()
                .map(|request| request.directory.clone())
                .unwrap_or_default(),
            explanation: pending
                .as_ref()
                .map(|request| request.explanation.clone())
                .unwrap_or_default(),
            run: pending.as_ref().and_then(|request| request.run.clone()),
            step: pending.as_ref().and_then(|request| request.step.clone()),
            attempt: pending.and_then(|request| request.attempt),
        },
        decision,
    ) {
        Ok(()) => {
            let _ = job.resume();
            render_detail_command(
                graft,
                PatchStatus::Ok,
                detail_view(&state, session.0, &record, &record.title, "")
                    .with_command_decision(command, decision),
            )
        }
        Err(crate::execution::ApprovalError::Duplicate) => render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                crate::execution::ApprovalError::Duplicate.message(),
            ),
        ),
        Err(error) => render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(&state, session.0, &record, &record.title, error.message()),
        ),
    }
}

#[cfg(test)]
mod tests;
