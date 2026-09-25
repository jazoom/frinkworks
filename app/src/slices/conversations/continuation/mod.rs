mod page;

#[cfg(test)]
mod tests;

pub(super) use page::ContinuationView;

use axum::{
    Form,
    extract::{Path, State},
    response::Response,
};
use hypergraft::{PatchGraft, PatchStatus};
use serde::Deserialize;

use crate::{
    conversations::{CheckpointId, ConversationError},
    error::AppResult,
    sessions::RequiredSession,
    state::AppState,
    workflows,
};

use super::{
    REVISION_MESSAGE, StartMessageError, detail_view, load_conversation, parse_revision,
    preflight_execution, render_detail_command, spawn_conversation_work,
};

#[derive(Deserialize)]
pub(super) struct ContinueForm {
    revision: String,
    checkpoint: String,
    #[serde(default)]
    run: String,
    #[serde(default)]
    attempt: String,
    #[serde(default)]
    step: String,
    #[serde(default)]
    approval: String,
}

pub(super) async fn resume(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path(conversation_id): Path<String>,
    Form(form): Form<ContinueForm>,
) -> AppResult<Response> {
    decide(state, session, graft, conversation_id, form, false).await
}

pub(super) async fn end_pause(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path(conversation_id): Path<String>,
    Form(form): Form<ContinueForm>,
) -> AppResult<Response> {
    decide(state, session, graft, conversation_id, form, true).await
}

async fn decide(
    state: AppState,
    session: RequiredSession,
    graft: PatchGraft,
    conversation_id: String,
    form: ContinueForm,
    end: bool,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(crate::responses::command_navigation("/conversations"));
    };
    let Some(revision) = parse_revision(&form.revision) else {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(&state, session.0, &record, &record.title, REVISION_MESSAGE),
        );
    };
    let Some(checkpoint) = CheckpointId::parse(&form.checkpoint) else {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                "That continuation is no longer valid.",
            ),
        );
    };
    let Some(stored) = record.continuation.clone() else {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                "That continuation is no longer valid.",
            ),
        );
    };
    if stored.id != checkpoint
        || revision != record.revision
        || record.active_job.is_some()
        || (stored.run.is_none()
            && (!form.run.is_empty() || !form.attempt.is_empty() || !form.step.is_empty()))
    {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                "That continuation is no longer valid.",
            ),
        );
    }
    if end {
        return end_checkpoint(&state, session, graft, record, revision, checkpoint).await;
    }
    continue_checkpoint(&state, session, graft, record, revision, form, stored).await
}

async fn end_checkpoint(
    state: &AppState,
    session: RequiredSession,
    graft: PatchGraft,
    record: crate::conversations::ConversationRecord,
    revision: u32,
    checkpoint: CheckpointId,
) -> AppResult<Response> {
    let Ok(reservation) = state.sessions.begin_conversation_job(&session.0, record.id) else {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(
                state,
                session.0,
                &record,
                &record.title,
                ConversationError::Active.message(),
            ),
        );
    };
    let cleared = state.conversations.clear_continuation(
        &record.id,
        revision,
        checkpoint,
        &state.workflow_runs,
    );
    state
        .sessions
        .finish_conversation_job(&session.0, record.id, reservation.id());
    match cleared {
        Ok(updated) => render_detail_command(
            graft,
            PatchStatus::Ok,
            detail_view(state, session.0, &updated, &updated.title, ""),
        ),
        Err(error) => render_detail_command(
            graft,
            status_for(error),
            detail_view(state, session.0, &record, &record.title, error.message()),
        ),
    }
}

