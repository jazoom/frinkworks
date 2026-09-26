mod forms;
mod page;

#[cfg(test)]
pub(crate) mod tests;

use axum::{
    Form, Router,
    extract::{Path, State},
    response::Response,
    routing::{get, post},
};
use hypergraft::{PageGraft, PatchGraft, PatchStatus};

use crate::{
    error::AppResult,
    responses,
    sessions::{JobStatus, RequiredSession, SessionId},
    state::AppState,
    workflows::{GateId, RunId, settle_cancelled_job},
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/runs/{run_id}/gates/{gate_id}", get(detail))
        .route("/runs/{run_id}/gates/{gate_id}/approve", post(approve))
        .route(
            "/runs/{run_id}/gates/{gate_id}/request-revision",
            post(request_revision),
        )
        .route("/runs/{run_id}/gates/{gate_id}/cancel", post(cancel))
        .route(
            "/runs/{run_id}/gates/{gate_id}/discard-and-switch",
            post(discard_and_switch),
        )
        .layer(axum::extract::DefaultBodyLimit::max(70 * 1024))
}

fn ids(run: &str, gate: &str) -> Option<(RunId, GateId)> {
    Some((RunId::parse(run)?, GateId::parse(gate)?))
}

async fn detail(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PageGraft,
    Path((run_id, gate_id)): Path<(String, String)>,
) -> AppResult<Response> {
    let Some((run_id, gate_id)) = ids(&run_id, &gate_id) else {
        return Ok(responses::request_navigation(graft, "/runs"));
    };
    let Some(run) = state.workflow_runs.get(&run_id) else {
        return Ok(responses::request_navigation(graft, "/runs"));
    };
    let Some(gate) = run.gates.iter().find(|gate| gate.id == gate_id) else {
        return Ok(responses::request_navigation(
            graft,
            &format!("/runs/{}", run.id.as_hex()),
        ));
    };
    let Some(plan) = load_gate_plan(&run, gate, &state.workflow_artefacts) else {
        return static_error(
            PatchStatus::UnprocessableEntity,
            graft,
            &state,
            "The plan is unavailable.",
        );
    };
    let mut view = page::GatePage::new(&run, gate, plan);
    if run.recoverable_gate() && !state.gate_continuations.available(&run.id, &session.0) {
        view.needs_recovery = true;
        view.awaiting = false;
    }
    match graft {
        PageGraft::Document => {
            let mut response = responses::chat_page_response(page::TITLE, &state, &view)?;
            responses::apply_patch_status(&mut response, PatchStatus::Ok);
            Ok(response)
        }
        PageGraft::Navigation => Ok(hypergraft::outcome::page_patch(
            page::TITLE,
            "chat-main",
            &view,
        )?),
    }
}

fn static_error(
    status: PatchStatus,
    graft: PageGraft,
    state: &AppState,
    message: &'static str,
) -> AppResult<Response> {
    #[derive(askama::Template)]
    #[template(
        source = "<main data-section=\"runs\" class=\"mx-auto max-w-4xl p-8\"><div role=\"alert\" class=\"alert alert-error\">{{ message }}</div><a href=\"/runs\" data-graft class=\"btn btn-ghost mt-4\">Runs</a></main>",
        ext = "html"
    )]
    struct ErrorView {
        message: &'static str,
    }
    let view = ErrorView { message };
    match graft {
        PageGraft::Document => {
            let mut response = responses::chat_page_response(page::TITLE, state, &view)?;
            responses::apply_patch_status(&mut response, status);
            Ok(response)
        }
        PageGraft::Navigation => Ok(hypergraft::outcome::page_patch(
            page::TITLE,
            "chat-main",
            &view,
        )?),
    }
}

