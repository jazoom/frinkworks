mod forms;
mod page;

#[cfg(test)]
pub(crate) mod tests;

use axum::{
    Form, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
    routing::{get, post},
};
use hypergraft::{PageGraft, PatchGraft, PatchStatus};

use crate::{
    error::AppResult,
    responses,
    sessions::{JobStatus, RequiredSession, SessionId},
    state::AppState,
    workflows::{GateId, RunId, RunKind, settle_cancelled_job},
};

#[derive(serde::Deserialize)]
struct RawQuery {
    page: Option<String>,
    change: Option<String>,
    line: Option<String>,
}

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
        .route(
            "/runs/{run_id}/gates/{gate_id}/objects/{side}/{change}",
            get(object),
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
    Query(raw): Query<RawQuery>,
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
    let Some(query) = forms::DiffQuery::parse(
        raw.page.as_deref(),
        raw.change.as_deref(),
        raw.line.as_deref(),
    ) else {
        return static_error(
            PatchStatus::UnprocessableEntity,
            graft,
            &state,
            "That diff page is not valid.",
        );
    };
    let (diff, plan_text) =
        if gate.candidate.kind == crate::workflows::definition::ArtefactKind::Plan {
            let Some(plan) = load_gate_plan(&run, gate, &state.workflow_artefacts) else {
                return static_error(
                    PatchStatus::UnprocessableEntity,
                    graft,
                    &state,
                    "The immutable plan is unavailable.",
                );
            };
            (None, Some(plan))
        } else {
            let Ok(diff) = crate::workflows::artefacts::CandidateDiff::load(
                &run,
                &gate.diff_base,
                &gate.candidate,
                &state.workflow_artefacts,
            ) else {
                return static_error(
                    PatchStatus::UnprocessableEntity,
                    graft,
                    &state,
                    "The immutable candidate diff is unavailable.",
                );
            };
            (Some(diff), None)
        };
    let Some(mut view) = page::GatePage::new(
        &run,
        gate,
        diff,
        plan_text,
        &state.workflow_artefacts,
        query,
        "",
        application_destination(&state, &run),
    ) else {
        return static_error(
            PatchStatus::UnprocessableEntity,
            graft,
            &state,
            "That diff page is not valid.",
        );
    };
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
            "That candidate is unavailable.",
            "conversation-settings",
        );
    };
    let Some(conversation_id) = run.conversation_id else {
        return command_error_target(
            graft,
            PatchStatus::Conflict,
            "That candidate does not belong to this conversation.",
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
            "That candidate is unavailable.",
            "conversation-settings",
        );
    };
    let target = run
        .artefact(&gate.candidate.id)
        .and_then(crate::workflows::artefacts::ArtefactRecord::candidate_hash)
        .map(|hash| hash.as_str());
    if conversation.revision != form.conversation_revision
        || run.decision_revision(gate) != form.revision
        || gate.state != crate::workflows::gates::HumanGateState::AwaitingDecision
        || target.as_deref() != Some(form.candidate.as_str())
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
            "That candidate is unavailable.",
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
            "The changes were discarded, but Power Plant could not save the new execution settings.",
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
    if state
        .conversations
        .update_execution_settings(&conversation_id, settled.revision, settings)
        .is_err()
    {
        return command_error_target(
            graft,
            PatchStatus::Conflict,
            "The changes were discarded, but Power Plant could not save the new execution settings.",
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

pub(in crate::slices) fn application_destination(
    state: &AppState,
    run: &crate::workflows::WorkflowRun,
) -> String {
    let conversation = run
        .conversation_id
        .and_then(|id| state.conversations.get(&id));
    run.directory_settings()
        .or_else(|| {
            conversation
                .as_ref()?
                .model
                .as_ref()
                .map(|model| model.settings.clone())
        })
        .map(|settings| {
            settings
                .directories
                .iter()
                .filter(|grant| {
                    grant.access == crate::execution::DirectoryAccess::ReviewBeforeApply
                })
                .map(|grant| format!("{} ({})", grant.alias, grant.host_path.display()))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|destination| !destination.is_empty())
        .unwrap_or_default()
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
    let plan_gate = gate.candidate.kind == crate::workflows::definition::ArtefactKind::Plan;
    let target = if plan_gate {
        Some(gate.candidate.artefact_hash.as_str())
    } else {
        run.artefact(&gate.candidate.id)
            .and_then(crate::workflows::artefacts::ArtefactRecord::candidate_hash)
            .map(|hash| hash.as_str())
    };
    let submitted_target = if plan_gate {
        form.plan.as_str()
    } else {
        form.candidate.as_str()
    };
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
    let diff = if plan_gate {
        if !matches!(action, DecisionAction::Cancel)
            && load_gate_plan(&run, gate, &state.workflow_artefacts).is_none()
        {
            return command_error_for_run(
                graft,
                PatchStatus::Conflict,
                "The immutable plan is unavailable.",
                &run,
                form.conversation_surface,
            );
        }
        None
    } else {
        let Ok(diff) = crate::workflows::artefacts::CandidateDiff::load(
            &run,
            &gate.diff_base,
            &gate.candidate,
            &state.workflow_artefacts,
        ) else {
            return command_error_for_run(
                graft,
                PatchStatus::Conflict,
                "The immutable candidate diff is unavailable.",
                &run,
                form.conversation_surface,
            );
        };
        Some(diff)
    };

    if matches!(action, DecisionAction::Revision) {
        let valid_route = run.human_revision_policy(&gate.step).is_some();
        if run.kind == RunKind::QuickTask && run.conversation_id.is_none() || !valid_route {
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
        if run.kind == RunKind::QuickTask {
            settle_cancelled_job(&state, &continuation);
        } else {
            if let Some(key) = continuation.conversation_key() {
                let _ =
                    state
                        .sessions
                        .fail_turn(&session, &key, &continuation.job.id(), String::new());
                continuation.job.finish(JobStatus::Cancelled, None);
            } else {
                settle_cancelled_job(&state, &continuation);
            }
        }
        return Ok(responses::command_navigation(&destination));
    }

    if plan_gate {
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
                        "Power Plant could not prepare the plan revision. Try again.",
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
                "Power Plant could not store the plan decision. Try again.",
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
                tokio::spawn(crate::workflows::execute_run(
                    state.clone(),
                    continuation,
                    agent,
                    execution,
                ));
            }
        }
        return Ok(responses::command_navigation(&destination));
    }

    let kind = if matches!(action, DecisionAction::Approve) {
        crate::workflows::gates::HumanDecisionKind::Approved
    } else {
        crate::workflows::gates::HumanDecisionKind::RevisionRequested
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
                    "Power Plant could not prepare the revision. Try again.",
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
    let diff = diff.expect("candidate gate diff");
    let encoded = crate::workflows::artefacts::encode_human_decision(
        diff.target,
        diff.base,
        kind,
        form.note.as_deref(),
        decided_at,
        secret,
    );
    let Ok((bytes, object_hash, artefact_hash)) = encoded else {
        state.gate_continuations.put_back(continuation);
        return command_error_for_run(
            graft,
            PatchStatus::UnprocessableEntity,
            "That revision note is not valid.",
            &run,
            form.conversation_surface,
        );
    };
    if state.workflow_artefacts.publish(&bytes) != Ok(object_hash) {
        state.gate_continuations.put_back(continuation);
        return command_error_for_run(
            graft,
            PatchStatus::Conflict,
            "Power Plant could not store the decision. Try again.",
            &run,
            form.conversation_surface,
        );
    }
    let Some(record) = decision_record(
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
            "That gate is unavailable.",
            &run,
            form.conversation_surface,
        );
    };
    let changed = state.workflow_runs.mutate(&run_id, |run| {
        run.decide_gate(
            gate_id,
            form.revision,
            record,
            kind,
            if matches!(
                kind,
                crate::workflows::gates::HumanDecisionKind::RevisionRequested
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
            "That gate page is stale. Reload it.",
            &run,
            form.conversation_surface,
        );
    };

    if let Some((agent, execution)) = leases {
        if changed.is_terminal() {
            crate::workflows::settle_terminal_job(&state, &continuation, &changed);
        } else {
            continuation.job.resume();
            tokio::spawn(crate::workflows::execute_run(
                state.clone(),
                continuation,
                agent,
                execution,
            ));
        }
    }
    Ok(responses::command_navigation(&destination))
}

fn decision_record(
    run: &crate::workflows::WorkflowRun,
    gate: &crate::workflows::gates::HumanGateRecord,
    decision: crate::workflows::gates::HumanDecisionKind,
    at: u64,
    object_hash: crate::workflows::artefacts::ObjectHash,
    artefact_hash: crate::workflows::artefacts::ArtefactHash,
    bytes: u64,
) -> Option<crate::workflows::artefacts::ArtefactRecord> {
    let step = run.pinned.definition.step(&gate.step)?;
    let mut inputs = Vec::new();
    for input in &step.inputs {
        let reference = match &input.source {
            crate::workflows::definition::ArtefactSource::RunInitialCandidate => {
                match &run.source {
                    crate::workflows::RunSource::Captured { source } => source.initial.clone(),
                    _ => return None,
                }
            }
            crate::workflows::definition::ArtefactSource::RunCurrentCandidate => {
                match &run.source {
                    crate::workflows::RunSource::Captured { source } => source.accepted.clone(),
                    _ => return None,
                }
            }
            crate::workflows::definition::ArtefactSource::RunCurrentPlan => run.current_plan()?,

            crate::workflows::definition::ArtefactSource::StepOutput { step, output } => run
                .attempts
                .iter()
                .rev()
                .find(|attempt| attempt.step == *step)
                .and_then(|attempt| attempt.outputs.iter().find(|item| item.key == *output))
                .map(|item| item.artefact.clone())
                .or_else(|| {
                    run.gates
                        .iter()
                        .rev()
                        .find(|item| item.step == *step && item.output == *output)
                        .and_then(|item| item.decision.clone())
                })?,
        };
        if !inputs.contains(&reference) {
            inputs.push(reference);
        }
    }
    if !inputs.contains(&gate.diff_base) {
        inputs.push(gate.diff_base.clone());
    }
    Some(crate::workflows::artefacts::ArtefactRecord {
        id: crate::workflows::ArtefactId::generate().ok()?,
        kind: crate::workflows::definition::ArtefactKind::HumanDecision,
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
            inputs,
        },
        summary: crate::workflows::artefacts::ArtefactSummary::HumanDecision {
            candidate: run.artefact(&gate.candidate.id)?.candidate_hash()?,
            diff_base: run.artefact(&gate.diff_base.id)?.candidate_hash()?,
            decision,
        },
    })
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
        || gate.diff_base != gate.candidate
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
    match (run.kind, run.conversation_id) {
        (RunKind::QuickTask, Some(conversation)) => {
            format!("/conversations/{}", conversation.as_hex())
        }
        (RunKind::QuickTask, None) => "/runs".to_owned(),
        (RunKind::Configured, _) => format!("/runs/{}", run.id.as_hex()),
    }
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
        if settings
            .directories
            .iter()
            .any(|grant| grant.access != crate::execution::DirectoryAccess::ReadOnly)
            && !source_is_unchanged(state, run, &settings)
        {
            return ContinuationAuthority::Stale;
        }
        for grant in settings.directories.iter().filter(|grant| {
            grant.access == crate::execution::DirectoryAccess::ReviewBeforeApply
                || crate::execution::authority::sensitive_directory(
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
                            root.identity == grant.identity && root.access == grant.access
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

// A writable continuation must match the exact source it captured. The initial
// candidate hash covers every pinned root, so a later host edit invalidates it.
fn source_is_unchanged(
    state: &AppState,
    run: &crate::workflows::WorkflowRun,
    settings: &crate::execution::ExecutionSettings,
) -> bool {
    let crate::workflows::RunSource::Captured { source } = &run.source else {
        return false;
    };
    let Some(initial) = run
        .artefact(&source.initial.id)
        .and_then(crate::workflows::artefacts::ArtefactRecord::candidate_hash)
    else {
        return false;
    };
    crate::workflows::artefacts::CandidateCapture::capture_set(
        &settings.directories,
        state.local_data.root(),
        &state.workflow_artefacts,
    )
    .is_ok_and(|current| current.candidate_hash == initial)
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

async fn object(
    State(state): State<AppState>,
    _session: RequiredSession,
    Path((run_raw, gate_raw, side, change)): Path<(String, String, String, String)>,
    headers: HeaderMap,
) -> Response {
    if headers.contains_key(header::RANGE)
        || headers.contains_key(header::IF_MATCH)
        || headers.contains_key(header::IF_NONE_MATCH)
        || headers.contains_key(header::IF_MODIFIED_SINCE)
        || headers.contains_key(header::IF_UNMODIFIED_SINCE)
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some((run_id, gate_id)) = ids(&run_raw, &gate_raw) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(index) = change
        .parse::<usize>()
        .ok()
        .filter(|_| !change.starts_with('+') && !change.starts_with('-'))
    else {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    };
    let Some(run) = state.workflow_runs.get(&run_id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(gate) = run.gates.iter().find(|item| item.id == gate_id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(diff) = crate::workflows::artefacts::CandidateDiff::load(
        &run,
        &gate.diff_base,
        &gate.candidate,
        &state.workflow_artefacts,
    ) else {
        return StatusCode::CONFLICT.into_response();
    };
    let Ok((filename, bytes)) = diff.object(index, &side, &state.workflow_artefacts) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let filename: String = filename
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(120)
        .collect();
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

use axum::response::IntoResponse;