async fn continue_checkpoint(
    state: &AppState,
    session: RequiredSession,
    graft: PatchGraft,
    record: crate::conversations::ConversationRecord,
    revision: u32,
    form: ContinueForm,
    stored: crate::conversations::ContinuationCheckpoint,
) -> AppResult<Response> {
    let pinned = stored
        .run
        .and_then(|id| state.workflow_runs.get(&id))
        .and_then(|run| {
            stored
                .step
                .as_ref()
                .and_then(|step| crate::workflows::definition::StepKey::parse(step).ok())
                .and_then(|step| run.phase_settings(&step).cloned())
        })
        .or_else(|| record.model.as_ref().map(|model| model.settings.clone()));
    if pinned.as_ref() != Some(&stored.pinned) {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(
                state,
                session.0,
                &record,
                &record.title,
                "Settings changed. Continue stays unavailable until the pinned settings match.",
            ),
        );
    }
    let Some(mut model) = record.model.clone() else {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(
                state,
                session.0,
                &record,
                &record.title,
                ConversationError::Selection.message(),
            ),
        );
    };
    if stored.run.is_some() && form.approval != "continue-run" {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(
                state,
                session.0,
                &record,
                &record.title,
                "Approve the pinned execution settings for this continuation.",
            ),
        );
    }
    if let Some(run_id) = stored.run {
        let Some(run) = state.workflow_runs.get(&run_id) else {
            return render_detail_command(
                graft,
                PatchStatus::Conflict,
                detail_view(
                    state,
                    session.0,
                    &record,
                    &record.title,
                    "That continuation is no longer valid.",
                ),
            );
        };
        if run.conversation_id != Some(record.id)
            || run.pending_handoff.is_some()
            || !matches!(
                &run.state,
                crate::workflows::run::RunState::Paused { checkpoint, step, attempt }
                    if *checkpoint == stored.id && Some(*attempt) == stored.attempt
                        && Some(step.as_str()) == stored.step.as_deref()
            )
        {
            return render_detail_command(
                graft,
                PatchStatus::Conflict,
                detail_view(
                    state,
                    session.0,
                    &record,
                    &record.title,
                    "That continuation is no longer valid.",
                ),
            );
        }
        if form.run != run_id.as_hex()
            || stored
                .attempt
                .is_none_or(|attempt| form.attempt != attempt.as_hex())
            || stored.step.as_deref() != Some(form.step.as_str()).filter(|step| !step.is_empty())
        {
            return render_detail_command(
                graft,
                PatchStatus::Conflict,
                detail_view(
                    state,
                    session.0,
                    &record,
                    &record.title,
                    "That continuation is no longer valid.",
                ),
            );
        }
    }
    if let Some(run_id) = stored.run {
        let validation = state
            .workflow_runs
            .get(&run_id)
            .ok_or("That continuation is no longer valid.")
            .and_then(|run| super::handoff::transfer::validate_pinned(state, &run));
        if state.conversation_runtime.unsettled(record.id)
            || super::has_pending_review(state, record.id)
            || validation.is_err()
        {
            return render_detail_command(
                graft,
                PatchStatus::Conflict,
                detail_view(
                    state,
                    session.0,
                    &record,
                    &record.title,
                    validation
                        .err()
                        .unwrap_or("Resolve the pending decision or recovery before continuation."),
                ),
            );
        }
        return continue_workflow(state, session, graft, record, revision, stored).await;
    }
    model.settings = stored.pinned.clone();
    match preflight_execution(state, session.0, Some(record.id), &model).await {
        Ok(()) => {}
        Err(StartMessageError::User(status, error)) => {
            return render_detail_command(
                graft,
                status,
                detail_view(state, session.0, &record, &record.title, error),
            );
        }
    }
    continue_ordinary(state, session, graft, record, revision, stored, model).await
}