async fn approve(
    State(state): State<AppState>,
    RequiredSession(session): RequiredSession,
    graft: PatchGraft,
    Path((run_id, gate_id)): Path<(String, String)>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> AppResult<Response> {
    decide(
        state,
        session,
        graft,
        run_id,
        gate_id,
        pairs,
        DecisionAction::Approve,
    )
    .await
}

async fn request_revision(
    State(state): State<AppState>,
    RequiredSession(session): RequiredSession,
    graft: PatchGraft,
    Path((run_id, gate_id)): Path<(String, String)>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> AppResult<Response> {
    decide(
        state,
        session,
        graft,
        run_id,
        gate_id,
        pairs,
        DecisionAction::Revision,
    )
    .await
}

async fn cancel(
    State(state): State<AppState>,
    RequiredSession(session): RequiredSession,
    graft: PatchGraft,
    Path((run_id, gate_id)): Path<(String, String)>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> AppResult<Response> {
    decide(
        state,
        session,
        graft,
        run_id,
        gate_id,
        pairs,
        DecisionAction::Cancel,
    )
    .await
}

async fn discard_and_switch(
    State(state): State<AppState>,
    RequiredSession(session): RequiredSession,
    graft: PatchGraft,
    Path((run_raw, gate_raw)): Path<(String, String)>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> AppResult<Response> {
    let Some((run_id, gate_id)) = ids(&run_raw, &gate_raw) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let mut form = match forms::EnvironmentSwitchDecisionForm::parse(pairs) {
        Ok(form) => form,
        Err(_) => {
            return command_error_target(
                graft,
                PatchStatus::Conflict,
                "That environment switch is stale. Reload the conversation.",
                "conversation-settings",
            );
        }
    };
    let Some(run) = state.workflow_runs.get(&run_id) else {
        return command_error_target(
            graft,
            PatchStatus::Conflict,
            "That plan is unavailable.",
            "conversation-settings",
        );
    };
    let Some(conversation_id) = run.conversation_id else {
        return command_error_target(
            graft,
            PatchStatus::Conflict,
            "That plan does not belong to this conversation.",
            "conversation-settings",
        );
    };
    let Some(conversation) = state.conversations.get(&conversation_id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let Some(gate) = run.gates.iter().find(|gate| gate.id == gate_id) else {
        return command_error_target(
            graft,
            PatchStatus::Conflict,
            "That plan is unavailable.",
            "conversation-settings",
        );
    };
    let target = Some(gate.candidate.artefact_hash.as_str());
    if conversation.revision != form.conversation_revision
        || run.decision_revision(gate) != form.revision
        || gate.state != crate::workflows::gates::HumanGateState::AwaitingDecision
        || target.as_deref() != Some(form.plan.as_str())
        || !form.conversation_surface
        || !state.gate_continuations.available(&run_id, &session)
    {
        return command_error_target(
            graft,
            PatchStatus::Conflict,
            "That environment switch is stale. Reload the conversation.",
            "conversation-settings",
        );
    }
    form.revision = gate.revision;
    let Some(current_settings) = conversation.model.as_ref() else {
        return command_error_target(
            graft,
            PatchStatus::UnprocessableEntity,
            "Choose a model first.",
            "conversation-settings",
        );
    };
    if let Err(error) = crate::slices::conversations::settings::replacement_directory_access(
        &current_settings.settings,
        &form.directory_access,
    ) {
        return command_error_target(
            graft,
            PatchStatus::UnprocessableEntity,
            error,
            "conversation-settings",
        );
    }
    let network = match crate::slices::conversations::settings::replacement_network(
        &current_settings.settings.network,
        &form.network,
        &form.network_domains,
    ) {
        Ok(network) => network,
        Err(error) => {
            return command_error_target(
                graft,
                PatchStatus::UnprocessableEntity,
                error,
                "conversation-settings",
            );
        }
    };
    let location = form.location.unwrap_or(current_settings.settings.location);
    let host_approval = form
        .host_approval
        .unwrap_or(current_settings.settings.host_approval);
    if let Err(error) = crate::slices::conversations::settings::replacement_execution_ready(
        &state,
        &conversation,
        location,
        form.environment,
    )
    .await
    {
        return command_error_target(
            graft,
            PatchStatus::UnprocessableEntity,
            error,
            "conversation-settings",
        );
    }
    let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
        return command_error_target(
            graft,
            PatchStatus::Conflict,
            crate::local_data::HOST_PATH_RESET_PENDING,
            "conversation-settings",
        );
    };
    let Some(continuation) = state.gate_continuations.take(&run_id) else {
        return command_error_target(
            graft,
            PatchStatus::Conflict,
            "That plan is unavailable.",
            "conversation-settings",
        );
    };
    if continuation.session_id != session
        || continuation.conversation_id != Some(conversation_id)
        || !state
            .conversations
            .get(&conversation_id)
            .is_some_and(|current| {
                current.revision == form.conversation_revision
                    && current.active_job == Some(continuation.job.id())
            })
        || !state
            .sessions
            .owns_conversation_job(&session, conversation_id, continuation.job.id())
    {
        state.gate_continuations.put_back(continuation);
        return command_error_target(
            graft,
            PatchStatus::Conflict,
            "That conversation task is no longer available. Reload the conversation.",
            "conversation-settings",
        );
    }
    if state
        .workflow_runs
        .mutate(&run_id, |run| {
            run.cancel_gate(gate_id, form.revision, crate::workflows::now_ms())
        })
        .is_err()
    {
        state.gate_continuations.put_back(continuation);
        return command_error_target(
            graft,
            PatchStatus::Conflict,
            "That environment switch is stale. Reload the conversation.",
            "conversation-settings",
        );
    }
    settle_cancelled_job(&state, &continuation);
    let Some(settled) = state.conversations.get(&conversation_id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    if settled.model != conversation.model || settled.active_job.is_some() {
        return command_error_target(
            graft,
            PatchStatus::Conflict,
            "The conversation changed. Execution settings did not change.",
            "conversation-settings",
        );
    }
    if let Err(error) = crate::slices::conversations::settings::replacement_execution_ready(
        &state,
        &settled,
        location,
        form.environment,
    )
    .await
    {
        return command_error_target(graft, PatchStatus::Conflict, error, "conversation-settings");
    }
    let Some(model) = settled.model.as_ref() else {
        return command_error_target(
            graft,
            PatchStatus::Conflict,
            "The changes were discarded, but Frinkworks could not save the new execution settings.",
            "conversation-settings",
        );
    };
    let replacement = match crate::slices::conversations::settings::replacement_directory_access(
        &model.settings,
        &form.directory_access,
    ) {
        Ok(settings) => settings,
        Err(error) => {
            return command_error_target(
                graft,
                PatchStatus::Conflict,
                error,
                "conversation-settings",
            );
        }
    };
    let mut settings = replacement
        .with_location(location)
        .with_host_approval(host_approval);
    settings.environment = form.environment;
    settings.network = network;
    if state
        .conversations
        .update_execution_settings(&conversation_id, settled.revision, settings)
        .is_err()
    {
        return command_error_target(
            graft,
            PatchStatus::Conflict,
            "The changes were discarded, but Frinkworks could not save the new execution settings.",
            "conversation-settings",
        );
    }
    state
        .access_consent
        .invalidate_conversation(conversation_id);
    state
        .host_approvals
        .invalidate_conversation(conversation_id);
    Ok(responses::command_navigation(&format!(
        "/conversations/{}",
        conversation_id.as_hex()
    )))
}

#[derive(Clone, Copy)]
enum DecisionAction {
    Approve,
    Revision,
    Cancel,
}

async fn decide(
    state: AppState,
    session: SessionId,
    graft: PatchGraft,
    run_raw: String,
    gate_raw: String,
    pairs: Vec<(String, String)>,
    action: DecisionAction,
) -> AppResult<Response> {
    let Some((run_id, gate_id)) = ids(&run_raw, &gate_raw) else {
        return Ok(responses::request_navigation(graft, "/runs"));
    };
    let error_target = if pairs
        .iter()
        .any(|(key, value)| key == "surface" && value == "conversation")
    {
        "conversation-candidate"
    } else {
        "gate-detail"
    };
    let mut form =
        match forms::DecisionForm::parse(pairs, matches!(action, DecisionAction::Revision)) {
            Ok(form) => form,
            Err(forms::FormError::Note) => {
                return command_error_target(
                    graft,
                    PatchStatus::UnprocessableEntity,
                    "Enter a revision note.",
                    error_target,
                );
            }
            Err(forms::FormError::Invalid) => {
                return command_error_target(
                    graft,
                    PatchStatus::Conflict,
                    "That gate page is stale. Reload it.",
                    error_target,
                );
            }
        };
    let Some(run) = state.workflow_runs.get(&run_id) else {
        return command_error_target(
            graft,
            PatchStatus::Conflict,
            "That gate is unavailable.",
            error_target,
        );
    };
    let Some(gate) = run.gates.iter().find(|gate| gate.id == gate_id) else {
        return command_error_target(
            graft,
            PatchStatus::Conflict,
            "That gate is unavailable.",
            error_target,
        );
    };
    let target = Some(gate.candidate.artefact_hash.as_str());
    let submitted_target = form.plan.as_str();
    if gate.state != crate::workflows::gates::HumanGateState::AwaitingDecision
        || run.decision_revision(gate) != form.revision
        || target.as_deref() != Some(submitted_target)
        || !state.gate_continuations.available(&run_id, &session)
    {
        return command_error_for_run(
            graft,
            PatchStatus::Conflict,
            "That gate page is stale. Reload it.",
            &run,
            form.conversation_surface,
        );
    }
    form.revision = gate.revision;
    if matches!(action, DecisionAction::Revision) {
        let valid_route = run.human_revision_policy(&gate.step).is_some();
        if !valid_route {
            return command_error_for_run(
                graft,
                PatchStatus::Conflict,
                "That gate has no available revision route.",
                &run,
                form.conversation_surface,
            );
        }
    }
    let destination = decision_destination(&run);
    let Some(continuation) = state.gate_continuations.take(&run_id) else {
        return command_error_for_run(
            graft,
            PatchStatus::Conflict,
            "That gate is unavailable.",
            &run,
            form.conversation_surface,
        );
    };
    if continuation.session_id != session {
        state.gate_continuations.put_back(continuation);
        return command_error_for_run(
            graft,
            PatchStatus::Conflict,
            "That gate is unavailable.",
            &run,
            form.conversation_surface,
        );
    }
    if let Some(conversation_id) = continuation.conversation_id
        && !state
            .sessions
            .owns_conversation_job(&session, conversation_id, continuation.job.id())
    {
        state.gate_continuations.put_back(continuation);
        return command_error_for_run(
            graft,
            PatchStatus::Conflict,
            "That conversation task is no longer available. Reload the conversation.",
            &run,
            form.conversation_surface,
        );
    }
    let response_state = state.clone();
    let leases = if matches!(action, DecisionAction::Approve | DecisionAction::Revision) {
        let execution = match state.workflow_execution.acquire() {
            Ok(execution) => execution,
            Err(error) => {
                state.gate_continuations.put_back(continuation);
                return command_error_for_run(
                    graft,
                    PatchStatus::Conflict,
                    error,
                    &run,
                    form.conversation_surface,
                );
            }
        };
        let agent = if run.conversation_id.is_some() {
            None
        } else {
            match state
                .agent_leases
                .acquire(run.agent_id.expect("catalogue-backed run agent"))
            {
                Ok(agent) => Some(agent),
                Err(()) => {
                    state.gate_continuations.put_back(continuation);
                    return command_error_for_run(
                        graft,
                        PatchStatus::Conflict,
                        "That agent is active. Try again.",
                        &run,
                        form.conversation_surface,
                    );
                }
            }
        };
        Some((agent, execution))
    } else {
        None
    };
    if matches!(action, DecisionAction::Approve | DecisionAction::Revision) {
        match continuation_authority(&state, &run, &continuation) {
            ContinuationAuthority::Ready => {}
            ContinuationAuthority::Unavailable => {
                state.gate_continuations.put_back(continuation);
                return command_error_for_run(
                    graft,
                    PatchStatus::Conflict,
                    "A granted directory is no longer at the saved path.",
                    &run,
                    form.conversation_surface,
                );
            }
            ContinuationAuthority::Stale => {
                return interrupt_and_redirect(
                    state,
                    continuation,
                    run_id,
                    graft,
                    &destination,
                    form.conversation_surface,
                );
            }
        }
    }

    if matches!(action, DecisionAction::Cancel) {
        let result = state.workflow_runs.mutate(&run_id, |run| {
            run.cancel_gate(gate_id, form.revision, crate::workflows::now_ms())
        });
        if result.is_err() {
            state.gate_continuations.put_back(continuation);
            return command_error_for_run(
                graft,
                PatchStatus::Conflict,
                "That gate page is stale. Reload it.",
                &run,
                form.conversation_surface,
            );
        }
        settle_cancelled_job(&state, &continuation);
        return decision_response(
            &response_state,
            session,
            graft,
            &run,
            form.conversation_surface,
        );
    }

    {
        let kind = if matches!(action, DecisionAction::Approve) {
            crate::workflows::gates::PlanDecisionKind::Accepted
        } else {
            crate::workflows::gates::PlanDecisionKind::RevisionRequested
        };
        let reserved_attempt = if matches!(action, DecisionAction::Revision) {
            match crate::workflows::AttemptId::generate() {
                Ok(attempt) => Some(attempt),
                Err(_) => {
                    if let Some((agent, execution)) = leases {
                        drop(agent);
                        drop(execution);
                    }
                    state.gate_continuations.put_back(continuation);
                    return command_error_for_run(
                        graft,
                        PatchStatus::Conflict,
                        "Frinkworks could not prepare the plan revision. Try again.",
                        &run,
                        form.conversation_surface,
                    );
                }
            }
        } else {
            None
        };
        let connection = continuation
            .active_connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .unwrap_or_else(|| continuation.connection.clone());
        let secret = match connection.auth {
            crate::providers::AuthMethod::ApiKey => Some(connection.api_key.expose()),
            crate::providers::AuthMethod::Plan => None,
        };
        let decided_at = crate::workflows::now_ms();
        let Ok((bytes, object_hash, artefact_hash)) =
            crate::workflows::artefacts::encode_plan_decision(
                gate.candidate.artefact_hash,
                kind,
                form.note.as_deref(),
                decided_at,
                secret,
            )
        else {
            state.gate_continuations.put_back(continuation);
            return command_error_for_run(
                graft,
                PatchStatus::UnprocessableEntity,
                "That plan decision note is not valid.",
                &run,
                form.conversation_surface,
            );
        };
        if state.workflow_artefacts.publish(&bytes) != Ok(object_hash) {
            state.gate_continuations.put_back(continuation);
            return command_error_for_run(
                graft,
                PatchStatus::Conflict,
                "Frinkworks could not store the plan decision. Try again.",
                &run,
                form.conversation_surface,
            );
        }
        let Some(record) = plan_decision_record(
            &run,
            gate,
            kind,
            decided_at,
            object_hash,
            artefact_hash,
            bytes.len() as u64,
        ) else {
            state.gate_continuations.put_back(continuation);
            return command_error_for_run(
                graft,
                PatchStatus::Conflict,
                "That plan checkpoint is unavailable.",
                &run,
                form.conversation_surface,
            );
        };
        let changed = state.workflow_runs.mutate(&run_id, |run| {
            run.decide_plan_gate(
                gate_id,
                form.revision,
                record,
                kind,
                if matches!(
                    kind,
                    crate::workflows::gates::PlanDecisionKind::RevisionRequested
                ) {
                    form.note.clone()
                } else {
                    None
                },
                reserved_attempt,
                decided_at,
            )
        });
        let Ok(changed) = changed else {
            state.gate_continuations.put_back(continuation);
            return command_error_for_run(
                graft,
                PatchStatus::Conflict,
                "That checkpoint is stale. Reload it.",
                &run,
                form.conversation_surface,
            );
        };
        if let Some((agent, execution)) = leases {
            if changed.is_terminal() {
                crate::workflows::settle_terminal_job(&state, &continuation, &changed);
            } else {
                continuation.job.resume();
                let follow_session = continuation.session_id;
                let follow_conversation = continuation.conversation_id;
                tokio::spawn(async move {
                    crate::workflows::execute_run(state.clone(), continuation, agent, execution)
                        .await;
                    if let Some(conversation) = follow_conversation {
                        crate::slices::conversations::continue_follow_ups(
                            state,
                            follow_session,
                            conversation,
                        )
                        .await;
                    }
                });
            }
        }
        decision_response(
            &response_state,
            session,
            graft,
            &run,
            form.conversation_surface,
        )
    }
}

fn decision_response(
    state: &AppState,
    session: SessionId,
    graft: PatchGraft,
    run: &crate::workflows::WorkflowRun,
    conversation_surface: bool,
) -> AppResult<Response> {
    if conversation_surface && let Some(conversation) = run.conversation_id {
        return super::conversations::refresh_detail_after_decision(
            state,
            session,
            graft,
            conversation,
        );
    }
    Ok(responses::command_navigation(&decision_destination(run)))
}

fn load_gate_plan(
    run: &crate::workflows::WorkflowRun,
    gate: &crate::workflows::gates::HumanGateRecord,
    store: &crate::workflows::WorkflowArtefactRepository,
) -> Option<String> {
    let record = run.artefact(&gate.candidate.id)?;
    if record.kind != crate::workflows::definition::ArtefactKind::Plan
        || record.artefact_hash != gate.candidate.artefact_hash
        || record.provenance.run_id != run.id
    {
        return None;
    }
    let bytes = store.get(&record.object_hash).ok()?;
    let crate::workflows::artefacts::TypedPayload::Plan(plan) =
        crate::workflows::artefacts::parse_typed_payload(record.kind, &bytes).ok()?
    else {
        return None;
    };
    if crate::workflows::artefacts::ObjectHash::of(&bytes) != record.object_hash
        || crate::workflows::artefacts::artefact_hash_for(record.kind, plan.format_version, &bytes)
            != record.artefact_hash
    {
        return None;
    }
    Some(plan.markdown)
}

fn plan_decision_record(
    run: &crate::workflows::WorkflowRun,
    gate: &crate::workflows::gates::HumanGateRecord,
    decision: crate::workflows::gates::PlanDecisionKind,
    at: u64,
    object_hash: crate::workflows::artefacts::ObjectHash,
    artefact_hash: crate::workflows::artefacts::ArtefactHash,
    bytes: u64,
) -> Option<crate::workflows::artefacts::ArtefactRecord> {
    let step = run.pinned.definition.step(&gate.step)?;
    if !matches!(
        &step.action,
        crate::workflows::definition::StepAction::HumanGate(action)
            if action.is_plan_checkpoint()
    ) || gate.candidate.kind != crate::workflows::definition::ArtefactKind::Plan
    {
        return None;
    }
    Some(crate::workflows::artefacts::ArtefactRecord {
        id: crate::workflows::ArtefactId::generate().ok()?,
        kind: crate::workflows::definition::ArtefactKind::PlanDecision,
        artefact_hash,
        object_hash,
        payload_bytes: bytes,
        created_at_ms: at,
        provenance: crate::workflows::artefacts::ArtefactProvenance {
            run_id: run.id,
            producer: crate::workflows::artefacts::ArtefactProducer::HumanGate {
                gate_id: gate.id,
                step: gate.step.clone(),
                output: gate.output.clone(),
            },
            inputs: vec![gate.candidate.clone()],
        },
        summary: crate::workflows::artefacts::ArtefactSummary::PlanDecision {
            plan: gate.candidate.artefact_hash,
            decision,
        },
    })
}

fn command_error_for_run(
    graft: PatchGraft,
    status: PatchStatus,
    message: &'static str,
    run: &crate::workflows::WorkflowRun,
    conversation_surface: bool,
) -> AppResult<Response> {
    let target = if conversation_surface && run.conversation_id.is_some() {
        "conversation-candidate"
    } else {
        "gate-detail"
    };
    command_error_target(graft, status, message, target)
}

fn command_error_target(
    _graft: PatchGraft,
    status: PatchStatus,
    message: &'static str,
    target: &'static str,
) -> AppResult<Response> {
    #[derive(askama::Template)]
    #[template(
        source = "<div role=\"alert\" class=\"alert alert-error\"><span>{{ message }}</span></div>",
        ext = "html"
    )]
    struct ErrorView {
        message: &'static str,
    }
    let view = ErrorView { message };
    Ok(hypergraft::outcome::children_patch(status, target, &view)?)
}

fn decision_destination(run: &crate::workflows::WorkflowRun) -> String {
    format!("/runs/{}", run.id.as_hex())
}

enum ContinuationAuthority {
    Ready,
    Unavailable,
    Stale,
}

fn continuation_authority(
    state: &AppState,
    run: &crate::workflows::WorkflowRun,
    continuation: &crate::workflows::WorkflowJob,
) -> ContinuationAuthority {
    if continuation.run_id != run.id
        || continuation.agent_id != run.agent_id
        || continuation.conversation_id != run.conversation_id
        || run.pending_handoff.is_some()
    {
        return ContinuationAuthority::Stale;
    }
    if run.conversation_id.is_some() {
        let (Some(conversation_id), Some(pinned)) = (
            run.conversation_id,
            continuation.project_free_authority.as_ref(),
        ) else {
            return ContinuationAuthority::Stale;
        };
        let Some(record) = state.conversations.get(&conversation_id) else {
            return ContinuationAuthority::Stale;
        };
        let Some(settings) = run
            .directory_settings()
            .or_else(|| record.model.as_ref().map(|model| model.settings.clone()))
        else {
            return ContinuationAuthority::Stale;
        };
        match crate::execution::ProjectFreeAuthority::from_settings(pinned.revision, &settings) {
            Ok(authority) if authority == *pinned => {}
            Ok(_) => return ContinuationAuthority::Stale,
            Err(crate::execution::DirectoryGrantError::Unavailable) => {
                return ContinuationAuthority::Unavailable;
            }
            Err(_) => return ContinuationAuthority::Stale,
        }
        if !state.sessions.contains_live(&continuation.session_id) {
            return ContinuationAuthority::Stale;
        }
        for grant in settings.directories.iter().filter(|grant| {
            crate::execution::authority::sensitive_directory(
                &grant.host_path,
                state.local_data.root(),
            )
        }) {
            if !state.access_consent.authorised_conversation(
                continuation.session_id,
                conversation_id,
                &settings,
                grant,
            ) && !state
                .conversations
                .directory_approved(&conversation_id, &settings, grant)
                && !run.model_phases().any(|phase| {
                    phase.settings.as_ref().is_some_and(|phase_settings| {
                        phase_settings.directories.iter().any(|root| {
                            root.host_path == grant.host_path && root.access == grant.access
                        }) && state.access_consent.authorised_launch(
                            run.id,
                            continuation.session_id,
                            conversation_id,
                            phase_settings,
                        )
                    })
                })
            {
                return ContinuationAuthority::Stale;
            }
        }
        return ContinuationAuthority::Ready;
    }
    ContinuationAuthority::Stale
}

fn interrupt_and_redirect(
    state: AppState,
    continuation: crate::workflows::WorkflowJob,
    run_id: RunId,
    graft: PatchGraft,
    destination: &str,
    conversation_surface: bool,
) -> AppResult<Response> {
    if state
        .workflow_runs
        .mutate(&run_id, |run| run.interrupt(crate::workflows::now_ms()))
        .is_err()
    {
        let conversation = conversation_surface && continuation.conversation_id.is_some();
        state.gate_continuations.put_back(continuation);
        return command_error_target(
            graft,
            PatchStatus::Conflict,
            "That gate page is stale. Reload it.",
            if conversation {
                "conversation-candidate"
            } else {
                "gate-detail"
            },
        );
    }
    if let Some(key) = continuation.conversation_key() {
        let _ = state.sessions.fail_turn(
            &continuation.session_id,
            &key,
            &continuation.job.id(),
            String::new(),
        );
        continuation.job.finish(JobStatus::Cancelled, None);
    } else {
        settle_cancelled_job(&state, &continuation);
    }
    Ok(responses::command_navigation(destination))
}
