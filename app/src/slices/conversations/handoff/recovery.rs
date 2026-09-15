use std::sync::{Arc, Mutex};

use axum::{
    Form,
    extract::{Path, State},
    response::Response,
};
use hypergraft::{PatchGraft, PatchStatus};

use crate::{
    conversations::ConversationRecord,
    error::AppResult,
    sessions::{RequiredSession, SessionId},
    state::AppState,
    workflows::{WorkflowJob, WorkflowRun},
};

pub(in crate::slices::conversations) struct RecoveryView {
    pub(in crate::slices::conversations) run: String,
    pub(in crate::slices::conversations) fingerprint: String,
    pub(in crate::slices::conversations) settings: String,
}

impl RecoveryView {
    pub(in crate::slices::conversations) fn for_conversation(
        state: &AppState,
        session: SessionId,
        record: &ConversationRecord,
    ) -> Option<Self> {
        let run = state
            .workflow_runs
            .for_conversation(&record.id)
            .into_iter()
            .find(WorkflowRun::recoverable_gate)?;
        if state.gate_continuations.available(&run.id, &session) {
            return None;
        }
        Some(Self {
            run: run.id.as_hex(),
            fingerprint: run.handoff_fingerprint(),
            settings: super::transfer::settings_text(&run),
        })
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::slices::conversations) struct RecoveryForm {
    revision: u32,
    run: String,
    fingerprint: String,
    #[serde(default)]
    approval: String,
}

pub(in crate::slices::conversations) async fn restore(
    State(state): State<AppState>,
    RequiredSession(session): RequiredSession,
    graft: PatchGraft,
    Path(id): Path<String>,
    Form(form): Form<RecoveryForm>,
) -> AppResult<Response> {
    let Some(record) = super::super::load_conversation(&state, &id) else {
        return Ok(crate::responses::request_navigation(
            graft,
            "/conversations",
        ));
    };
    let reject = |message| {
        super::super::render_detail(
            &state,
            session,
            graft.into(),
            PatchStatus::Conflict,
            super::super::detail_view(&state, session, &record, &record.title, message),
        )
    };
    let Some(run) =
        crate::workflows::RunId::parse(&form.run).and_then(|id| state.workflow_runs.get(&id))
    else {
        return reject("The prepared changes are unavailable.");
    };
    if form.approval != "restore-prepared"
        || record.revision != form.revision
        || run.conversation_id != Some(record.id)
        || !run.recoverable_gate()
        || run.handoff_fingerprint() != form.fingerprint
    {
        return reject(
            "Reload the owner conversation and approve its pinned settings for this execution only.",
        );
    }
    if state.sessions.conversation_reserved(record.id) || state.gate_continuations.occupied(&run.id)
    {
        return reject("Another operation controls the prepared changes.");
    }
    let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
        return reject(crate::local_data::HOST_PATH_RESET_PENDING);
    };
    if let Err(error) = super::transfer::validate_pinned(&state, &run) {
        return reject(error);
    }
    if state
        .conversations
        .recover_handoffs(&state.workflow_runs)
        .is_err()
    {
        return reject(
            "Power Plant cannot finish ownership recovery. The prepared changes remain reserved.",
        );
    }
    let current = state
        .conversations
        .get(&record.id)
        .expect("recovered owner");
    let run = state.workflow_runs.get(&run.id).expect("recovered run");
    let Ok(job) = state.sessions.begin_conversation_job(
        &session,
        record.id,
        current.messages.len().saturating_sub(1),
    ) else {
        return reject("Another command is active in this browser session.");
    };
    let restored =
        match state
            .conversations
            .restore_prepared(&state.workflow_runs, &run, &current, job.id())
        {
            Ok(record) => record,
            Err(_) => {
                state
                    .sessions
                    .finish_conversation_job(&session, record.id, job.id());
                return reject("The owner changed before recovery. Reload the conversation.");
            }
        };
    if let Err(error) = attach(&state, session, &run, &restored, job.clone()) {
        job.finish(crate::sessions::JobStatus::Failed, Some(error));
        state
            .sessions
            .finish_conversation_job(&session, record.id, job.id());
        return reject(error);
    }
    Ok(crate::responses::command_navigation(&format!(
        "/conversations/{}",
        record.id.as_hex()
    )))
}

pub(super) fn attach(
    state: &AppState,
    session: SessionId,
    run: &WorkflowRun,
    record: &ConversationRecord,
    job: Arc<crate::sessions::Job>,
) -> Result<(), &'static str> {
    let project = run
        .project_authority
        .as_ref()
        .map(|snapshot| snapshot.resolve(state, run))
        .transpose()?;
    let private = if project.is_none() {
        let settings = run
            .directory_settings()
            .ok_or("The pinned settings are unavailable.")?;
        Some(
            crate::execution::ProjectFreeAuthority::from_settings(record.revision, &settings)
                .map_err(|_| "A pinned directory changed identity.")?,
        )
    } else {
        None
    };
    let policy = project
        .as_ref()
        .map(|authority| authority.policy.clone())
        .or_else(|| private.as_ref().map(|authority| authority.policy.clone()))
        .ok_or("The pinned authority is unavailable.")?;
    let connection = run
        .model_phases()
        .next()
        .and_then(|phase| state.vault.connection_for(&phase.selection))
        .ok_or("The selected provider is unavailable.")?;
    // This approval supplies runtime authority only for the pinned run, never future conversation messages.
    state
        .access_consent
        .approve_handoff(run.id, session, record.id, run.handoff_settings())?;
    job.set_workflow_name(run.pinned.definition.name().to_owned());
    job.set_step_label("Prepared changes await your decision".to_owned());
    if job.set_awaiting_decision().is_none() {
        return Err("The execution no longer awaits a decision.");
    }
    let continuation = WorkflowJob {
        run_id: run.id,
        session_id: session,
        project_id: run.project_id,
        agent_id: None,
        agent_revision: project
            .as_ref()
            .map_or(record.revision, |authority| authority.revision),
        conversation_id: Some(record.id),
        host_policy: policy,
        project_free_authority: private,
        grant_alias: project
            .as_ref()
            .map_or_else(String::new, |authority| authority.grant_alias.clone()),
        grant_access: project
            .as_ref()
            .map_or(crate::agents::AccessMode::ReadWrite, |authority| {
                authority.grant_access
            }),
        authority: project,
        connection,
        phase_providers: run
            .model_phases()
            .map(|phase| phase.selection.provider)
            .collect(),
        active_connection: Arc::new(Mutex::new(None)),
        turns: record
            .messages
            .iter()
            .rfind(|message| message.role == crate::conversations::MessageRole::User)
            .map(|message| vec![crate::providers::ChatTurn::user(message.text.clone())])
            .unwrap_or_default(),
        job: job.clone(),
        eligible_reply: Arc::new(Mutex::new(String::new())),
    };
    if !state.gate_continuations.insert(continuation) {
        return Err("Another operation controls the prepared changes.");
    }
    state
        .sessions
        .release_job_reservation(&session, Some(record.id), job.id());
    Ok(())
}