async fn continue_ordinary(
    state: &AppState,
    session: RequiredSession,
    graft: PatchGraft,
    record: crate::conversations::ConversationRecord,
    revision: u32,
    stored: crate::conversations::ContinuationCheckpoint,
    model: crate::conversations::ConversationModelConfiguration,
) -> AppResult<Response> {
    let Some(connection) = state.vault.connection_for(&model.settings.model) else {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(
                state,
                session.0,
                &record,
                &record.title,
                "Choose a stored provider.",
            ),
        );
    };
    let ordinary = crate::execution::ordinary_kind(&model.settings);
    let execution = if matches!(
        ordinary,
        Some(crate::execution::OrdinaryKind::Host | crate::execution::OrdinaryKind::Sandbox)
    ) {
        match state.workflow_execution.acquire() {
            Ok(execution) => Some(execution),
            Err(error) => {
                return render_detail_command(
                    graft,
                    PatchStatus::Conflict,
                    detail_view(state, session.0, &record, &record.title, error),
                );
            }
        }
    } else {
        None
    };
    let job = match state.sessions.begin_conversation_job(&session.0, record.id) {
        Ok(job) => job,
        Err(_) => {
            return render_detail_command(
                graft,
                PatchStatus::Conflict,
                detail_view(
                    state,
                    session.0,
                    &record,
                    &record.title,
                    ConversationError::Active.message(),
                ),
            );
        }
    };
    let claimed =
        match state
            .conversations
            .claim_continuation(&record.id, revision, stored.id, job.id())
        {
            Ok(claimed) => claimed,
            Err(error) => {
                state
                    .sessions
                    .finish_conversation_job(&session.0, record.id, job.id());
                return render_detail_command(
                    graft,
                    status_for(error),
                    detail_view(state, session.0, &record, &record.title, error.message()),
                );
            }
        };
    let secret = match connection.auth {
        crate::providers::AuthMethod::ApiKey => Some(connection.api_key.expose()),
        crate::providers::AuthMethod::Plan => None,
    };
    let turns = match super::job::history_with_review(state, &claimed, secret) {
        Ok(turns) => turns,
        Err(error) => {
            let _ = state.conversations.settle_message(
                &claimed.id,
                job.id(),
                String::new(),
                crate::conversations::MessageStatus::Failed,
                Some(error.to_owned()),
            );
            state
                .sessions
                .finish_conversation_job(&session.0, claimed.id, job.id());
            return render_detail_command(
                graft,
                PatchStatus::UnprocessableEntity,
                detail_view(state, session.0, &record, &record.title, error),
            );
        }
    };
    if matches!(
        ordinary,
        Some(crate::execution::OrdinaryKind::Host | crate::execution::OrdinaryKind::Sandbox)
    ) {
        let kind = ordinary.expect("host or sandbox continuation");
        let execution = execution.expect("ordinary continuation holds reset protection");
        spawn_conversation_work(
            state.clone(),
            session.0,
            claimed.id,
            crate::execution::conversation::run(
                state.clone(),
                crate::execution::conversation::OrdinaryRun {
                    session: session.0,
                    record: claimed.clone(),
                    connection,
                    job,
                    execution,
                    kind,
                    turns,
                },
            ),
        );
    } else {
        spawn_conversation_work(
            state.clone(),
            session.0,
            claimed.id,
            super::job::run(
                state.clone(),
                session.0,
                claimed.id,
                claimed.clone(),
                connection,
                job,
            ),
        );
    }
    let updated = state.conversations.get(&record.id).unwrap_or(claimed);
    render_detail_command(
        graft,
        PatchStatus::Ok,
        detail_view(state, session.0, &updated, &updated.title, ""),
    )
}

async fn continue_workflow(
    state: &AppState,
    session: RequiredSession,
    graft: PatchGraft,
    record: crate::conversations::ConversationRecord,
    revision: u32,
    stored: crate::conversations::ContinuationCheckpoint,
) -> AppResult<Response> {
    let Some(run_id) = stored.run else {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(
                state,
                session.0,
                &record,
                &record.title,
                "That continuation is no longer valid.",
            ),
        );
    };
    let execution = match state.workflow_execution.acquire() {
        Ok(execution) => execution,
        Err(error) => {
            return render_detail_command(
                graft,
                PatchStatus::Conflict,
                detail_view(state, session.0, &record, &record.title, error),
            );
        }
    };
    let Some(attempt_id) = stored.attempt else {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(
                state,
                session.0,
                &record,
                &record.title,
                "That continuation is no longer valid.",
            ),
        );
    };
    let job = match state.sessions.begin_conversation_job(&session.0, record.id) {
        Ok(job) => job,
        Err(_) => {
            return render_detail_command(
                graft,
                PatchStatus::Conflict,
                detail_view(
                    state,
                    session.0,
                    &record,
                    &record.title,
                    ConversationError::Active.message(),
                ),
            );
        }
    };
    let claimed =
        match state
            .conversations
            .claim_continuation(&record.id, revision, stored.id, job.id())
        {
            Ok(claimed) => claimed,
            Err(error) => {
                state
                    .sessions
                    .finish_conversation_job(&session.0, record.id, job.id());
                return render_detail_command(
                    graft,
                    status_for(error),
                    detail_view(state, session.0, &record, &record.title, error.message()),
                );
            }
        };
    if state
        .workflow_runs
        .mutate(&run_id, |run| {
            if run.conversation_id != Some(record.id) || run.pending_handoff.is_some() {
                return Err(crate::workflows::run::TransitionError::Invalid);
            }
            run.resume_paused(attempt_id, stored.id, workflows::now_ms())?;
            state
                .access_consent
                .approve_handoff(run.id, session.0, record.id, run.handoff_settings())
                .map_err(|_| crate::workflows::run::TransitionError::Invalid)
        })
        .is_err()
    {
        let _ = state.conversations.settle_message(
            &claimed.id,
            job.id(),
            String::new(),
            crate::conversations::MessageStatus::Failed,
            Some("That continuation is no longer valid.".to_owned()),
        );
        state
            .sessions
            .finish_conversation_job(&session.0, record.id, job.id());
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(
                state,
                session.0,
                &record,
                &record.title,
                "That continuation is no longer valid.",
            ),
        );
    }
    spawn_conversation_work(
        state.clone(),
        session.0,
        claimed.id,
        resume_workflow_job(state.clone(), session.0, claimed, job, run_id, execution),
    );
    let updated = state.conversations.get(&record.id).unwrap_or(record);
    render_detail_command(
        graft,
        PatchStatus::Ok,
        detail_view(state, session.0, &updated, &updated.title, ""),
    )
}

async fn resume_workflow_job(
    state: AppState,
    session: crate::sessions::SessionId,
    record: crate::conversations::ConversationRecord,
    job: std::sync::Arc<crate::sessions::Job>,
    run_id: crate::workflows::RunId,
    execution: crate::workflows::ExecutionGuard,
) {
    let Some(run) = state.workflow_runs.get(&run_id) else {
        let _ = state.conversations.settle_message(
            &record.id,
            job.id(),
            String::new(),
            crate::conversations::MessageStatus::Failed,
            Some("That continuation is no longer valid.".to_owned()),
        );
        job.finish(
            crate::sessions::JobStatus::Failed,
            Some("That continuation is no longer valid."),
        );
        state
            .sessions
            .finish_conversation_job(&session, record.id, job.id());
        return;
    };
    let Some(connection) = record
        .model
        .as_ref()
        .and_then(|model| state.vault.connection_for(&model.settings.model))
    else {
        drop(execution);
        let _ = state.conversations.settle_message(
            &record.id,
            job.id(),
            String::new(),
            crate::conversations::MessageStatus::Failed,
            Some("Choose a stored provider.".to_owned()),
        );
        job.finish(
            crate::sessions::JobStatus::Failed,
            Some("Choose a stored provider."),
        );
        state
            .sessions
            .finish_conversation_job(&session, record.id, job.id());
        return;
    };
    let turns = match crate::slices::conversations::job::history_with_review(&state, &record, None)
    {
        Ok(turns) => turns,
        Err(error) => {
            let _ = state
                .workflow_runs
                .mutate(&run_id, |run| run.cancel(workflows::now_ms()));
            let _ = state.conversations.settle_message(
                &record.id,
                job.id(),
                String::new(),
                crate::conversations::MessageStatus::Failed,
                Some(error.to_owned()),
            );
            job.finish(crate::sessions::JobStatus::Failed, Some(error));
            state
                .sessions
                .finish_conversation_job(&session, record.id, job.id());
            return;
        }
    };
    let workflow = crate::workflows::WorkflowJob {
        run_id,
        session_id: session,
        agent_id: run.agent_id,
        agent_revision: record.revision,
        conversation_id: Some(record.id),
        authority: None,
        project_free_authority: run.directory_settings().and_then(|settings| {
            crate::execution::ProjectFreeAuthority::from_settings(record.revision, &settings).ok()
        }),
        connection,
        phase_providers: run
            .model_phases()
            .map(|phase| phase.selection.provider)
            .collect(),
        active_connection: std::sync::Arc::new(std::sync::Mutex::new(None)),
        host_policy: crate::agents::DirectoryPolicy::from_grants(Vec::new(), String::new()),
        turns,
        job,
        eligible_reply: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
    };
    crate::workflows::execute_run(state, workflow, None, execution).await;
}

fn status_for(error: ConversationError) -> PatchStatus {
    match error {
        ConversationError::Conflict | ConversationError::Active => PatchStatus::Conflict,
        ConversationError::Unsettled => PatchStatus::Conflict,
        _ => PatchStatus::UnprocessableEntity,
    }
}
