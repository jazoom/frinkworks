use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::agents::{AccessMode, DirectoryPolicy, EffectiveAuthority, LeaseGuard, PolicyGrant};
use crate::conversations::ConversationId;

use crate::providers::{ChatTurn, ProviderConnection};
use crate::sandbox::{CommandEvent, GUEST_PROJECT, GuestExec, GuestSandbox};
use crate::sessions::{Job, JobStatus, SessionId};
use crate::slices::{AgentOutcome, AgentRunSpec};
use crate::state::AppState;

use super::definition::{
    AgentAuthority, AgentStep, CandidateAuthority, StepAction, StepDefinition, SystemCommandId,
};
use super::execution::ExecutionGuard;
use super::id::{AttemptId, RunId};
use super::run::{FailureCategory, now_ms};
use super::store::StoreError;

pub(crate) const OPERATIONAL_STORE_ERROR: &str =
    "Power Plant could not store the workflow run. Try again.";

const COMMAND_DEADLINE: Duration = if cfg!(test) {
    Duration::from_millis(50)
} else {
    Duration::from_secs(10)
};
const COMMAND_OUTPUT_LIMIT: usize = 64 * 1024;

pub(crate) struct WorkflowContinuationRegistry {
    inner: std::sync::Mutex<std::collections::BTreeMap<RunId, WorkflowJob>>,
    // Each unresolved operation retains its leases until startup reconciliation.
    recovery_protection: std::sync::Mutex<Vec<(Option<LeaseGuard>, ExecutionGuard)>>,
}

impl WorkflowContinuationRegistry {
    pub(crate) fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            recovery_protection: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn protect_apply_recovery(
        &self,
        job: &Job,
        agent: Option<LeaseGuard>,
        execution: ExecutionGuard,
    ) {
        let _ = job.finish(
            JobStatus::Failed,
            Some("Restart Power Plant to reconcile the uncertain file application. This operation retains its reservations."),
        );
        self.retain_recovery(agent, execution);
    }

    fn protect_commit_recovery(
        &self,
        job: &Job,
        agent: Option<LeaseGuard>,
        execution: ExecutionGuard,
    ) {
        let _ = job.finish(
            JobStatus::Failed,
            Some("Restart Power Plant to reconcile the uncertain Git commit. This operation retains its reservations."),
        );
        self.retain_recovery(agent, execution);
    }

    fn protect_cleanup_failure(
        &self,
        job: &Job,
        agent: Option<LeaseGuard>,
        execution: ExecutionGuard,
    ) {
        let _ = job.finish(
            JobStatus::Failed,
            Some("Power Plant could not clean up the sandbox. This operation retains its reservations."),
        );
        self.retain_recovery(agent, execution);
    }

    fn retain_recovery(&self, agent: Option<LeaseGuard>, execution: ExecutionGuard) {
        execution.require_recovery();
        self.recovery_protection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((agent, execution));
    }

    pub(crate) fn insert(&self, job: WorkflowJob) -> bool {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if inner.contains_key(&job.run_id) {
            return false;
        }
        inner.insert(job.run_id, job);
        true
    }

    pub(crate) fn take(&self, run: &RunId) -> Option<WorkflowJob> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(run)
    }

    pub(crate) fn occupied(&self, run: &RunId) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(run)
    }

    pub(crate) fn available(&self, run: &RunId, session: &SessionId) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(run)
            .is_some_and(|job| job.session_id == *session)
    }

    pub(crate) fn put_back(&self, job: WorkflowJob) {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(job.run_id, job);
    }

    pub(crate) fn commit_recovery_locked(&self) -> bool {
        !self
            .recovery_protection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty()
    }

    fn take_provider(&self, provider: crate::providers::ProviderKind) -> Vec<WorkflowJob> {
        self.take_matching(|job| {
            (!job.phase_providers.is_empty() && job.phase_providers.contains(&provider))
                || (job.phase_providers.is_empty() && job.connection.kind == provider)
        })
    }

    fn take_session(&self, session: SessionId) -> Vec<WorkflowJob> {
        self.take_matching(|job| job.session_id == session)
    }

    fn take_matching(&self, predicate: impl Fn(&WorkflowJob) -> bool) -> Vec<WorkflowJob> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner
            .extract_if(.., |_, job| predicate(job))
            .map(|(_, job)| job)
            .collect()
    }
}

#[derive(Clone)]
pub(crate) struct WorkflowJob {
    pub(crate) run_id: RunId,
    pub(crate) session_id: SessionId,
    pub(crate) agent_id: Option<crate::agents::AgentId>,
    pub(crate) agent_revision: u32,
    pub(crate) conversation_id: Option<ConversationId>,
    pub(crate) authority: Option<EffectiveAuthority>,
    pub(crate) project_free_authority: Option<crate::execution::ProjectFreeAuthority>,
    pub(crate) grant_alias: String,
    pub(crate) connection: ProviderConnection,
    pub(crate) phase_providers: Vec<crate::providers::ProviderKind>,
    pub(crate) active_connection: Arc<std::sync::Mutex<Option<ProviderConnection>>>,
    pub(crate) host_policy: DirectoryPolicy,
    pub(crate) turns: Vec<ChatTurn>,
    pub(crate) job: Arc<Job>,
    pub(crate) eligible_reply: Arc<std::sync::Mutex<String>>,
}

impl WorkflowJob {
    pub(crate) fn conversation_key(&self) -> Option<crate::sessions::ConversationKey> {
        match (self.conversation_id, self.agent_id) {
            (None, Some(agent_id)) => Some(crate::sessions::ConversationKey { agent_id }),
            _ => None,
        }
    }

    fn active_connection(&self) -> ProviderConnection {
        self.active_connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .unwrap_or_else(|| self.connection.clone())
    }
}

fn set_active_connection(job: &WorkflowJob, connection: Option<ProviderConnection>) {
    if let Some(connection) = connection {
        *job.active_connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(connection);
    }
}

pub(crate) fn validate_phase_selection(
    state: &AppState,
    selection: &crate::providers::ModelSelection,
) -> Result<ProviderConnection, String> {
    if state
        .models_dev
        .model(selection.provider, &selection.model)
        .is_none()
    {
        return Err("The selected phase model is unavailable.".to_owned());
    }
    match selection.thinking.as_ref() {
        Some(effort)
            if !state
                .models_dev
                .supports(selection.provider, &selection.model, effort) =>
        {
            return Err("The selected phase thinking effort is unavailable.".to_owned());
        }
        None if !state
            .models_dev
            .efforts(selection.provider, &selection.model)
            .is_empty() =>
        {
            return Err("The selected phase needs a thinking effort.".to_owned());
        }
        _ => {}
    }
    state
        .vault
        .connection_for(selection)
        .ok_or_else(|| "The provider for this phase is no longer stored.".to_owned())
}

fn phase_connection(
    state: &AppState,
    job: &WorkflowJob,
    run: &crate::workflows::WorkflowRun,
    step: &StepDefinition,
) -> Result<Option<ProviderConnection>, String> {
    if !matches!(&step.action, StepAction::Agent(_)) {
        return Ok(None);
    }
    let Some(selection) = run.phase_model(&step.key) else {
        return Ok(Some(job.connection.clone()));
    };
    validate_phase_selection(state, &selection.selection).map(Some)
}

fn phase_authority(
    _state: &AppState,
    job: &WorkflowJob,
    run: &crate::workflows::WorkflowRun,
    step: &crate::workflows::definition::StepKey,
) -> Result<Option<EffectiveAuthority>, String> {
    let Some(base) = job.authority.clone() else {
        return Ok(None);
    };
    let Some(selection) = run.phase_model(step) else {
        return Ok(Some(base));
    };
    let Some(settings) = selection.settings.as_ref() else {
        return Ok(Some(base));
    };
    crate::conversations::apply_settings_ceiling(&base, settings)
        .map(Some)
        .map_err(|error| error.message().to_owned())
}

pub(crate) fn interrupt_provider_continuations(
    state: &AppState,
    provider: crate::providers::ProviderKind,
) -> Result<(), StoreError> {
    interrupt_continuations(state, state.gate_continuations.take_provider(provider))
}

pub(crate) fn interrupt_session_continuations(
    state: &AppState,
    session: SessionId,
) -> Result<(), StoreError> {
    interrupt_continuations(state, state.gate_continuations.take_session(session))
}

fn interrupt_continuations(state: &AppState, jobs: Vec<WorkflowJob>) -> Result<(), StoreError> {
    let mut jobs = jobs.into_iter();
    while let Some(continuation) = jobs.next() {
        let job = &continuation;
        if state
            .workflow_runs
            .mutate(&job.run_id, |run| {
                if run.recoverable_gate() {
                    Ok(())
                } else {
                    run.interrupt(now_ms())
                }
            })
            .is_err()
        {
            state.gate_continuations.put_back(continuation);
            for unprocessed in jobs {
                state.gate_continuations.put_back(unprocessed);
            }
            return Err(StoreError::Persist);
        }
        settle_with_reply(
            state,
            job,
            JobStatus::Cancelled,
            None,
            &crate::providers::AssistantReply::default(),
        );
    }
    Ok(())
}

pub(crate) async fn execute_run(
    state: AppState,
    mut job: WorkflowJob,
    _agent_lease: Option<LeaseGuard>,
    _execution_lease: ExecutionGuard,
) {
    loop {
        let Some(run) = state.workflow_runs.get(&job.run_id) else {
            fail_operational(&state, &job);
            return;
        };
        if run.is_terminal() {
            fail_operational(&state, &job);
            return;
        }
        if job.job.cancel_requested() {
            if persist_cancel(&state, &job.run_id).is_err() {
                fail_operational(&state, &job);
            } else if finish_driven_job(&state, &mut job, JobStatus::Cancelled, None) {
                return;
            } else {
                continue;
            }
            return;
        }
        if matches!(run.source, crate::workflows::run::RunSource::Pending) {
            if let Err(error) = capture_initial_source(&state, &job).await {
                if persist_initial_fail(&state, &job.run_id).is_err() {
                    fail_operational(&state, &job);
                } else if finish_driven_job(&state, &mut job, JobStatus::Failed, Some(&error)) {
                    return;
                } else {
                    continue;
                }
                return;
            }
            continue;
        }
        if (job.authority.is_some() || job.project_free_authority.is_some())
            && let Err(error) = confirm_run_authority(&state, &job)
        {
            if state
                .workflow_runs
                .mutate(&job.run_id, |run| run.fail_before_attempt(now_ms()))
                .is_err()
            {
                fail_operational(&state, &job);
            } else {
                settle_job(&state, &job, JobStatus::Failed, Some(&error));
            }
            return;
        }
        let Some(step_key) = run.ready_step().cloned() else {
            fail_operational(&state, &job);
            return;
        };
        let Some(step) = run.pinned.definition.step(&step_key).cloned() else {
            fail_operational(&state, &job);
            return;
        };
        let _application = if matches!(
            &step.action,
            StepAction::SystemCommand(action)
                if matches!(action.command, SystemCommandId::ApplyChanges | SystemCommandId::CommitCandidate)
        ) {
            let application = tokio::select! {
                result = state.workflow_execution.lock_application() => result,
                _ = job.job.cancelled() => continue,
            };
            match application {
                Ok(application) => Some(application),
                Err(error) => {
                    if state
                        .workflow_runs
                        .mutate(&job.run_id, |run| run.fail_before_attempt(now_ms()))
                        .is_err()
                    {
                        fail_operational(&state, &job);
                    } else {
                        settle_job(&state, &job, JobStatus::Failed, Some(error));
                    }
                    return;
                }
            }
        } else {
            None
        };
        let connection = match phase_connection(&state, &job, &run, &step) {
            Ok(connection) => connection,
            Err(error) => {
                settle_job(&state, &job, JobStatus::Failed, Some(&error));
                return;
            }
        };
        set_active_connection(&job, connection);
        let phase_authority = match phase_authority(&state, &job, &run, &step.key) {
            Ok(authority) => authority,
            Err(error) => {
                settle_job(&state, &job, JobStatus::Failed, Some(&error));
                return;
            }
        };
        let inputs = match resolve_inputs(&run, &step) {
            Ok(inputs) => inputs,
            Err(error) => {
                settle_job(&state, &job, JobStatus::Failed, Some(error));
                return;
            }
        };
        if let Err(error) = crate::workflows::input_context::verify_inputs(
            &run,
            &step,
            &inputs,
            &state.workflow_artefacts,
        ) {
            settle_job(&state, &job, JobStatus::Failed, Some(error.message()));
            return;
        }
        if matches!(step.action, StepAction::HumanGate(_)) {
            match state
                .workflow_runs
                .mutate(&job.run_id, |run| run.complete_unchanged_task())
            {
                Ok(_) => {
                    if finish_driven_job(&state, &mut job, JobStatus::Completed, None) {
                        return;
                    }
                    continue;
                }
                Err(StoreError::Conflict) => {}
                Err(_) => {
                    fail_operational(&state, &job);
                    return;
                }
            }
        }
        if matches!(step.action, StepAction::HumanGate(_)) {
            let plan_checkpoint = matches!(
                &step.action,
                StepAction::HumanGate(action) if action.is_plan_checkpoint()
            );
            let subject = inputs
                .iter()
                .find(|input| {
                    input.artefact.kind
                        == if plan_checkpoint {
                            crate::workflows::definition::ArtefactKind::Plan
                        } else {
                            crate::workflows::definition::ArtefactKind::CandidateRevision
                        }
                })
                .map(|input| input.artefact.clone());
            let Some(subject) = subject else {
                settle_job(
                    &state,
                    &job,
                    JobStatus::Failed,
                    Some(if plan_checkpoint {
                        "A plan checkpoint needs a plan input."
                    } else {
                        "A human gate needs a candidate input."
                    }),
                );
                return;
            };
            let initial = match &run.source {
                crate::workflows::RunSource::Captured { source } => source.initial.clone(),
                crate::workflows::RunSource::None | crate::workflows::RunSource::Pending => {
                    settle_job(
                        &state,
                        &job,
                        JobStatus::Failed,
                        Some(OPERATIONAL_STORE_ERROR),
                    );
                    return;
                }
            };
            let Ok(gate_id) = crate::workflows::GateId::generate() else {
                settle_job(
                    &state,
                    &job,
                    JobStatus::Failed,
                    Some(OPERATIONAL_STORE_ERROR),
                );
                return;
            };
            let opened = state.workflow_runs.mutate(&job.run_id, |run| {
                if plan_checkpoint {
                    run.open_plan_gate(gate_id, subject.clone(), now_ms())
                        .map(|_| ())
                } else {
                    run.open_gate(gate_id, subject.clone(), initial.clone(), now_ms())
                        .map(|_| ())
                }
            });
            if opened.is_err() {
                settle_job(
                    &state,
                    &job,
                    JobStatus::Failed,
                    Some(OPERATIONAL_STORE_ERROR),
                );
                return;
            }
            let _ = job.job.set_awaiting_decision();
            if let Some(conversation_id) = job.conversation_id
                && !state.sessions.owns_conversation_job(
                    &job.session_id,
                    conversation_id,
                    job.job.id(),
                )
            {
                let _ = state
                    .workflow_runs
                    .mutate(&run.id, |run| run.interrupt(now_ms()));
                let _ = finish_driven_job(&state, &mut job, JobStatus::Cancelled, None);
                return;
            }
            if !state.gate_continuations.insert(job) {
                let _ = state
                    .workflow_runs
                    .mutate(&run.id, |run| run.interrupt(now_ms()));
            }
            return;
        }
        let apply_step = matches!(
            &step.action,
            crate::workflows::definition::StepAction::SystemCommand(action)
                if action.command == crate::workflows::commands::SystemCommandId::ApplyChanges
        );
        let commit_step = matches!(
            &step.action,
            crate::workflows::definition::StepAction::SystemCommand(action)
                if action.command == crate::workflows::commands::SystemCommandId::CommitCandidate
        );
        if commit_step && run.kind != crate::workflows::RunKind::Configured {
            if persist_fail(&state, &job.run_id, None, FailureCategory::Definition).is_err() {
                fail_operational(&state, &job);
                return;
            }
            settle_job(
                &state,
                &job,
                JobStatus::Failed,
                Some(
                    "Ordinary agent work cannot create a commit through an application step. The prepared candidate remains unchanged.",
                ),
            );
            return;
        }
        let apply_precondition = if apply_step {
            crate::workflows::apply::require_approval(
                &run,
                &step,
                &inputs,
                &state.workflow_artefacts,
            )
            .and_then(|_| {
                reject_stale_assurance(&state, &run, &inputs)
                    .map_err(|_| crate::workflows::apply::ApplyExecutionError::Assurance)
            })
            .err()
        } else {
            None
        };
        let commit_precondition = if commit_step {
            crate::workflows::commit::require_commit_approval(
                &run,
                &step,
                &inputs,
                &state.workflow_artefacts,
            )
            .and_then(|_| {
                reject_stale_assurance(&state, &run, &inputs)
                    .map_err(|_| crate::workflows::commit::CommitError::Assurance)
            })
            .err()
        } else {
            if let Err(error) = reject_stale_assurance(&state, &run, &inputs) {
                settle_job(&state, &job, JobStatus::Failed, Some(error));
                return;
            }
            None
        };
        let attempt_id = match run
            .revision_reservation
            .as_ref()
            .map(|reservation| Ok(reservation.attempt))
            .unwrap_or_else(AttemptId::generate)
        {
            Ok(id) => id,
            Err(_) => {
                fail_operational(&state, &job);
                return;
            }
        };
        let capabilities = if let Some(authority) = job.project_free_authority.as_ref() {
            if run.agent_id.is_some() || run.conversation_id != job.conversation_id {
                settle_job(
                    &state,
                    &job,
                    JobStatus::Failed,
                    Some("The private workspace authority does not match this run."),
                );
                return;
            }
            let step_authority = match run.phase_settings(&step.key) {
                Some(settings) => crate::execution::ProjectFreeAuthority::from_settings(
                    authority.revision,
                    settings,
                )
                .map_err(|_| crate::workflows::capabilities::CapabilityError::Authority),
                None => Ok(authority.clone()),
            };
            step_authority.and_then(|step_authority| {
                crate::workflows::capabilities::AttemptCapabilities::derive_project_free(
                    &step,
                    &step_authority,
                )
            })
        } else if let Some(authority) = phase_authority.as_ref() {
            if run.conversation_id != job.conversation_id {
                settle_job(
                    &state,
                    &job,
                    JobStatus::Failed,
                    Some("The conversation authority does not match this run."),
                );
                return;
            }
            crate::workflows::capabilities::AttemptCapabilities::derive_for_authority(
                &step, authority,
            )
        } else {
            let Some(agent) = job.agent_id.and_then(|id| state.agents.get(&id)) else {
                fail_operational(&state, &job);
                return;
            };
            crate::workflows::capabilities::AttemptCapabilities::derive(
                &step,
                &agent,
                &job.grant_alias,
            )
        };
        let capabilities = match capabilities {
            Ok(capabilities) => capabilities,
            Err(error) => {
                settle_job(&state, &job, JobStatus::Failed, Some(error.message()));
                return;
            }
        };
        let host_step = step_is_host(&run, &step);
        let sandbox_record = if host_step {
            host_sandbox_record()
        } else {
            let Some(snapshot_digest) = run
                .environments
                .steps
                .iter()
                .find(|item| item.step == step.key)
                .map(|item| item.snapshot_digest.clone())
            else {
                fail_operational(&state, &job);
                return;
            };
            crate::workflows::run::AttemptSandboxRecord {
                kind: if apply_step {
                    crate::workflows::run::AttemptSandboxKind::FileApplication
                } else {
                    crate::workflows::run::AttemptSandboxKind::IsolatedAttempt
                },
                snapshot_digest,
            }
        };
        if persist_start(
            &state,
            &job.run_id,
            attempt_id,
            inputs.clone(),
            capabilities.clone(),
            sandbox_record,
        )
        .is_err()
        {
            fail_operational(&state, &job);
            return;
        }
        if let Some(error) = apply_precondition {
            let stored = persist_cleanup(
                &state,
                &job.run_id,
                attempt_id,
                crate::workflows::run::AttemptCleanupRecord::Complete,
            )
            .and_then(|_| {
                persist_fail(
                    &state,
                    &job.run_id,
                    Some(attempt_id),
                    FailureCategory::Assurance,
                )
            });
            if stored.is_err() {
                fail_operational(&state, &job);
            } else {
                settle_job(&state, &job, JobStatus::Failed, Some(error.message()));
            }
            return;
        }
        if let Some(error) = commit_precondition {
            let stored = persist_cleanup(
                &state,
                &job.run_id,
                attempt_id,
                crate::workflows::run::AttemptCleanupRecord::Complete,
            )
            .and_then(|_| {
                persist_fail(
                    &state,
                    &job.run_id,
                    Some(attempt_id),
                    FailureCategory::Assurance,
                )
            });
            if stored.is_err() {
                fail_operational(&state, &job);
            } else {
                settle_job(&state, &job, JobStatus::Failed, Some(error.message()));
            }
            return;
        }
        if let Err(error) = super::direct::before(&state, &job, &step, attempt_id) {
            if persist_cleanup(
                &state,
                &job.run_id,
                attempt_id,
                super::run::AttemptCleanupRecord::Complete,
            )
            .and_then(|_| {
                persist_fail(
                    &state,
                    &job.run_id,
                    Some(attempt_id),
                    FailureCategory::Operational,
                )
            })
            .is_err()
            {
                fail_operational(&state, &job);
            } else {
                settle_job(&state, &job, JobStatus::Failed, Some(error));
            }
            return;
        }
        let isolated =
            isolate_and_run(&state, &job, &step, attempt_id, &inputs, &capabilities).await;
        let (mut outcome, mut cleanup, drafts, captured) = match isolated {
            IsolatedRun::Finished {
                outcome,
                cleanup,
                drafts,
                captured,
            } => (outcome, cleanup, drafts, captured),
        };
        if let Err(error) = super::direct::after(&state, &job, &step, attempt_id) {
            outcome = StepOutcome::Failed {
                category: FailureCategory::Operational,
                error: Some(format!(
                    "{error} Direct writes remain. The final diff is unavailable."
                )),
            };
        }
        record_missing_terminal_evidence(&state, &job, &step, attempt_id, &outcome);
        if cleanup != crate::workflows::run::AttemptCleanupRecord::Complete {
            let _ = persist_cleanup(&state, &job.run_id, attempt_id, cleanup);
            state.gate_continuations.protect_cleanup_failure(
                &job.job,
                _agent_lease,
                _execution_lease,
            );
            return;
        }
        let recovery_pending = state.workflow_runs.get(&job.run_id).is_some_and(|run| {
            run.attempts
                .iter()
                .find(|attempt| attempt.id == attempt_id)
                .is_some_and(|attempt| {
                    attempt.commit_transaction.is_some() || attempt.apply_transaction.is_some()
                })
                && !matches!(outcome, StepOutcome::Completed)
        });
        if recovery_pending {
            if cleanup != crate::workflows::run::AttemptCleanupRecord::Complete
                || recover_apply_transactions(&state).is_err()
                || recover_commit_transactions(&state).is_err()
            {
                state.gate_continuations.protect_commit_recovery(
                    &job.job,
                    _agent_lease,
                    _execution_lease,
                );
                return;
            }
            if let Some(run) = state.workflow_runs.get(&job.run_id)
                && run.active_attempt() != Some(attempt_id)
            {
                if run.is_terminal() {
                    if finish_terminal_run(&state, &mut job, &run) {
                        return;
                    }
                    continue;
                }
                continue;
            }
        }
        let atomic_agent_publication = matches!(step.action, StepAction::Agent(_))
            && matches!(outcome, StepOutcome::Completed)
            && cleanup == crate::workflows::run::AttemptCleanupRecord::Complete;
        if atomic_agent_publication
            && persist_cleanup(&state, &job.run_id, attempt_id, cleanup.clone()).is_err()
        {
            fail_operational(&state, &job);
            return;
        }
        let mut published = false;
        if matches!(outcome, StepOutcome::Completed) {
            match publish_success(
                &state,
                &job,
                &step,
                SuccessAttempt {
                    id: attempt_id,
                    complete: atomic_agent_publication,
                },
                &inputs,
                &drafts,
                captured.as_ref(),
            ) {
                Ok(()) => published = true,
                Err(error) => {
                    outcome = StepOutcome::Failed {
                        category: FailureCategory::Definition,
                        error: Some(error.to_owned()),
                    };
                }
            }
        }
        if matches!(
            &step.action,
            StepAction::SystemCommand(action)
                if action.command == SystemCommandId::ApplyChanges
        ) {
            let retain_journal = state.workflow_runs.get(&job.run_id).is_some_and(|run| {
                run.attempts
                    .iter()
                    .find(|attempt| attempt.id == attempt_id)
                    .and_then(|attempt| attempt.apply_transaction.as_ref())
                    .is_some_and(|transaction| {
                        !transaction.is_settled() && !matches!(outcome, StepOutcome::Completed)
                    })
            });
            if retain_journal {
                state.gate_continuations.protect_apply_recovery(
                    &job.job,
                    _agent_lease,
                    _execution_lease,
                );
                return;
            }
        }
        if matches!(
            &step.action,
            StepAction::SystemCommand(action)
                if action.command == SystemCommandId::CommitCandidate
        ) {
            let retain_journal = state
                .workflow_runs
                .get(&job.run_id)
                .and_then(|run| {
                    run.attempts
                        .iter()
                        .find(|attempt| attempt.id == attempt_id)
                        .and_then(|attempt| attempt.commit_transaction.as_ref())
                        .map(|transaction| {
                            matches!(
                                transaction.state,
                                crate::workflows::commit::CommitTransactionState::ReferenceUpdated { .. }
                            ) && !matches!(outcome, StepOutcome::Completed)
                        })
                })
                .unwrap_or(false);
            if retain_journal {
                state.gate_continuations.protect_commit_recovery(
                    &job.job,
                    _agent_lease,
                    _execution_lease,
                );
                return;
            }
            let journal_gone = state.commit_journals.remove(job.run_id, attempt_id).is_ok();
            if !journal_gone {
                cleanup = match cleanup {
                    crate::workflows::run::AttemptCleanupRecord::Orphaned {
                        sandbox,
                        workspace,
                        ..
                    } => crate::workflows::run::AttemptCleanupRecord::Orphaned {
                        sandbox,
                        workspace,
                        journal: true,
                    },
                    _ => crate::workflows::run::AttemptCleanupRecord::Orphaned {
                        sandbox: false,
                        workspace: false,
                        journal: true,
                    },
                };
                outcome = StepOutcome::Failed {
                    category: FailureCategory::Cleanup,
                    error: Some("Power Plant could not clean up the commit journal.".to_owned()),
                };
            }
        }
        if !atomic_agent_publication
            && persist_cleanup(&state, &job.run_id, attempt_id, cleanup).is_err()
        {
            fail_operational(&state, &job);
            return;
        }
        if atomic_agent_publication && published {
            if let Some(run) = state.workflow_runs.get(&job.run_id)
                && run.is_terminal()
            {
                if finish_terminal_run(&state, &mut job, &run) {
                    return;
                }
                continue;
            }
            continue;
        }
        if let Err(error) = finalise_attempt(
            &state,
            &job,
            &step,
            attempt_id,
            &inputs,
            captured.as_ref(),
            &outcome,
            published,
        )
        .await
        {
            fail_operational(&state, &job);
            let _ = error;
            return;
        }
        if apply_step && state.apply_journals.remove(job.run_id, attempt_id).is_err() {
            state.gate_continuations.protect_apply_recovery(
                &job.job,
                _agent_lease,
                _execution_lease,
            );
            return;
        }
        match outcome {
            StepOutcome::Completed => {
                if let Some(run) = state.workflow_runs.get(&job.run_id)
                    && run.is_terminal()
                {
                    if finish_terminal_run(&state, &mut job, &run) {
                        return;
                    }
                    continue;
                }
            }
            StepOutcome::Failed { error, .. } => {
                if finish_driven_job(&state, &mut job, JobStatus::Failed, error.as_deref()) {
                    return;
                }
                continue;
            }
            StepOutcome::Cancelled => {
                if finish_driven_job(&state, &mut job, JobStatus::Cancelled, None) {
                    return;
                }
                continue;
            }
        }
    }
}

pub(crate) fn settle_terminal_job(
    state: &AppState,
    job: &WorkflowJob,
    run: &crate::workflows::WorkflowRun,
) {
    match &run.state {
        crate::workflows::run::RunState::Escalated { reason, .. } => {
            let message = match reason {
                crate::workflows::run::EscalationReason::Blocked => {
                    "The review blocked this workflow run."
                }
                crate::workflows::run::EscalationReason::AttemptLimit => {
                    "The review attempt limit escalated this workflow run."
                }
            };
            settle_job(state, job, JobStatus::Failed, Some(message));
        }
        crate::workflows::run::RunState::Failed | crate::workflows::run::RunState::Interrupted => {
            settle_job(
                state,
                job,
                JobStatus::Failed,
                Some("The task did not complete."),
            );
        }
        crate::workflows::run::RunState::Cancelled => {
            settle_job(state, job, JobStatus::Cancelled, None);
        }
        crate::workflows::run::RunState::Completed => {
            settle_job(state, job, JobStatus::Completed, None);
        }
        _ => {}
    }
}

enum StepOutcome {
    Completed,
    Failed {
        category: FailureCategory,
        error: Option<String>,
    },
    Cancelled,
}

enum IsolatedRun {
    Finished {
        outcome: StepOutcome,
        cleanup: crate::workflows::run::AttemptCleanupRecord,
        drafts: std::sync::Arc<std::sync::Mutex<crate::workflows::artefacts::output::OutputDrafts>>,
        captured: Option<crate::workflows::artefacts::CandidatePayload>,
    },
}

async fn isolate_and_run(
    state: &AppState,
    job: &WorkflowJob,
    step: &StepDefinition,
    attempt_id: AttemptId,
    inputs: &[super::run::AttemptArtefactInput],
    capabilities: &crate::workflows::capabilities::AttemptCapabilities,
) -> IsolatedRun {
    let drafts = std::sync::Arc::new(std::sync::Mutex::new(
        crate::workflows::artefacts::output::OutputDrafts::default(),
    ));
    if let Some(run) = state.workflow_runs.get(&job.run_id)
        && step_is_host(&run, step)
    {
        let StepAction::Agent(action) = &step.action else {
            return IsolatedRun::Finished {
                outcome: StepOutcome::Failed {
                    category: FailureCategory::Definition,
                    error: Some("Host steps must be model steps.".to_owned()),
                },
                cleanup: crate::workflows::run::AttemptCleanupRecord::Complete,
                drafts,
                captured: None,
            };
        };
        let outcome = run_agent_step(state, job, action, None, drafts.clone()).await;
        return IsolatedRun::Finished {
            outcome,
            cleanup: crate::workflows::run::AttemptCleanupRecord::Complete,
            drafts,
            captured: None,
        };
    }
    if job.job.cancel_requested() {
        return IsolatedRun::Finished {
            outcome: StepOutcome::Cancelled,
            cleanup: crate::workflows::run::AttemptCleanupRecord::Complete,
            drafts,
            captured: None,
        };
    }
    let private_workspace = capabilities.source_location
        == crate::workflows::capabilities::PrimarySourceLocation::PrivateWorkspace;
    let candidate_input = load_candidate_input(state, job, inputs);
    if candidate_input.is_none() && !private_workspace {
        return IsolatedRun::Finished {
            outcome: StepOutcome::Failed {
                category: FailureCategory::Definition,
                error: Some("A sandbox-backed step needs a candidate input.".to_owned()),
            },
            cleanup: crate::workflows::run::AttemptCleanupRecord::Complete,
            drafts,
            captured: None,
        };
    }
    let workspace = match state
        .workflow_workspaces
        .create_attempt(job.run_id, attempt_id)
    {
        Ok(workspace) => workspace,
        Err(error) => {
            let cleanup = if error.orphaned {
                crate::workflows::run::AttemptCleanupRecord::Orphaned {
                    sandbox: false,
                    workspace: true,
                    journal: false,
                }
            } else {
                crate::workflows::run::AttemptCleanupRecord::Complete
            };
            return IsolatedRun::Finished {
                outcome: fail_for_orphan(
                    StepOutcome::Failed {
                        category: FailureCategory::Operational,
                        error: Some(
                            "Power Plant could not create the attempt workspace.".to_owned(),
                        ),
                    },
                    &cleanup,
                ),
                cleanup,
                drafts,
                captured: None,
            };
        }
    };
    if candidate_input.as_ref().is_some_and(|candidate_input| {
        materialise_candidate(
            &workspace,
            &candidate_input.artefact,
            candidate_input.artefact_hash,
            &state.workflow_artefacts,
        )
        .is_err()
    }) {
        let (outcome, cleanup) = finish_workspace_only(
            workspace,
            StepOutcome::Failed {
                category: FailureCategory::Operational,
                error: Some("Power Plant could not materialise the source tree.".to_owned()),
            },
        );
        return IsolatedRun::Finished {
            outcome,
            cleanup,
            drafts,
            captured: None,
        };
    }
    let commit_attempt = matches!(
        &step.action,
        StepAction::SystemCommand(action) if action.command == SystemCommandId::CommitCandidate
    );
    // Directory candidates own their roots. Only Git commits need a host primary.
    let user_project =
        if private_workspace || (job.project_free_authority.is_some() && !commit_attempt) {
            workspace.project.clone()
        } else {
            match job
                .host_policy
                .grants()
                .iter()
                .find(|grant| grant.alias == job.host_policy.primary_alias())
            {
                Some(grant) => grant.host_path.clone(),
                None => {
                    let (outcome, cleanup) = finish_workspace_only(
                        workspace,
                        StepOutcome::Failed {
                            category: FailureCategory::Operational,
                            error: Some(OPERATIONAL_STORE_ERROR.to_owned()),
                        },
                    );
                    return IsolatedRun::Finished {
                        outcome,
                        cleanup,
                        drafts,
                        captured: None,
                    };
                }
            }
        };
    if matches!(
        &step.action,
        StepAction::SystemCommand(action) if action.command == SystemCommandId::ApplyChanges
    ) {
        let outcome = run_apply_transaction(state, job, step, attempt_id, inputs);
        let captured = if matches!(outcome, StepOutcome::Completed) {
            candidate_input
                .as_ref()
                .and_then(|candidate| match &candidate.artefact {
                    crate::workflows::artefacts::CandidatePayload::Revision(candidate) => {
                        crate::workflows::artefacts::CandidateCapture::capture_directory(
                            &user_project,
                            &candidate.exclusions,
                            &state.workflow_artefacts,
                        )
                        .ok()
                        .map(crate::workflows::artefacts::CandidatePayload::Revision)
                    }
                    crate::workflows::artefacts::CandidatePayload::Set(candidate) => Some(
                        crate::workflows::artefacts::CandidatePayload::Set(candidate.clone()),
                    ),
                })
        } else {
            None
        };
        let (outcome, cleanup) = finish_workspace_only(workspace, outcome);
        return IsolatedRun::Finished {
            outcome,
            cleanup,
            drafts,
            captured,
        };
    }
    let git_dir = user_project.join(".git");
    if !private_workspace
        && candidate_input
            .as_ref()
            .expect("candidate-backed attempt")
            .revision()
            .and_then(|candidate| candidate.git_admin.as_ref())
            .is_some_and(|expected| {
                crate::workflows::artefacts::candidate::git_fingerprint(&git_dir).as_ref()
                    != Ok(expected)
            })
    {
        let (outcome, cleanup) = finish_workspace_only(
            workspace,
            StepOutcome::Failed {
                category: FailureCategory::Operational,
                error: Some("The Git directory changed before that step.".to_owned()),
            },
        );
        return IsolatedRun::Finished {
            outcome,
            cleanup,
            drafts,
            captured: None,
        };
    }
    let sandbox = state.sandboxes.attempt_handle(job.run_id, attempt_id);
    if let Err(error) =
        start_attempt_sandbox(state, job, step, capabilities, &workspace, sandbox.clone()).await
    {
        let outcome = if job.job.cancel_requested() {
            StepOutcome::Cancelled
        } else {
            StepOutcome::Failed {
                category: FailureCategory::Operational,
                error: Some(error.to_owned()),
            }
        };
        let (outcome, cleanup) =
            cleanup_after_start_failure(state, attempt_id, sandbox, workspace, outcome).await;
        return IsolatedRun::Finished {
            outcome,
            cleanup,
            drafts,
            captured: None,
        };
    }
    let outcome = if commit_attempt {
        run_commit_transaction(
            state,
            job,
            step,
            attempt_id,
            inputs,
            &user_project,
            candidate_input.as_ref().expect("commit candidate"),
            &sandbox,
        )
        .await
    } else {
        dispatch_step(state, job, step, &sandbox, drafts.clone()).await
    };
    let stopped = sandbox.stop().await.is_ok();
    let captured = if stopped && commit_attempt {
        state.workflow_runs.get(&job.run_id).and_then(|run| {
            capture_commit_candidate(state, &run, &candidate_input.as_ref()?.artefact).ok()
        })
    } else if stopped && !private_workspace {
        capture_isolated_candidate(
            state,
            job,
            &workspace,
            &candidate_input
                .as_ref()
                .expect("candidate-backed attempt")
                .artefact,
            &git_dir,
        )
    } else {
        None
    };
    if commit_attempt && matches!(outcome, StepOutcome::Completed) {
        let commit = state.workflow_runs.get(&job.run_id).and_then(|run| {
            run.attempts
                .iter()
                .find(|attempt| attempt.id == attempt_id)
                .and_then(|attempt| attempt.commit_transaction.as_ref())
                .and_then(|transaction| transaction.expected_commit.clone())
        });
        let verified = captured
            .as_ref()
            .zip(commit.as_ref())
            .is_some_and(|(captured, commit)| {
                captured.candidate_hash()
                    == candidate_input
                        .as_ref()
                        .expect("commit candidate")
                        .artefact
                        .candidate_hash()
                    && state
                        .workflow_runs
                        .get(&job.run_id)
                        .as_ref()
                        .and_then(|run| {
                            crate::workflows::commit::destination_revision(run, captured).ok()
                        })
                        .and_then(|candidate| candidate.repository.as_ref())
                        .and_then(|repository| repository.head.as_ref())
                        .map(|head| head.0.as_str())
                        == Some(commit.as_str())
            });
        let recorded = verified
            && commit.as_ref().is_some_and(|commit| {
                let transaction_result = state.workflow_runs.mutate(&job.run_id, |run| {
                    let mut transaction = run
                        .attempts
                        .iter()
                        .find(|attempt| attempt.id == attempt_id)
                        .and_then(|attempt| attempt.commit_transaction.clone())
                        .ok_or(crate::workflows::run::TransitionError::Invalid)?;
                    transaction.state =
                        crate::workflows::commit::CommitTransactionState::Verified {
                            commit: commit.clone(),
                        };
                    run.record_commit_transaction(attempt_id, transaction)
                });
                transaction_result.is_ok()
                    && state
                        .workflow_runs
                        .mutate(&job.run_id, |run| {
                            run.record_commit_result(
                                attempt_id,
                                crate::workflows::commit::CommitResult {
                                    commit: commit.clone(),
                                },
                            )
                        })
                        .is_ok()
            });
        if !recorded {
            let sandbox_gone = stopped && sandbox.remove().await.is_ok();
            if sandbox_gone {
                state.sandboxes.drop_attempt(attempt_id);
            } else {
                state.sandboxes.expose_orphan(sandbox.name().to_owned());
            }
            let workspace_gone = sandbox_gone && workspace.destroy().is_ok();
            let cleanup = if sandbox_gone && workspace_gone {
                crate::workflows::run::AttemptCleanupRecord::Complete
            } else {
                crate::workflows::run::AttemptCleanupRecord::Orphaned {
                    sandbox: !sandbox_gone,
                    workspace: !workspace_gone,
                    journal: true,
                }
            };
            return IsolatedRun::Finished {
                outcome: StepOutcome::Failed {
                    category: FailureCategory::Commit,
                    error: Some("Power Plant could not verify the Git commit.".to_owned()),
                },
                cleanup,
                drafts,
                captured,
            };
        }
    }
    let sandbox_gone = if stopped {
        sandbox.remove().await.is_ok()
    } else {
        false
    };
    if sandbox_gone {
        state.sandboxes.drop_attempt(attempt_id);
    } else {
        state.sandboxes.expose_orphan(sandbox.name().to_owned());
    }
    let workspace_gone = if sandbox_gone {
        workspace.destroy().is_ok()
    } else {
        false
    };
    let cleanup = if sandbox_gone && workspace_gone {
        crate::workflows::run::AttemptCleanupRecord::Complete
    } else {
        crate::workflows::run::AttemptCleanupRecord::Orphaned {
            sandbox: !sandbox_gone,
            workspace: !workspace_gone,
            journal: false,
        }
    };
    let mut outcome = match (outcome, stopped, private_workspace || captured.is_some()) {
        (StepOutcome::Completed, true, true) => StepOutcome::Completed,
        (StepOutcome::Completed, _, _) => StepOutcome::Failed {
            category: FailureCategory::Operational,
            error: Some("Power Plant could not capture isolated outputs.".to_owned()),
        },
        (other, _, _) => other,
    };
    outcome = fail_for_orphan(outcome, &cleanup);
    IsolatedRun::Finished {
        outcome,
        cleanup,
        drafts,
        captured,
    }
}

fn materialise_candidate(
    workspace: &crate::workflows::workspace::AttemptWorkspace,
    payload: &crate::workflows::artefacts::CandidatePayload,
    artefact_hash: crate::workflows::artefacts::ArtefactHash,
    store: &crate::workflows::WorkflowArtefactRepository,
) -> Result<(), ()> {
    match payload {
        crate::workflows::artefacts::CandidatePayload::Revision(candidate) => {
            crate::workflows::artefacts::CandidateMaterialise::into_workspace(
                &workspace.project,
                candidate,
                artefact_hash,
                store,
            )
            .map_err(|_| ())
        }
        crate::workflows::artefacts::CandidatePayload::Set(set) => {
            for root in &set.roots {
                let destination = workspace.reviewed_root(&root.alias).map_err(|_| ())?;
                let bytes = root.candidate.manifest_bytes().map_err(|_| ())?;
                let hash = crate::workflows::artefacts::artefact_hash_for(
                    crate::workflows::definition::ArtefactKind::CandidateRevision,
                    root.candidate.format_version,
                    &bytes,
                );
                crate::workflows::artefacts::CandidateMaterialise::into_workspace(
                    &destination,
                    &root.candidate,
                    hash,
                    store,
                )
                .map_err(|_| ())?;
            }
            Ok(())
        }
    }
}

fn capture_isolated_candidate(
    state: &AppState,
    job: &WorkflowJob,
    workspace: &crate::workflows::workspace::AttemptWorkspace,
    baseline: &crate::workflows::artefacts::CandidatePayload,
    git_dir: &std::path::Path,
) -> Option<crate::workflows::artefacts::CandidatePayload> {
    match baseline {
        crate::workflows::artefacts::CandidatePayload::Revision(candidate) => {
            crate::workflows::artefacts::CandidateCapture::capture_isolated(
                &workspace.project,
                candidate,
                git_dir,
                &state.workflow_artefacts,
            )
            .ok()
            .map(crate::workflows::artefacts::CandidatePayload::Revision)
        }
        crate::workflows::artefacts::CandidatePayload::Set(set) => {
            let run = state.workflow_runs.get(&job.run_id)?;
            let conversation = job
                .conversation_id
                .and_then(|id| state.conversations.get(&id));
            let settings = run.directory_settings().or_else(|| {
                conversation
                    .as_ref()?
                    .model
                    .as_ref()
                    .map(|model| model.settings.clone())
            })?;
            let reviewed = run.reviewed_directories();
            let grants = if reviewed.is_empty() {
                &settings.directories
            } else {
                &reviewed
            };
            crate::workflows::artefacts::CandidateCapture::capture_isolated_set(
                workspace,
                set,
                grants,
                &state.workflow_artefacts,
            )
            .ok()
            .map(crate::workflows::artefacts::CandidatePayload::Set)
        }
    }
}

fn finish_workspace_only(
    workspace: crate::workflows::workspace::AttemptWorkspace,
    outcome: StepOutcome,
) -> (StepOutcome, crate::workflows::run::AttemptCleanupRecord) {
    let cleanup = if workspace.destroy().is_ok() {
        crate::workflows::run::AttemptCleanupRecord::Complete
    } else {
        crate::workflows::run::AttemptCleanupRecord::Orphaned {
            sandbox: false,
            workspace: true,
            journal: false,
        }
    };
    let outcome = fail_for_orphan(outcome, &cleanup);
    (outcome, cleanup)
}

async fn cleanup_after_start_failure(
    state: &AppState,
    attempt_id: AttemptId,
    sandbox: Arc<GuestSandbox>,
    workspace: crate::workflows::workspace::AttemptWorkspace,
    outcome: StepOutcome,
) -> (StepOutcome, crate::workflows::run::AttemptCleanupRecord) {
    let stopped = sandbox.stop().await.is_ok();
    let sandbox_gone = stopped && sandbox.remove().await.is_ok();
    if sandbox_gone {
        state.sandboxes.drop_attempt(attempt_id);
    } else {
        state.sandboxes.expose_orphan(sandbox.name().to_owned());
    }
    let workspace_gone = sandbox_gone && workspace.destroy().is_ok();
    let cleanup = if sandbox_gone && workspace_gone {
        crate::workflows::run::AttemptCleanupRecord::Complete
    } else {
        crate::workflows::run::AttemptCleanupRecord::Orphaned {
            sandbox: !sandbox_gone,
            workspace: !workspace_gone,
            journal: false,
        }
    };
    let outcome = fail_for_orphan(outcome, &cleanup);
    (outcome, cleanup)
}

fn fail_for_orphan(
    outcome: StepOutcome,
    cleanup: &crate::workflows::run::AttemptCleanupRecord,
) -> StepOutcome {
    if matches!(
        cleanup,
        crate::workflows::run::AttemptCleanupRecord::Complete
    ) {
        outcome
    } else {
        StepOutcome::Failed {
            category: FailureCategory::Cleanup,
            error: Some("Power Plant could not clean up the isolated sandbox.".to_owned()),
        }
    }
}

struct LoadedCandidate {
    artefact_hash: crate::workflows::artefacts::ArtefactHash,
    artefact: crate::workflows::artefacts::CandidatePayload,
}

impl LoadedCandidate {
    fn revision(
        &self,
    ) -> Option<&crate::workflows::artefacts::candidate::CandidateRevisionArtefact> {
        self.artefact.revision()
    }
}

fn load_candidate_input(
    state: &AppState,
    job: &WorkflowJob,
    inputs: &[super::run::AttemptArtefactInput],
) -> Option<LoadedCandidate> {
    let input = inputs.iter().find(|input| {
        input.artefact.kind == crate::workflows::definition::ArtefactKind::CandidateRevision
    })?;
    let run = state.workflow_runs.get(&job.run_id)?;
    let record = run.artefact(&input.artefact.id)?;
    let bytes = state.workflow_artefacts.get(&record.object_hash).ok()?;
    let artefact = crate::workflows::artefacts::CandidatePayload::from_manifest_bytes(&bytes)?;
    Some(LoadedCandidate {
        artefact_hash: record.artefact_hash,
        artefact,
    })
}

fn run_apply_transaction(
    state: &AppState,
    job: &WorkflowJob,
    step: &StepDefinition,
    attempt_id: AttemptId,
    inputs: &[super::run::AttemptArtefactInput],
) -> StepOutcome {
    match execute_apply_transaction(state, job, step, attempt_id, inputs) {
        Ok(()) => StepOutcome::Completed,
        Err(error) => StepOutcome::Failed {
            category: match error {
                crate::workflows::apply::ApplyExecutionError::Assurance => {
                    FailureCategory::Assurance
                }
                crate::workflows::apply::ApplyExecutionError::Authority => {
                    FailureCategory::Authority
                }
                crate::workflows::apply::ApplyExecutionError::Operational => {
                    FailureCategory::Operational
                }
                crate::workflows::apply::ApplyExecutionError::Conflict
                | crate::workflows::apply::ApplyExecutionError::Integrity
                | crate::workflows::apply::ApplyExecutionError::Write => FailureCategory::Apply,
            },
            error: Some(error.message().to_owned()),
        },
    }
}

fn execute_apply_transaction(
    state: &AppState,
    job: &WorkflowJob,
    step: &StepDefinition,
    attempt_id: AttemptId,
    inputs: &[super::run::AttemptArtefactInput],
) -> Result<(), crate::workflows::apply::ApplyExecutionError> {
    use crate::workflows::apply::{
        ApplyExecutionError, ApplyRoot, ApplyTransaction, ApplyTransactionState,
    };

    confirm_run_authority(state, job).map_err(|_| ApplyExecutionError::Authority)?;
    let conversation_id = job.conversation_id.ok_or(ApplyExecutionError::Authority)?;
    let conversation = state
        .conversations
        .get(&conversation_id)
        .and_then(|record| record.model)
        .ok_or(ApplyExecutionError::Authority)?;
    let run = state
        .workflow_runs
        .get(&job.run_id)
        .ok_or(ApplyExecutionError::Operational)?;
    let approved =
        crate::workflows::apply::require_approval(&run, step, inputs, &state.workflow_artefacts)?;
    let (
        crate::workflows::artefacts::CandidatePayload::Set(baseline),
        crate::workflows::artefacts::CandidatePayload::Set(candidate),
    ) = (&approved.baseline, &approved.candidate)
    else {
        return Err(ApplyExecutionError::Integrity);
    };
    let settings = run.directory_settings().unwrap_or(conversation.settings);
    let mut roots = Vec::new();
    for (before, after) in baseline.roots.iter().zip(&candidate.roots) {
        let grant = settings
            .directories
            .iter()
            .find(|grant| {
                grant.id == before.grant_id
                    && grant.alias == before.alias
                    && grant.access == crate::execution::DirectoryAccess::ReviewBeforeApply
            })
            .ok_or(ApplyExecutionError::Authority)?;
        grant
            .revalidate()
            .map_err(|_| ApplyExecutionError::Authority)?;
        if grant.identity != before.identity || before.identity != after.identity {
            return Err(ApplyExecutionError::Authority);
        }
        let outcome = if before.candidate == after.candidate {
            crate::workflows::apply::ApplyRootOutcome::Unchanged
        } else {
            crate::workflows::apply::ApplyRootOutcome::Pending
        };
        roots.push(ApplyRoot {
            grant_id: grant.id,
            alias: grant.alias.clone(),
            host_path: grant.host_path.clone(),
            identity: grant.identity,
            baseline_candidate: before.candidate.candidate_hash,
            candidate_hash: after.candidate.candidate_hash,
            exclusions: before.candidate.exclusions.clone(),
            outcome,
        });
    }
    if roots.len() != baseline.roots.len() || roots.is_empty() {
        return Err(ApplyExecutionError::Integrity);
    }
    let baseline_manifest = approved
        .baseline
        .manifest_bytes()
        .map_err(|_| ApplyExecutionError::Integrity)?;
    let mut transaction = ApplyTransaction {
        state: ApplyTransactionState::Prepared,
        roots,
        baseline: approved.baseline_reference,
        candidate: approved.candidate_reference,
        approval: approved.approval,
    };
    let journal = state
        .apply_journals
        .create(
            job.run_id,
            attempt_id,
            &transaction,
            &baseline_manifest,
            transaction.baseline.artefact_hash,
        )
        .map_err(|_| ApplyExecutionError::Operational)?;
    persist_apply_transaction(state, job.run_id, attempt_id, transaction.clone())?;
    transaction.apply_roots(
        baseline,
        candidate,
        &state.workflow_artefacts,
        &journal,
        |transaction| {
            persist_apply_transaction(state, job.run_id, attempt_id, transaction.clone())?;
            if matches!(transaction.state, ApplyTransactionState::Applying { .. }) {
                confirm_run_authority(state, job).map_err(|_| ApplyExecutionError::Authority)?;
                if job.job.cancel_requested() {
                    return Err(ApplyExecutionError::Write);
                }
            }
            Ok(())
        },
    )
}

fn persist_apply_transaction(
    state: &AppState,
    run_id: RunId,
    attempt_id: AttemptId,
    transaction: crate::workflows::apply::ApplyTransaction,
) -> Result<(), crate::workflows::apply::ApplyExecutionError> {
    state
        .workflow_runs
        .mutate(&run_id, |run| {
            run.record_apply_transaction(attempt_id, transaction)
        })
        .map(|_| ())
        .map_err(|_| crate::workflows::apply::ApplyExecutionError::Operational)
}

#[allow(clippy::too_many_arguments)]
async fn run_commit_transaction(
    state: &AppState,
    job: &WorkflowJob,
    _step: &StepDefinition,
    attempt_id: AttemptId,
    inputs: &[super::run::AttemptArtefactInput],
    user_project: &std::path::Path,
    target: &LoadedCandidate,
    sandbox: &GuestSandbox,
) -> StepOutcome {
    let result = execute_commit_transaction(
        state,
        job,
        attempt_id,
        inputs,
        user_project,
        target,
        sandbox,
    )
    .await;
    let temporary_index = user_project
        .join(".git")
        .join(format!("powerplant-commit-index-{}", attempt_id.as_hex()));
    match std::fs::remove_file(temporary_index) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) if result.is_ok() => {
            return StepOutcome::Failed {
                category: FailureCategory::Cleanup,
                error: Some("Power Plant could not clean up the temporary Git index.".to_owned()),
            };
        }
        Err(_) => {}
    }
    match result {
        Ok(()) => StepOutcome::Completed,
        Err(_) if job.job.cancel_requested() => StepOutcome::Cancelled,
        Err(error) => StepOutcome::Failed {
            category: error.category(),
            error: Some(error.message().to_owned()),
        },
    }
}

async fn execute_commit_transaction(
    state: &AppState,
    job: &WorkflowJob,
    attempt_id: AttemptId,
    inputs: &[super::run::AttemptArtefactInput],
    user_project: &std::path::Path,
    target: &LoadedCandidate,
    sandbox: &GuestSandbox,
) -> Result<(), crate::workflows::commit::CommitError> {
    use crate::workflows::commit::{CommitError, CommitTransaction, CommitTransactionState};

    let run = state
        .workflow_runs
        .get(&job.run_id)
        .ok_or(CommitError::Operational)?;
    confirm_run_authority(state, job).map_err(|_| CommitError::Authority)?;
    let crate::workflows::RunSource::Captured { source } = &run.source else {
        return Err(CommitError::Operational);
    };
    let initial_record = run
        .artefact(&source.initial.id)
        .ok_or(CommitError::Operational)?;
    let initial_bytes = state
        .workflow_artefacts
        .get(&initial_record.object_hash)
        .map_err(|_| CommitError::Operational)?;
    let initial_payload =
        crate::workflows::artefacts::CandidatePayload::from_manifest_bytes(&initial_bytes)
            .ok_or(CommitError::Operational)?;
    let (initial, target_revision) =
        crate::workflows::commit::destination_pair(&run, &initial_payload, &target.artefact)?;
    if capture_commit_candidate(state, &run, &initial_payload)
        .map_err(|_| CommitError::Preflight)?
        != initial_payload
    {
        return Err(CommitError::Preflight);
    }
    crate::workflows::commit::require_unchanged_project(
        user_project,
        initial,
        target_revision,
        &state.workflow_artefacts,
    )?;
    let initial_repository = initial.repository.as_ref().ok_or(CommitError::Preflight)?;
    let target_repository = target_revision
        .repository
        .as_ref()
        .ok_or(CommitError::Preflight)?;
    let expected_reference = current_reference(user_project)?;
    let candidate = inputs
        .iter()
        .find(|input| {
            input.artefact.kind == crate::workflows::definition::ArtefactKind::CandidateRevision
        })
        .map(|input| input.artefact.clone())
        .ok_or(CommitError::Assurance)?;
    let reviews: Vec<_> = inputs
        .iter()
        .filter(|input| {
            input.artefact.kind == crate::workflows::definition::ArtefactKind::ReviewReport
        })
        .map(|input| input.artefact.clone())
        .collect();
    let approval = inputs
        .iter()
        .find(|input| {
            input.artefact.kind == crate::workflows::definition::ArtefactKind::HumanDecision
        })
        .map(|input| input.artefact.clone());
    if reviews.is_empty() && approval.is_none() {
        return Err(CommitError::Assurance);
    }
    let timestamp = crate::workflows::commit::utc_timestamp(now_ms());
    let mut transaction = CommitTransaction {
        state: CommitTransactionState::Prepared,
        candidate,
        reviews,
        approval,
        expected_reference,
        old_object: initial_repository
            .head
            .as_ref()
            .map(|object| object.0.clone()),
        target_tree: None,
        expected_commit: None,
        timestamp: timestamp.clone(),
    };
    let journal = state
        .commit_journals
        .create(job.run_id, attempt_id)
        .map_err(|_| CommitError::Operational)?;
    let live_index = user_project.join(".git/index");
    let original_index = match std::fs::read(&live_index) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(_) => return Err(CommitError::Preflight),
    };
    journal
        .write_index_backup("original.index", &original_index)
        .map_err(|_| CommitError::Operational)?;
    journal.flush().map_err(|_| CommitError::Operational)?;
    persist_transaction(state, job.run_id, attempt_id, transaction.clone())?;

    let index_guest = crate::workflows::commit::temporary_index_guest(attempt_id);
    let index_host = user_project
        .join(".git")
        .join(format!("powerplant-commit-index-{}", attempt_id.as_hex()));
    if index_host.exists() {
        return Err(CommitError::Preflight);
    }
    run_git_capture(
        sandbox,
        &job.job,
        crate::workflows::commit::read_tree_empty_command(&index_guest, &timestamp),
        true,
    )
    .await?;
    let mut index_info = Vec::new();
    for entry in &target_revision.entries {
        let object = match &entry.kind {
            crate::workflows::artefacts::candidate::CandidateEntryKind::Regular {
                blob, ..
            }
            | crate::workflows::artefacts::candidate::CandidateEntryKind::Symlink {
                blob, ..
            } => {
                let bytes = state
                    .workflow_artefacts
                    .get(blob)
                    .map_err(|_| CommitError::Operational)?;
                let output = run_git_capture(
                    sandbox,
                    &job.job,
                    crate::workflows::commit::hash_object_command(bytes, &timestamp),
                    true,
                )
                .await?;
                crate::workflows::commit::parse_object_id(&output, target_repository.object_format)?
                    .0
            }
            crate::workflows::artefacts::candidate::CandidateEntryKind::Directory { .. } => {
                continue;
            }
            crate::workflows::artefacts::candidate::CandidateEntryKind::Gitlink { commit } => {
                crate::workflows::commit::parse_object_id(
                    &commit.0,
                    target_repository.object_format,
                )?
                .0
            }
        };
        index_info.extend(crate::workflows::commit::index_info_record(entry, &object)?);
    }
    run_git_capture(
        sandbox,
        &job.job,
        crate::workflows::commit::index_info_command(index_info, &index_guest, &timestamp),
        true,
    )
    .await?;
    let tree = run_git_capture(
        sandbox,
        &job.job,
        crate::workflows::commit::write_tree_command(&index_guest, &timestamp),
        true,
    )
    .await?;
    let tree = crate::workflows::commit::parse_object_id(&tree, target_repository.object_format)?.0;
    if let Some(old) = transaction.old_object.as_deref() {
        let old_tree = git_host_text(user_project, &["rev-parse", &format!("{old}^{{tree}}")])?;
        if old_tree == tree {
            return Err(CommitError::Preflight);
        }
    }
    transaction.target_tree = Some(tree.clone());
    persist_transaction(state, job.run_id, attempt_id, transaction.clone())?;
    let commit = run_git_capture(
        sandbox,
        &job.job,
        crate::workflows::commit::commit_tree_command(
            &tree,
            transaction.old_object.as_deref(),
            &timestamp,
        ),
        true,
    )
    .await?;
    let commit =
        crate::workflows::commit::parse_object_id(&commit, target_repository.object_format)?.0;
    let target_index = std::fs::read(&index_host).map_err(|_| CommitError::Command)?;
    journal
        .write_index_backup("target.index", &target_index)
        .map_err(|_| CommitError::Operational)?;
    journal.flush().map_err(|_| CommitError::Operational)?;
    std::fs::remove_file(&index_host).map_err(|_| CommitError::Operational)?;
    transaction.expected_commit = Some(commit.clone());
    persist_transaction(state, job.run_id, attempt_id, transaction.clone())?;
    if job.job.cancel_requested() {
        return Err(CommitError::Operational);
    }

    confirm_run_authority(state, job).map_err(|_| CommitError::Authority)?;
    let target_bytes = target_revision
        .manifest_bytes()
        .map_err(|_| CommitError::Operational)?;
    let target_hash = crate::workflows::artefacts::artefact_hash_for(
        crate::workflows::definition::ArtefactKind::CandidateRevision,
        target_revision.format_version,
        &target_bytes,
    );
    crate::workflows::artefacts::CandidateApply::apply(
        user_project,
        initial,
        target_revision,
        target_hash,
        &state.workflow_artefacts,
    )
    .map_err(map_apply_error)?;
    transaction.state = CommitTransactionState::WorktreeApplied;
    persist_transaction(state, job.run_id, attempt_id, transaction.clone())?;

    let old_guard = transaction.old_object.as_deref().unwrap_or({
        match target_repository.object_format {
            crate::workflows::artefacts::candidate::GitObjectFormat::Sha1 => {
                "0000000000000000000000000000000000000000"
            }
            crate::workflows::artefacts::candidate::GitObjectFormat::Sha256 => {
                "0000000000000000000000000000000000000000000000000000000000000000"
            }
        }
    });
    if run_git_capture(
        sandbox,
        &job.job,
        crate::workflows::commit::update_ref_command(
            &transaction.expected_reference,
            &commit,
            Some(old_guard),
            &timestamp,
        ),
        false,
    )
    .await
    .is_err()
    {
        restore_before_reference(state, user_project, initial, target_revision, &journal)?;
        return Err(CommitError::Command);
    }
    transaction.state = CommitTransactionState::ReferenceUpdated {
        commit: commit.clone(),
    };
    persist_transaction(state, job.run_id, attempt_id, transaction.clone())?;
    crate::storage::write_private(&live_index, &target_index)
        .map_err(|_| CommitError::Operational)?;
    Ok(())
}

fn persist_transaction(
    state: &AppState,
    run_id: RunId,
    attempt_id: AttemptId,
    transaction: crate::workflows::commit::CommitTransaction,
) -> Result<(), crate::workflows::commit::CommitError> {
    state
        .workflow_runs
        .mutate(&run_id, |run| {
            run.record_commit_transaction(attempt_id, transaction)
        })
        .map(|_| ())
        .map_err(|_| crate::workflows::commit::CommitError::Operational)
}

fn map_apply_error(
    error: crate::workflows::artefacts::apply::ApplyError,
) -> crate::workflows::commit::CommitError {
    match error {
        crate::workflows::artefacts::apply::ApplyError::Drift
        | crate::workflows::artefacts::apply::ApplyError::Conflict => {
            crate::workflows::commit::CommitError::Preflight
        }
        crate::workflows::artefacts::apply::ApplyError::Integrity => {
            crate::workflows::commit::CommitError::Operational
        }
        _ => crate::workflows::commit::CommitError::Apply,
    }
}

fn restore_before_reference(
    state: &AppState,
    user_project: &std::path::Path,
    initial: &crate::workflows::artefacts::candidate::CandidateRevisionArtefact,
    target: &crate::workflows::artefacts::candidate::CandidateRevisionArtefact,
    journal: &crate::workflows::commit::CommitJournal,
) -> Result<(), crate::workflows::commit::CommitError> {
    crate::workflows::artefacts::CandidateApply::rollback(
        user_project,
        initial,
        target,
        &state.workflow_artefacts,
    )
    .map_err(|_| crate::workflows::commit::CommitError::Operational)?;
    let original = journal
        .read_index_backup("original.index")
        .map_err(|_| crate::workflows::commit::CommitError::Operational)?;
    let index = user_project.join(".git/index");
    if original.is_empty() {
        match std::fs::remove_file(index) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(crate::workflows::commit::CommitError::Operational),
        }
    } else {
        crate::storage::write_private(&index, &original)
            .map_err(|_| crate::workflows::commit::CommitError::Operational)?;
    }
    Ok(())
}

fn current_reference(
    project: &std::path::Path,
) -> Result<String, crate::workflows::commit::CommitError> {
    let output = std::process::Command::new("git")
        .current_dir(project)
        .args(["symbolic-ref", "--quiet", "HEAD"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .map_err(|_| crate::workflows::commit::CommitError::Preflight)?;
    if output.status.success() {
        let reference = String::from_utf8(output.stdout)
            .map_err(|_| crate::workflows::commit::CommitError::Preflight)?;
        let reference = reference.trim();
        if reference.starts_with("refs/heads/")
            && !reference.contains("..")
            && !reference.contains(['\\', ' ', '~', '^', ':', '?', '*', '['])
        {
            return Ok(reference.to_owned());
        }
        return Err(crate::workflows::commit::CommitError::Preflight);
    }
    Ok("HEAD".to_owned())
}

fn git_host_text(
    project: &std::path::Path,
    args: &[&str],
) -> Result<String, crate::workflows::commit::CommitError> {
    let output = std::process::Command::new("git")
        .current_dir(project)
        .args(["--no-optional-locks", "-c", "core.hooksPath=/dev/null"])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|_| crate::workflows::commit::CommitError::Preflight)?;
    if !output.status.success() {
        return Err(crate::workflows::commit::CommitError::Preflight);
    }
    String::from_utf8(output.stdout)
        .map(|text| text.trim().to_owned())
        .map_err(|_| crate::workflows::commit::CommitError::Preflight)
}

fn persist_cleanup(
    state: &AppState,
    run_id: &RunId,
    attempt_id: AttemptId,
    cleanup: crate::workflows::run::AttemptCleanupRecord,
) -> Result<(), StoreError> {
    state
        .workflow_runs
        .mutate(run_id, |run| run.record_cleanup(attempt_id, cleanup))
        .map(|_| ())
}

async fn dispatch_step(
    state: &AppState,
    job: &WorkflowJob,
    step: &StepDefinition,
    sandbox: &std::sync::Arc<GuestSandbox>,
    drafts: std::sync::Arc<std::sync::Mutex<crate::workflows::artefacts::output::OutputDrafts>>,
) -> StepOutcome {
    match &step.action {
        StepAction::Agent(action) => {
            run_agent_step(state, job, action, Some(sandbox), drafts).await
        }
        StepAction::SystemCommand(action) => match action.command {
            SystemCommandId::ApplyChanges | SystemCommandId::CommitCandidate => {
                StepOutcome::Failed {
                    category: FailureCategory::Definition,
                    error: Some(OPERATIONAL_STORE_ERROR.to_owned()),
                }
            }
            SystemCommandId::RepositoryStatus => {
                run_system_exec(sandbox, &job.job, guest_command(action.command)).await
            }
        },
        StepAction::HumanGate(_) => StepOutcome::Failed {
            category: FailureCategory::Definition,
            error: Some(OPERATIONAL_STORE_ERROR.to_owned()),
        },
    }
}

fn step_is_host(run: &crate::workflows::WorkflowRun, step: &StepDefinition) -> bool {
    run.phase_settings(&step.key)
        .is_some_and(|settings| settings.location == crate::execution::ToolLocation::Host)
        || matches!(&step.action, StepAction::Agent(action) if action.host_tools())
}

fn host_sandbox_record() -> crate::workflows::run::AttemptSandboxRecord {
    crate::workflows::run::AttemptSandboxRecord {
        kind: crate::workflows::run::AttemptSandboxKind::HostExecution,
        snapshot_digest: crate::environments::SnapshotDigest::parse(&format!(
            "sha256:{}",
            "0".repeat(64)
        ))
        .expect("host snapshot digest"),
    }
}

async fn run_agent_step(
    state: &AppState,
    job: &WorkflowJob,
    action: &AgentStep,
    sandbox: Option<&std::sync::Arc<GuestSandbox>>,
    drafts: std::sync::Arc<std::sync::Mutex<crate::workflows::artefacts::output::OutputDrafts>>,
) -> StepOutcome {
    if job.authority.is_some()
        && let Err(error) = confirm_run_authority(state, job)
    {
        return StepOutcome::Failed {
            category: FailureCategory::Authority,
            error: Some(error),
        };
    }
    let Some(run) = state.workflow_runs.get(&job.run_id) else {
        return StepOutcome::Failed {
            category: FailureCategory::Operational,
            error: Some(OPERATIONAL_STORE_ERROR.to_owned()),
        };
    };
    let Some(step_key) = run
        .active_attempt()
        .and_then(|attempt| run.attempts.iter().find(|item| item.id == attempt))
        .map(|attempt| attempt.step.clone())
    else {
        return StepOutcome::Failed {
            category: FailureCategory::Operational,
            error: Some(OPERATIONAL_STORE_ERROR.to_owned()),
        };
    };
    let Some(step_definition) = run.pinned.definition.step(&step_key) else {
        return StepOutcome::Failed {
            category: FailureCategory::Operational,
            error: Some(OPERATIONAL_STORE_ERROR.to_owned()),
        };
    };
    let connection = match phase_connection(state, job, &run, step_definition) {
        Ok(Some(connection)) => connection,
        Ok(None) => {
            return StepOutcome::Failed {
                category: FailureCategory::Provider,
                error: Some("The model phase has no provider selection.".to_owned()),
            };
        }
        Err(error) => {
            return StepOutcome::Failed {
                category: FailureCategory::Provider,
                error: Some(error),
            };
        }
    };
    set_active_connection(job, Some(connection));
    let resolved_authority = match phase_authority(state, job, &run, &step_key) {
        Ok(authority) => authority,
        Err(error) => {
            return StepOutcome::Failed {
                category: FailureCategory::Authority,
                error: Some(error),
            };
        }
    };
    if let Some(authority) = resolved_authority.as_ref() {
        if !action
            .authority
            .allowed_by(&authority.tools, authority.directories())
            || (action.candidate_authority.access().is_writable()
                && !authority.grant_access.is_writable())
        {
            return StepOutcome::Failed {
                category: FailureCategory::Authority,
                error: Some(
                    "The pinned phase authority exceeds the current conversation ceiling."
                        .to_owned(),
                ),
            };
        }
    } else if let Some(record) = job.agent_id.and_then(|id| state.agents.get(&id)) {
        let directories: Vec<(String, AccessMode)> = record
            .directories
            .iter()
            .map(|grant| (grant.alias.clone(), grant.access))
            .collect();
        if !action.authority.allowed_by(
            &record.tools,
            directories
                .iter()
                .map(|(alias, access)| (alias.as_str(), *access)),
        ) {
            return StepOutcome::Failed {
                category: FailureCategory::Authority,
                error: Some(
                    "The pinned step authority exceeds the current agent ceiling.".to_owned(),
                ),
            };
        }
    }
    let phase_directory_authority = if let Some(authority) = job.project_free_authority.as_ref() {
        match run.phase_settings(&step_key) {
            Some(settings) => match crate::execution::ProjectFreeAuthority::from_settings(
                authority.revision,
                settings,
            ) {
                Ok(authority) => Some(authority),
                Err(_) => {
                    return StepOutcome::Failed {
                        category: FailureCategory::Authority,
                        error: Some(
                            "A phase directory changed identity before dispatch.".to_owned(),
                        ),
                    };
                }
            },
            None => Some(authority.clone()),
        }
    } else {
        None
    };
    let phase_policy = phase_directory_authority
        .as_ref()
        .map(|authority| &authority.policy)
        .or_else(|| {
            resolved_authority
                .as_ref()
                .map(|authority| &authority.policy)
        })
        .unwrap_or(&job.host_policy);
    let policy = if let Some(authority) = phase_directory_authority.as_ref() {
        let grants = phase_policy
            .grants()
            .iter()
            .cloned()
            .map(|mut grant| {
                if action.candidate_authority == CandidateAuthority::Edit
                    && authority.reviewed_aliases.contains(&grant.alias)
                {
                    grant.access = AccessMode::ReadWrite;
                }
                grant
            })
            .collect();
        DirectoryPolicy::from_grants_with_workspace(grants, phase_policy.primary_alias().to_owned())
    } else {
        match intersect_authority(action.candidate_authority, &action.authority, phase_policy) {
            Ok(policy) => policy,
            Err(()) => {
                return StepOutcome::Failed {
                    category: FailureCategory::Authority,
                    error: Some(
                        "The pinned step authority exceeds the current directory policy."
                            .to_owned(),
                    ),
                };
            }
        }
    };
    let Some(role) = run.pinned.definition.role(&action.role).cloned() else {
        return StepOutcome::Failed {
            category: FailureCategory::Definition,
            error: Some(OPERATIONAL_STORE_ERROR.to_owned()),
        };
    };
    let agent_instructions = run
        .phase_model(&step_key)
        .map(|selection| {
            // Quick task roles already contain the instruction snapshot.
            if run.kind == crate::workflows::RunKind::QuickTask {
                String::new()
            } else {
                selection.instructions.clone()
            }
        })
        .unwrap_or_else(|| {
            if run.kind == crate::workflows::RunKind::QuickTask {
                String::new()
            } else if let Some(conversation_id) = job.conversation_id {
                state
                    .conversations
                    .get(&conversation_id)
                    .and_then(|record| record.model)
                    .map(|model| model.settings.instructions)
                    .or_else(|| {
                        job.agent_id
                            .and_then(|id| state.agents.get(&id))
                            .map(|record| record.instructions)
                    })
                    .unwrap_or_default()
            } else {
                job.agent_id
                    .and_then(|id| state.agents.get(&id))
                    .map(|record| record.instructions)
                    .unwrap_or_default()
            }
        });
    let instructions = match (
        role.prompt_defaults.trim().is_empty(),
        agent_instructions.trim().is_empty(),
    ) {
        (true, true) => String::new(),
        (false, true) => role.prompt_defaults.clone(),
        (true, false) => agent_instructions,
        (false, false) => format!(
            "{}

{}",
            role.prompt_defaults.trim(),
            agent_instructions.trim()
        ),
    };
    let connection = job.active_connection();
    let secret = match &connection.auth {
        crate::providers::AuthMethod::ApiKey => Some(connection.api_key.expose()),
        crate::providers::AuthMethod::Plan => None,
    };
    let directory_instructions = match sandbox {
        None => Ok(crate::workflows::input_context::ProjectInstructions::Absent),
        Some(sandbox) => {
            if let Some(authority) = phase_directory_authority.as_ref() {
                crate::workflows::input_context::read_directory_instructions(
                    sandbox, authority, secret,
                )
                .await
            } else {
                crate::workflows::input_context::read_project_instructions(sandbox, secret).await
            }
        }
    };
    let project_instructions = match directory_instructions {
        Ok(instructions) => instructions,
        Err(error) => {
            return StepOutcome::Failed {
                category: FailureCategory::Authority,
                error: Some(error.message().to_owned()),
            };
        }
    };
    let Some(run) = state.workflow_runs.get(&job.run_id) else {
        return StepOutcome::Failed {
            category: FailureCategory::Operational,
            error: Some(OPERATIONAL_STORE_ERROR.to_owned()),
        };
    };
    let inputs = run
        .attempts
        .last()
        .map(|attempt| attempt.inputs.clone())
        .unwrap_or_default();
    let instructions = if instructions.trim().is_empty() {
        String::new()
    } else {
        instructions.trim().to_owned()
    };
    let mut composed = crate::agents::compose_role(
        &role.name,
        &role.expertise,
        &instructions,
        &action.authority.tools,
        &policy,
    );
    if let Some(language) = state.sessions.language(&job.session_id) {
        language.append_instructions(&mut composed);
    }
    let location = run
        .phase_settings(&step_key)
        .map(|settings| settings.location)
        .unwrap_or(crate::execution::ToolLocation::Sandbox);
    if location == crate::execution::ToolLocation::Host {
        composed.push_str("\n\nTools run on this computer as the Power Plant process user. ");
        if run
            .phase_settings(&step_key)
            .is_some_and(|settings| settings.automatic_host_commands())
        {
            composed.push_str("This run authorises Run without approval. ");
        } else {
            composed.push_str("Each shell command waits for user approval bound to this run. ");
        }
        composed.push_str("Approval does not inspect script internals. Command output is sent to the hosted model. Sandbox guest paths such as /access/<alias> and /workspace from earlier turns are not host paths and grant no authority.");
    }
    let request_tools = crate::tools::definitions_for_step(
        &action.authority.tools,
        &action.required_outputs,
        location,
    );
    let model_context_limit = state
        .models_dev
        .context_limit(connection.kind, &connection.model);
    let packet = match crate::workflows::input_context::build_attempt_packet_for_request(
        &run,
        step_definition,
        &inputs,
        &state.workflow_artefacts,
        project_instructions,
        &job.turns,
        &composed,
        &request_tools,
        model_context_limit,
        secret,
    ) {
        Ok(packet) => packet,
        Err(error) => {
            return StepOutcome::Failed {
                category: FailureCategory::Definition,
                error: Some(error.message().to_owned()),
            };
        }
    };
    let Some(attempt_id) = run.active_attempt() else {
        return StepOutcome::Failed {
            category: FailureCategory::Operational,
            error: Some(OPERATIONAL_STORE_ERROR.to_owned()),
        };
    };
    if state
        .workflow_runs
        .mutate(&job.run_id, |run| {
            run.record_initial_context(attempt_id, packet.clone())
        })
        .is_err()
    {
        return StepOutcome::Failed {
            category: FailureCategory::Operational,
            error: Some(OPERATIONAL_STORE_ERROR.to_owned()),
        };
    }
    let evidence = crate::workflows::AttemptEvidenceContext::new(
        state.workflow_evidence.clone(),
        run.id,
        attempt_id,
        step_key.as_str(),
    );
    let host = if location == crate::execution::ToolLocation::Host {
        let settings = run
            .phase_settings(&step_key)
            .cloned()
            .or_else(|| run.directory_settings())
            .ok_or_else(|| StepOutcome::Failed {
                category: FailureCategory::Authority,
                error: Some("The pinned host settings are unavailable.".to_owned()),
            });
        let settings = match settings {
            Ok(settings) => settings,
            Err(outcome) => return outcome,
        };
        let Some(conversation) = job.conversation_id else {
            return StepOutcome::Failed {
                category: FailureCategory::Authority,
                error: Some("Host workflow steps need a conversation.".to_owned()),
            };
        };
        Some(crate::tools::HostRunSpec {
            session: job.session_id,
            conversation,
            execution_revision: job.agent_revision,
            directory: crate::execution::command_directory(&settings.directories),
            settings,
            run: Some(job.run_id.as_hex()),
            step: Some(step_key.as_str().to_owned()),
            attempt: Some(attempt_id.as_hex()),
        })
    } else {
        None
    };
    let spec = AgentRunSpec {
        agent_id: job.agent_id,
        revision: 0,
        preamble: packet.prompt.clone(),
        tools: packet.request_tools(),
        tool_ids: packet.tool_ids(),
        policy,
        connection: connection.clone(),
        location,
        sandbox: sandbox.cloned(),
        host,
        output_drafts: Some(drafts),
        required_outputs: action.required_outputs.clone(),
        evidence: Some(evidence.clone()),
    };
    // Conversation workflow output belongs to attempt evidence, not the conversation reply.
    if job.conversation_id.is_some() && run.kind == super::run::RunKind::Configured {
        job.job.set_output_visible(false);
    }
    let turns = packet.request_messages();
    let ended = crate::slices::run_agent_action(state, spec, turns, job.job.clone()).await;
    let terminal_state = match ended.outcome {
        AgentOutcome::Completed => crate::workflows::evidence::TerminalState::Completed,
        AgentOutcome::ProviderFailure | AgentOutcome::ToolFailure => {
            crate::workflows::evidence::TerminalState::Failed
        }
        AgentOutcome::Cancelled => crate::workflows::evidence::TerminalState::Cancelled,
    };
    evidence.terminal(terminal_state, &ended.reply, ended.error.as_deref(), secret);
    if ended.outcome == AgentOutcome::Completed {
        *job.eligible_reply
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = ended.reply.text.clone();
    }
    match ended.outcome {
        AgentOutcome::Completed => StepOutcome::Completed,
        AgentOutcome::ProviderFailure => StepOutcome::Failed {
            category: FailureCategory::Provider,
            error: ended.error,
        },
        AgentOutcome::ToolFailure => StepOutcome::Failed {
            category: FailureCategory::Tool,
            error: ended.error,
        },
        AgentOutcome::Cancelled => StepOutcome::Cancelled,
    }
}

fn record_missing_terminal_evidence(
    state: &AppState,
    job: &WorkflowJob,
    step: &StepDefinition,
    attempt_id: AttemptId,
    outcome: &StepOutcome,
) {
    if state
        .workflow_evidence
        .get(&job.run_id, &attempt_id)
        .is_some_and(|evidence| evidence.terminal.is_some())
    {
        return;
    }
    let evidence = crate::workflows::AttemptEvidenceContext::new(
        state.workflow_evidence.clone(),
        job.run_id,
        attempt_id,
        step.key.as_str(),
    );
    let (terminal_state, error) = terminal_for_outcome(outcome);
    let connection = job.active_connection();
    let secret = match &connection.auth {
        crate::providers::AuthMethod::ApiKey => Some(connection.api_key.expose()),
        crate::providers::AuthMethod::Plan => None,
    };
    evidence.terminal(
        terminal_state,
        &crate::providers::AssistantReply::default(),
        error,
        secret,
    );
}

fn terminal_for_outcome(
    outcome: &StepOutcome,
) -> (crate::workflows::evidence::TerminalState, Option<&str>) {
    match outcome {
        StepOutcome::Completed => (crate::workflows::evidence::TerminalState::Completed, None),
        StepOutcome::Failed { error, .. } => (
            crate::workflows::evidence::TerminalState::Failed,
            error.as_deref(),
        ),
        StepOutcome::Cancelled => (crate::workflows::evidence::TerminalState::Cancelled, None),
    }
}

async fn run_git_capture(
    sandbox: &GuestSandbox,
    job: &Job,
    exec: GuestExec,
    cancellable: bool,
) -> Result<String, crate::workflows::commit::CommitError> {
    let mut session = sandbox
        .exec_cmd(exec)
        .await
        .map_err(|_| crate::workflows::commit::CommitError::Command)?;
    let deadline = Instant::now() + COMMAND_DEADLINE;
    let mut output = String::new();
    let mut exit = None;
    loop {
        if cancellable && job.cancel_requested() {
            session.kill().await;
            session.close().await;
            return Err(crate::workflows::commit::CommitError::Operational);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            session.kill().await;
            session.close().await;
            return Err(crate::workflows::commit::CommitError::Command);
        }
        let event = if cancellable {
            tokio::select! {
                _ = job.cancelled() => {
                    session.kill().await;
                    session.close().await;
                    return Err(crate::workflows::commit::CommitError::Operational);
                }
                _ = tokio::time::sleep(remaining) => {
                    session.kill().await;
                    session.close().await;
                    return Err(crate::workflows::commit::CommitError::Command);
                }
                event = session.recv() => event,
            }
        } else {
            tokio::select! {
                _ = tokio::time::sleep(remaining) => {
                    session.kill().await;
                    session.close().await;
                    return Err(crate::workflows::commit::CommitError::Command);
                }
                event = session.recv() => event,
            }
        };
        let Some(event) = event else {
            break;
        };
        match event {
            CommandEvent::Output(text) => {
                if output.len().saturating_add(text.len()) > COMMAND_OUTPUT_LIMIT {
                    session.kill().await;
                    session.close().await;
                    return Err(crate::workflows::commit::CommitError::Command);
                }
                output.push_str(&text);
            }
            CommandEvent::Exited(code) => exit = Some(code),
            CommandEvent::Failed => {
                session.close().await;
                return Err(crate::workflows::commit::CommitError::Command);
            }
        }
    }
    session.close().await;
    if exit != Some(0) {
        return Err(crate::workflows::commit::CommitError::Command);
    }
    Ok(output)
}

async fn run_system_exec(sandbox: &GuestSandbox, job: &Job, exec: GuestExec) -> StepOutcome {
    let mut session = match sandbox.exec_cmd(exec).await {
        Ok(session) => session,
        Err(_) => {
            return StepOutcome::Failed {
                category: FailureCategory::Command,
                error: Some("Power Plant could not run the command. Try again.".to_owned()),
            };
        }
    };
    let deadline = Instant::now() + COMMAND_DEADLINE;
    let mut drained = 0usize;
    let mut exit = None;
    loop {
        if job.cancel_requested() {
            session.kill().await;
            session.close().await;
            return StepOutcome::Cancelled;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            session.kill().await;
            session.close().await;
            return StepOutcome::Failed {
                category: FailureCategory::Command,
                error: Some("The command did not finish in time.".to_owned()),
            };
        }
        let event = tokio::select! {
            biased;
            _ = job.cancelled() => {
                session.kill().await;
                session.close().await;
                return StepOutcome::Cancelled;
            }
            _ = tokio::time::sleep(remaining) => {
                session.kill().await;
                session.close().await;
                return StepOutcome::Failed {
                    category: FailureCategory::Command,
                    error: Some("The command did not finish in time.".to_owned()),
                };
            }
            event = session.recv() => event,
        };
        let Some(event) = event else {
            break;
        };
        match event {
            CommandEvent::Output(text) => {
                drained = drained.saturating_add(text.len());
                if drained > COMMAND_OUTPUT_LIMIT {
                    session.kill().await;
                    session.close().await;
                    return StepOutcome::Failed {
                        category: FailureCategory::Command,
                        error: Some("The command output was too large.".to_owned()),
                    };
                }
            }
            CommandEvent::Exited(code) => {
                exit = Some(code);
                break;
            }
            CommandEvent::Failed => {
                session.close().await;
                return StepOutcome::Failed {
                    category: FailureCategory::Command,
                    error: Some("Power Plant could not run the command. Try again.".to_owned()),
                };
            }
        }
    }
    session.close().await;
    match exit {
        Some(0) => StepOutcome::Completed,
        _ => StepOutcome::Failed {
            category: FailureCategory::Command,
            error: Some("The command did not succeed.".to_owned()),
        },
    }
}

pub(crate) fn guest_command(command: SystemCommandId) -> GuestExec {
    match command {
        SystemCommandId::RepositoryStatus => GuestExec::command(
            "git",
            vec!["status".to_owned(), "--porcelain=v1".to_owned()],
        )
        .in_dir(GUEST_PROJECT),
        SystemCommandId::ApplyChanges => GuestExec::command("true", Vec::new()),
        SystemCommandId::CommitCandidate => {
            GuestExec::command("git", Vec::new()).in_dir(GUEST_PROJECT)
        }
    }
}

fn intersect_authority(
    candidate_authority: CandidateAuthority,
    authority: &AgentAuthority,
    host: &DirectoryPolicy,
) -> Result<DirectoryPolicy, ()> {
    let primary = host
        .grants()
        .iter()
        .find(|grant| grant.alias == host.primary_alias())
        .ok_or(())?;
    if candidate_authority.access().is_writable() && !primary.access.is_writable() {
        return Err(());
    }
    let mut grants = vec![PolicyGrant {
        alias: primary.alias.clone(),
        guest_path: primary.guest_path.clone(),
        host_path: primary.host_path.clone(),
        access: candidate_authority.access(),
    }];
    for directory in &authority.directories {
        if directory.alias == host.primary_alias() {
            return Err(());
        }
        let Some(host_grant) = host
            .grants()
            .iter()
            .find(|grant| grant.alias == directory.alias)
        else {
            return Err(());
        };
        if directory.access.is_writable() {
            return Err(());
        }
        grants.push(PolicyGrant {
            alias: host_grant.alias.clone(),
            guest_path: host_grant.guest_path.clone(),
            host_path: host_grant.host_path.clone(),
            access: AccessMode::ReadOnly,
        });
    }
    Ok(DirectoryPolicy::from_grants(
        grants,
        host.primary_alias().to_owned(),
    ))
}

async fn start_attempt_sandbox(
    state: &AppState,
    job: &WorkflowJob,
    step: &StepDefinition,
    capabilities: &crate::workflows::capabilities::AttemptCapabilities,
    workspace: &crate::workflows::workspace::AttemptWorkspace,
    sandbox: std::sync::Arc<GuestSandbox>,
) -> Result<(), &'static str> {
    if job.job.cancel_requested() {
        return Err("The task was cancelled.");
    }
    let run = state
        .workflow_runs
        .get(&job.run_id)
        .ok_or(OPERATIONAL_STORE_ERROR)?;
    let set = &run.environments;
    let binding = set
        .steps
        .iter()
        .find(|item| item.step == step.key)
        .ok_or(OPERATIONAL_STORE_ERROR)?;
    let environment = set
        .environments
        .iter()
        .find(|item| item.environment_id == binding.environment_id)
        .ok_or(OPERATIONAL_STORE_ERROR)?;
    state
        .environment_snapshots
        .matches_pin(&environment.snapshot)
        .await
        .map_err(|_| "That environment snapshot is unavailable.")?;
    let path = state
        .environment_snapshots
        .restore_path(&environment.snapshot.artifact_key)
        .map_err(|_| "That environment snapshot is unavailable.")?;
    let user_project = job
        .host_policy
        .grants()
        .iter()
        .find(|grant| grant.alias == job.host_policy.primary_alias());
    let spec = if let Some(authority) = job.project_free_authority.as_ref() {
        if capabilities.source_location
            == crate::workflows::capabilities::PrimarySourceLocation::UserProject
            && capabilities.git_admin.is_writable()
        {
            let grant = authority
                .policy
                .grants()
                .iter()
                .find(|grant| grant.alias == authority.policy.primary_alias())
                .ok_or("Choose a Git destination.")?;
            crate::workflows::artefacts::inspect_supported_worktree(&grant.host_path)
                .map_err(|_| "The Git destination is not a supported Git worktree.")?;
            commit_attempt_spec(capabilities, &grant.host_path)?
        } else {
            let spec = project_free_attempt_spec(capabilities, workspace, authority)?;
            for grant in authority.policy.grants() {
                crate::sandbox::confirm_host_write_access(&spec, &grant.host_path, grant.access)
                    .map_err(|error| error.message())?;
            }
            spec
        }
    } else if capabilities.source_location
        == crate::workflows::capabilities::PrimarySourceLocation::UserProject
    {
        let user_project = user_project.ok_or("Choose a project directory.")?;
        commit_attempt_spec(capabilities, &user_project.host_path)?
    } else {
        let user_project = user_project.ok_or("Choose a project directory.")?;
        let spec = attempt_spec(
            capabilities,
            workspace,
            &user_project.host_path,
            &job.host_policy,
        )?;
        crate::sandbox::confirm_host_write_access(
            &spec,
            &user_project.host_path,
            AccessMode::ReadOnly,
        )
        .map_err(|error| error.message())?;
        spec
    };
    if job.job.cancel_requested() {
        return Err("The task was cancelled.");
    }
    if job.project_free_authority.is_some() {
        confirm_run_authority(state, job)
            .map_err(|_| "The conversation directory authority changed before execution.")?;
    }
    sandbox
        .start_from_snapshot(&path, environment.snapshot.snapshot_digest.as_str(), spec)
        .await
        .map_err(|error| error.message())?;
    Ok(())
}

fn project_free_attempt_spec(
    capabilities: &crate::workflows::capabilities::AttemptCapabilities,
    workspace: &crate::workflows::workspace::AttemptWorkspace,
    authority: &crate::execution::ProjectFreeAuthority,
) -> Result<crate::sandbox::SandboxSpec, &'static str> {
    let mut mounts = vec![crate::sandbox::MountSpec {
        guest: crate::execution::GUEST_WORKSPACE.to_owned(),
        host: workspace.project.clone(),
        read_only: false,
    }];
    for directory in &capabilities.directories {
        if directory.guest_path == crate::execution::GUEST_WORKSPACE {
            continue;
        }
        let grant = authority
            .policy
            .grants()
            .iter()
            .find(|grant| {
                grant.alias == directory.alias && grant.guest_path == directory.guest_path
            })
            .ok_or("The pinned step authority exceeds the current directory policy.")?;
        let reviewed = authority.reviewed_aliases.contains(&directory.alias);
        let captured = capabilities.source_location
            == crate::workflows::capabilities::PrimarySourceLocation::AttemptWorkspace
            && (reviewed || (authority.reviewed_aliases.is_empty() && !grant.access.is_writable()));
        if directory.access.is_writable() && !(captured || grant.access.is_writable()) {
            return Err("Host write access requires an authorised Direct write grant.");
        }
        mounts.push(crate::sandbox::MountSpec {
            guest: grant.guest_path.clone(),
            host: if captured {
                workspace
                    .reviewed_root(&directory.alias)
                    .map_err(|_| "Power Plant cannot create a reviewed root workspace.")?
            } else {
                grant.host_path.clone()
            },
            read_only: !directory.access.is_writable(),
        });
    }
    Ok(crate::sandbox::SandboxSpec {
        mounts,
        workdir: capabilities
            .primary()
            .map(|directory| directory.guest_path.clone())
            .unwrap_or_else(|| crate::execution::GUEST_WORKSPACE.to_owned()),
        network: capabilities.sandbox_network(),
    })
}

fn commit_attempt_spec(
    capabilities: &crate::workflows::capabilities::AttemptCapabilities,
    user_project: &std::path::Path,
) -> Result<crate::sandbox::SandboxSpec, &'static str> {
    let Some(primary) = capabilities.primary() else {
        return Err("A sandbox-backed step needs a primary source.");
    };
    let git = user_project.join(".git");
    if !git.is_dir() {
        return Err("The project is not a supported Git worktree.");
    }
    Ok(crate::sandbox::SandboxSpec {
        mounts: vec![
            crate::sandbox::MountSpec {
                guest: primary.guest_path.clone(),
                host: user_project.to_path_buf(),
                read_only: false,
            },
            crate::sandbox::MountSpec {
                guest: format!("{}/.git", primary.guest_path),
                host: git,
                read_only: false,
            },
        ],
        workdir: primary.guest_path.clone(),
        network: capabilities.sandbox_network(),
    })
}

fn attempt_spec(
    capabilities: &crate::workflows::capabilities::AttemptCapabilities,
    workspace: &crate::workflows::workspace::AttemptWorkspace,
    user_project: &std::path::Path,
    host: &DirectoryPolicy,
) -> Result<crate::sandbox::SandboxSpec, &'static str> {
    let mut mounts = Vec::new();
    let Some(primary) = capabilities.primary() else {
        return Err("A sandbox-backed step needs a primary source.");
    };
    mounts.push(crate::sandbox::MountSpec {
        guest: primary.guest_path.clone(),
        host: workspace.project.clone(),
        read_only: !primary.access.is_writable(),
    });
    let git = user_project.join(".git");
    if git.is_dir() {
        // The guest cannot create a nested mount directory inside a read-only source mount.
        std::fs::create_dir(workspace.project.join(".git"))
            .map_err(|_| "Power Plant cannot create the Git mount directory.")?;
        mounts.push(crate::sandbox::MountSpec {
            guest: format!("{}/.git", primary.guest_path),
            host: git,
            read_only: true,
        });
    }
    for directory in &capabilities.directories {
        if directory.role != crate::workflows::capabilities::DirectoryRole::SecondaryContext {
            continue;
        }
        let Some(grant) = host
            .grants()
            .iter()
            .find(|grant| grant.alias == directory.alias)
        else {
            return Err("The pinned step authority exceeds the current directory policy.");
        };
        mounts.push(crate::sandbox::MountSpec {
            guest: directory.guest_path.clone(),
            host: grant.host_path.clone(),
            read_only: true,
        });
    }
    Ok(crate::sandbox::SandboxSpec {
        mounts,
        workdir: primary.guest_path.clone(),
        network: capabilities.sandbox_network(),
    })
}

fn confirm_run_authority(
    state: &AppState,
    job: &WorkflowJob,
) -> Result<std::path::PathBuf, String> {
    if let Some(authority) = job.project_free_authority.as_ref() {
        let Some(conversation_id) = job.conversation_id else {
            return Err("The private workspace authority is missing its conversation.".to_owned());
        };
        let Some(record) = state.conversations.get(&conversation_id) else {
            return Err("That conversation is not in the catalogue.".to_owned());
        };
        let run = state
            .workflow_runs
            .get(&job.run_id)
            .ok_or_else(|| "The workflow run is unavailable.".to_owned())?;
        if run.conversation_id != Some(conversation_id) || run.pending_handoff.is_some() {
            return Err(
                "The prepared changes belong to another conversation or need recovery.".to_owned(),
            );
        }
        let settings = run
            .directory_settings()
            .or_else(|| record.model.as_ref().map(|model| model.settings.clone()))
            .ok_or_else(|| "The conversation settings are unavailable.".to_owned())?;
        let pinned_authority =
            crate::execution::ProjectFreeAuthority::from_settings(authority.revision, &settings)
                .map_err(|_| "A pinned directory changed identity before dispatch.".to_owned())?;
        if pinned_authority != *authority {
            return Err("The pinned directory authority changed before dispatch.".to_owned());
        }
        let mut phase_settings: Vec<_> = run
            .model_phases()
            .filter_map(|phase| phase.settings.as_ref())
            .collect();
        if phase_settings.is_empty() {
            phase_settings.push(&settings);
        }
        if !state.sessions.contains_live(&job.session_id)
            || phase_settings.into_iter().any(|settings| {
                if state.access_consent.authorised_launch(
                    job.run_id,
                    job.session_id,
                    conversation_id,
                    settings,
                ) {
                    return false;
                }
                // Configured overrides need run-bound consent, including tool-only and network-only expansions.
                if run.kind == crate::workflows::run::RunKind::Configured {
                    return true;
                }
                if settings.location == crate::execution::ToolLocation::Host {
                    return settings.host_tools()
                        && !state.access_consent.authorised_host_conversation(
                            job.session_id,
                            conversation_id,
                            settings,
                        );
                }
                settings.directories.iter().any(|grant| {
                    (grant.access != crate::execution::DirectoryAccess::ReadOnly
                        || crate::execution::authority::sensitive_directory(
                            &grant.host_path,
                            state.local_data.root(),
                        ))
                        && !state.access_consent.authorised_conversation(
                            job.session_id,
                            conversation_id,
                            settings,
                            grant,
                        )
                        && !state.conversations.directory_approved(
                            &conversation_id,
                            settings,
                            grant,
                        )
                })
            })
        {
            return Err("Directory access needs explicit approval.".to_owned());
        }
        if !authority.policy.is_private_workspace() || job.agent_id.is_some() {
            return Err("The conversation tool authority changed before dispatch.".to_owned());
        }
        return authority
            .reviewed_aliases
            .first()
            .and_then(|alias| {
                authority
                    .policy
                    .grants()
                    .iter()
                    .find(|grant| &grant.alias == alias)
                    .map(|grant| grant.host_path.clone())
            })
            .map_or_else(|| Ok(std::path::PathBuf::new()), Ok);
    }
    Err("That project is not in the catalogue.".to_owned())
}

async fn capture_initial_source(state: &AppState, job: &WorkflowJob) -> Result<(), String> {
    let host_path = confirm_run_authority(state, job)?;
    let captured = if job.project_free_authority.is_some() {
        let run = state
            .workflow_runs
            .get(&job.run_id)
            .ok_or_else(|| "The workflow run is unavailable.".to_owned())?;
        let conversation = job
            .conversation_id
            .and_then(|id| state.conversations.get(&id));
        let settings = run
            .directory_settings()
            .or_else(|| {
                conversation
                    .as_ref()?
                    .model
                    .as_ref()
                    .map(|model| model.settings.clone())
            })
            .ok_or_else(|| "The reviewed directories are unavailable.".to_owned())?;
        let capture_dirs = {
            let reviewed = run.reviewed_directories();
            if reviewed.is_empty() {
                settings.directories.clone()
            } else {
                reviewed
            }
        };
        crate::workflows::artefacts::CandidateCapture::capture_set(
            &capture_dirs,
            state.local_data.root(),
            &state.workflow_artefacts,
        )
        .map(crate::workflows::artefacts::CandidatePayload::Set)
    } else {
        crate::workflows::artefacts::CandidateCapture::capture_host(
            &host_path,
            &state.workflow_artefacts,
        )
        .map(crate::workflows::artefacts::CandidatePayload::Revision)
    };
    let captured = match captured {
        Ok(captured) => captured,
        Err(error) => return Err(error.message().to_owned()),
    };
    let bytes = captured
        .manifest_bytes()
        .map_err(|error| error.message().to_owned())?;
    let object = state
        .workflow_artefacts
        .publish(&bytes)
        .map_err(|error| error.message().to_owned())?;
    let artefact_hash = crate::workflows::artefacts::artefact_hash_for(
        crate::workflows::definition::ArtefactKind::CandidateRevision,
        crate::workflows::artefacts::CANDIDATE_SCHEMA,
        &bytes,
    );
    let id = crate::workflows::id::ArtefactId::generate()
        .map_err(|_| OPERATIONAL_STORE_ERROR.to_owned())?;
    let record = crate::workflows::artefacts::ArtefactRecord {
        id,
        kind: crate::workflows::definition::ArtefactKind::CandidateRevision,
        artefact_hash,
        object_hash: object,
        payload_bytes: bytes.len() as u64,
        created_at_ms: now_ms(),
        provenance: crate::workflows::artefacts::ArtefactProvenance {
            run_id: job.run_id,
            producer: crate::workflows::artefacts::ArtefactProducer::RunSourceCapture,
            inputs: Vec::new(),
        },
        summary: crate::workflows::artefacts::ArtefactSummary::Candidate {
            candidate: captured.candidate_hash(),
            entries: captured.entry_count(),
            bytes: captured.byte_count(),
            disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
        },
    };
    state
        .workflow_runs
        .mutate(&job.run_id, |run| run.record_initial_candidate(record))
        .map(|_| ())
        .map_err(|_| OPERATIONAL_STORE_ERROR.to_owned())?;
    Ok(())
}

fn persist_initial_fail(state: &AppState, run_id: &RunId) -> Result<(), StoreError> {
    let at_ms = now_ms();
    state
        .workflow_runs
        .mutate(run_id, |run| run.fail_before_attempt(at_ms))
        .map(|_| ())
}

fn resolve_inputs(
    run: &crate::workflows::WorkflowRun,
    step: &StepDefinition,
) -> Result<Vec<super::run::AttemptArtefactInput>, &'static str> {
    let mut inputs = Vec::new();
    for input in &step.inputs {
        let artefact = match &input.source {
            crate::workflows::definition::ArtefactSource::RunInitialCandidate => {
                let crate::workflows::RunSource::Captured { source } = &run.source else {
                    return Err("Source capture has not finished.");
                };
                source.initial.clone()
            }
            crate::workflows::definition::ArtefactSource::RunCurrentCandidate => {
                let crate::workflows::RunSource::Captured { source } = &run.source else {
                    return Err("Source capture has not finished.");
                };
                source.accepted.clone()
            }
            crate::workflows::definition::ArtefactSource::RunCurrentPlan => {
                run.current_plan().ok_or("The current plan is missing.")?
            }

            crate::workflows::definition::ArtefactSource::StepOutput {
                step: source_step,
                output,
            } => {
                let found = if let Some(attempt) = run.attempts.iter().rev().find(|attempt| {
                    attempt.step == *source_step
                        && matches!(
                            attempt.result,
                            Some(crate::workflows::run::AttemptResult::Completed { .. })
                        )
                }) {
                    attempt
                        .outputs
                        .iter()
                        .find(|item| item.key == *output)
                        .map(|item| item.artefact.clone())
                } else {
                    run.gates
                        .iter()
                        .rev()
                        .find(|gate| gate.step == *source_step && gate.output == *output)
                        .and_then(|gate| gate.decision.clone())
                };
                let Some(found) = found else {
                    if output.as_str() == crate::workflows::definition::ASSISTANT_REPLY {
                        return Err("Assistant replies cannot be artefact inputs.");
                    }
                    return Err("That input names an unknown output.");
                };
                found
            }
        };
        if artefact.kind != input.kind {
            return Err("That input kind does not match the named output.");
        }
        inputs.push(super::run::AttemptArtefactInput {
            key: input.key.clone(),
            artefact,
        });
    }
    Ok(inputs)
}

fn reject_stale_assurance(
    state: &AppState,
    run: &crate::workflows::WorkflowRun,
    inputs: &[super::run::AttemptArtefactInput],
) -> Result<(), &'static str> {
    let Some(candidate) = inputs
        .iter()
        .find(|input| {
            input.artefact.kind == crate::workflows::definition::ArtefactKind::CandidateRevision
        })
        .and_then(|input| run.artefact(&input.artefact.id))
        .and_then(candidate_hash_of)
    else {
        return Ok(());
    };
    for input in inputs {
        if !input.artefact.kind.is_assurance() {
            continue;
        }
        let Some(record) = run.artefact(&input.artefact.id) else {
            return Err("That assurance artefact is missing.");
        };
        let Ok(bytes) = state.workflow_artefacts.get(&record.object_hash) else {
            return Err("That assurance artefact is missing.");
        };
        let Ok(payload) = crate::workflows::artefacts::parse_typed_payload(record.kind, &bytes)
        else {
            return Err("That assurance artefact is unreadable.");
        };
        let Some(bound) = crate::workflows::artefacts::assurance::candidate_constraint(&payload)
        else {
            return Err("That assurance artefact is unreadable.");
        };
        if bound != candidate {
            return Err("That assurance artefact is stale.");
        }
    }
    Ok(())
}

fn candidate_hash_of(
    record: &crate::workflows::artefacts::ArtefactRecord,
) -> Option<crate::workflows::artefacts::CandidateHash> {
    match &record.summary {
        crate::workflows::artefacts::ArtefactSummary::Candidate { candidate, .. }
        | crate::workflows::artefacts::ArtefactSummary::Review { candidate, .. }
        | crate::workflows::artefacts::ArtefactSummary::Test { candidate, .. } => Some(*candidate),
        crate::workflows::artefacts::ArtefactSummary::Plan { .. }
        | crate::workflows::artefacts::ArtefactSummary::PlanDecision { .. } => None,
        crate::workflows::artefacts::ArtefactSummary::HumanDecision { candidate, .. } => {
            Some(*candidate)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn finalise_attempt(
    state: &AppState,
    job: &WorkflowJob,
    step: &StepDefinition,
    attempt_id: AttemptId,
    inputs: &[super::run::AttemptArtefactInput],
    captured: Option<&crate::workflows::artefacts::CandidatePayload>,
    outcome: &StepOutcome,
    published: bool,
) -> Result<(), StoreError> {
    match outcome {
        StepOutcome::Cancelled => persist_cancel(state, &job.run_id),
        StepOutcome::Failed { category, .. } => {
            if step.writes_primary_source() {
                record_observed(state, job, attempt_id, step, inputs, captured)
                    .map_err(|_| StoreError::Persist)?;
            }
            persist_fail(state, &job.run_id, Some(attempt_id), *category)
        }
        StepOutcome::Completed if published => {
            persist_outcome(state, &job.run_id, attempt_id, outcome)
        }
        StepOutcome::Completed => persist_fail(
            state,
            &job.run_id,
            Some(attempt_id),
            FailureCategory::Definition,
        ),
    }
}

fn record_observed(
    state: &AppState,
    job: &WorkflowJob,
    attempt_id: AttemptId,
    step: &StepDefinition,
    inputs: &[super::run::AttemptArtefactInput],
    captured: Option<&crate::workflows::artefacts::CandidatePayload>,
) -> Result<(), &'static str> {
    let Some(captured) = captured else {
        return record_unknown_observed(state, &job.run_id, attempt_id)
            .map_err(|_| OPERATIONAL_STORE_ERROR);
    };
    let record = publish_candidate(
        state,
        job,
        captured,
        crate::workflows::artefacts::ArtefactProducer::StepAttempt {
            attempt_id,
            step: step.key.clone(),
            output: None,
            disposition: crate::workflows::artefacts::ProductionDisposition::ObservedAfterFailure,
        },
        inputs,
    )?;
    let observed = super::run::ObservedCandidate::Exact {
        artefact: super::artefacts::ArtefactReference {
            id: record.id,
            kind: record.kind,
            artefact_hash: record.artefact_hash,
        },
    };
    state
        .workflow_runs
        .mutate(&job.run_id, |run| {
            run.record_attempt_outputs(attempt_id, vec![record], Vec::new(), None, observed)
        })
        .map(|_| ())
        .map_err(|_| OPERATIONAL_STORE_ERROR)
}

fn record_unknown_observed(
    state: &AppState,
    run_id: &RunId,
    attempt_id: AttemptId,
) -> Result<(), StoreError> {
    state
        .workflow_runs
        .mutate(run_id, |run| {
            run.record_attempt_outputs(
                attempt_id,
                Vec::new(),
                Vec::new(),
                None,
                super::run::ObservedCandidate::Unknown,
            )
        })
        .map(|_| ())
}

struct SuccessAttempt {
    id: AttemptId,
    complete: bool,
}

fn publish_success(
    state: &AppState,
    job: &WorkflowJob,
    step: &StepDefinition,
    attempt: SuccessAttempt,
    inputs: &[super::run::AttemptArtefactInput],
    drafts: &std::sync::Mutex<crate::workflows::artefacts::output::OutputDrafts>,
    captured: Option<&crate::workflows::artefacts::CandidatePayload>,
) -> Result<(), &'static str> {
    let SuccessAttempt {
        id: attempt_id,
        complete: complete_attempt,
    } = attempt;
    let source_free = job.project_free_authority.is_some();
    let captured = if source_free {
        captured
    } else {
        Some(captured.ok_or("Power Plant could not capture isolated outputs.")?)
    };
    let writes = step.writes_primary_source();
    let produces_candidate = writes
        || matches!(
            step.command_source_effect(),
            Some(
                crate::workflows::commands::CommandSourceEffect::Apply
                    | crate::workflows::commands::CommandSourceEffect::Commit
            )
        );
    let expected = inputs
        .iter()
        .find(|input| {
            input.artefact.kind == crate::workflows::definition::ArtefactKind::CandidateRevision
        })
        .and_then(|input| {
            state
                .workflow_runs
                .get(&job.run_id)
                .and_then(|run| run.artefact(&input.artefact.id).cloned())
        })
        .and_then(|record| candidate_hash_of(&record));
    if !produces_candidate
        && expected.is_some_and(|hash| captured.is_none_or(|value| hash != value.candidate_hash()))
    {
        return Err("The project changed during that step.");
    }
    let mut artefacts = Vec::new();
    let mut outputs = Vec::new();
    let mut accepted = None;
    let mut observed = match inputs.iter().find(|input| {
        input.artefact.kind == crate::workflows::definition::ArtefactKind::CandidateRevision
    }) {
        Some(input) => super::run::ObservedCandidate::Exact {
            artefact: input.artefact.clone(),
        },
        None => super::run::ObservedCandidate::Unknown,
    };
    if produces_candidate {
        let output = step
            .required_outputs()
            .iter()
            .find(|item| item.kind == crate::workflows::definition::OutputKind::CandidateRevision)
            .ok_or("A source-write step must produce exactly one candidate revision.")?;
        let record = publish_candidate(
            state,
            job,
            captured.ok_or("Power Plant could not capture isolated outputs.")?,
            crate::workflows::artefacts::ArtefactProducer::StepAttempt {
                attempt_id,
                step: step.key.clone(),
                output: Some(output.key.clone()),
                disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
            },
            inputs,
        )?;
        let reference = super::artefacts::ArtefactReference {
            id: record.id,
            kind: record.kind,
            artefact_hash: record.artefact_hash,
        };
        accepted = Some(reference.clone());
        observed = super::run::ObservedCandidate::Exact {
            artefact: reference.clone(),
        };
        outputs.push(super::run::AttemptArtefactOutput {
            key: output.key.clone(),
            artefact: reference,
        });
        artefacts.push(record);
    }
    let candidate_hash = captured.map(|value| value.candidate_hash());
    let connection = job.active_connection();
    let secret = match &connection.auth {
        crate::providers::AuthMethod::ApiKey => Some(connection.api_key.expose().to_owned()),
        crate::providers::AuthMethod::Plan => None,
    };
    let mut held = drafts
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for output in step.required_outputs() {
        if matches!(
            output.kind,
            crate::workflows::definition::OutputKind::AssistantReply
                | crate::workflows::definition::OutputKind::CandidateRevision
        ) {
            continue;
        }
        let Some(draft) = held.take(&output.key) else {
            return Err("A required output is missing.");
        };
        let fixing_report_inputs;
        let provenance_inputs = if output.kind
            == crate::workflows::definition::OutputKind::ReviewReport
            && step.writes_primary_source()
        {
            let produced = accepted
                .clone()
                .ok_or("A fixing review needs a produced candidate revision.")?;
            fixing_report_inputs = std::iter::once(super::run::AttemptArtefactInput {
                key: crate::workflows::definition::InputKey::parse("candidate")
                    .map_err(|_| OPERATIONAL_STORE_ERROR)?,
                artefact: produced,
            })
            .chain(
                inputs
                    .iter()
                    .filter(|input| {
                        input.artefact.kind
                            != crate::workflows::definition::ArtefactKind::CandidateRevision
                    })
                    .cloned(),
            )
            .collect::<Vec<_>>();
            fixing_report_inputs.as_slice()
        } else {
            inputs
        };
        let record = publish_draft(
            state,
            job,
            attempt_id,
            step,
            output,
            draft,
            candidate_hash.ok_or("A structured output needs a candidate source.")?,
            provenance_inputs,
            secret.as_deref(),
        )?;
        outputs.push(super::run::AttemptArtefactOutput {
            key: output.key.clone(),
            artefact: super::artefacts::ArtefactReference {
                id: record.id,
                kind: record.kind,
                artefact_hash: record.artefact_hash,
            },
        });
        artefacts.push(record);
    }
    drop(held);
    state
        .workflow_runs
        .mutate(&job.run_id, |run| {
            run.record_attempt_outputs(attempt_id, artefacts, outputs, accepted, observed)?;
            if complete_attempt {
                run.complete_attempt(attempt_id, now_ms())?;
            }
            Ok(())
        })
        .map(|_| ())
        .map_err(|_| OPERATIONAL_STORE_ERROR)
}

fn publish_candidate(
    state: &AppState,
    job: &WorkflowJob,
    captured: &crate::workflows::artefacts::CandidatePayload,
    producer: crate::workflows::artefacts::ArtefactProducer,
    inputs: &[super::run::AttemptArtefactInput],
) -> Result<crate::workflows::artefacts::ArtefactRecord, &'static str> {
    let bytes = captured
        .manifest_bytes()
        .map_err(|_| OPERATIONAL_STORE_ERROR)?;
    let object = state
        .workflow_artefacts
        .publish(&bytes)
        .map_err(|_| OPERATIONAL_STORE_ERROR)?;
    let artefact_hash = crate::workflows::artefacts::artefact_hash_for(
        crate::workflows::definition::ArtefactKind::CandidateRevision,
        crate::workflows::artefacts::CANDIDATE_SCHEMA,
        &bytes,
    );
    let id = crate::workflows::ArtefactId::generate().map_err(|_| OPERATIONAL_STORE_ERROR)?;
    Ok(crate::workflows::artefacts::ArtefactRecord {
        id,
        kind: crate::workflows::definition::ArtefactKind::CandidateRevision,
        artefact_hash,
        object_hash: object,
        payload_bytes: bytes.len() as u64,
        created_at_ms: now_ms(),
        provenance: crate::workflows::artefacts::ArtefactProvenance {
            run_id: job.run_id,
            producer: producer.clone(),
            inputs: inputs.iter().map(|input| input.artefact.clone()).collect(),
        },
        summary: crate::workflows::artefacts::ArtefactSummary::Candidate {
            candidate: captured.candidate_hash(),
            entries: captured.entry_count(),
            bytes: captured.byte_count(),
            disposition: match producer {
                crate::workflows::artefacts::ArtefactProducer::StepAttempt {
                    disposition, ..
                } => disposition,
                _ => crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
            },
        },
    })
}

#[allow(clippy::too_many_arguments)]
fn publish_draft(
    state: &AppState,
    job: &WorkflowJob,
    attempt_id: AttemptId,
    step: &StepDefinition,
    output: &crate::workflows::definition::RequiredOutput,
    draft: crate::workflows::artefacts::output::OutputDraft,
    candidate: crate::workflows::artefacts::CandidateHash,
    inputs: &[super::run::AttemptArtefactInput],
    secret: Option<&str>,
) -> Result<crate::workflows::artefacts::ArtefactRecord, &'static str> {
    use crate::workflows::artefacts::output::OutputDraft;
    let (bytes, object, artefact_hash, kind, summary) = match draft {
        OutputDraft::Plan { markdown } => {
            let (bytes, object, hash) =
                crate::workflows::artefacts::payload::encode_plan(&markdown, secret)
                    .map_err(|_| "That plan output is not valid.")?;
            (
                bytes,
                object,
                hash,
                crate::workflows::definition::ArtefactKind::Plan,
                crate::workflows::artefacts::ArtefactSummary::Plan {
                    markdown_bytes: markdown.len() as u64,
                },
            )
        }
        OutputDraft::Review { verdict, markdown } => {
            let (bytes, object, hash) = crate::workflows::artefacts::payload::encode_review(
                candidate, verdict, &markdown, secret,
            )
            .map_err(|_| "That review output is not valid.")?;
            (
                bytes,
                object,
                hash,
                crate::workflows::definition::ArtefactKind::ReviewReport,
                crate::workflows::artefacts::ArtefactSummary::Review { candidate, verdict },
            )
        }
        OutputDraft::Test { outcome, markdown } => {
            let (bytes, object, hash) = crate::workflows::artefacts::payload::encode_test(
                candidate, outcome, &markdown, secret,
            )
            .map_err(|_| "That test output is not valid.")?;
            (
                bytes,
                object,
                hash,
                crate::workflows::definition::ArtefactKind::TestReport,
                crate::workflows::artefacts::ArtefactSummary::Test { candidate, outcome },
            )
        }
    };
    state
        .workflow_artefacts
        .publish(&bytes)
        .map_err(|_| OPERATIONAL_STORE_ERROR)?;
    let id = crate::workflows::ArtefactId::generate().map_err(|_| OPERATIONAL_STORE_ERROR)?;
    Ok(crate::workflows::artefacts::ArtefactRecord {
        id,
        kind,
        artefact_hash,
        object_hash: object,
        payload_bytes: bytes.len() as u64,
        created_at_ms: now_ms(),
        provenance: crate::workflows::artefacts::ArtefactProvenance {
            run_id: job.run_id,
            producer: crate::workflows::artefacts::ArtefactProducer::StepAttempt {
                attempt_id,
                step: step.key.clone(),
                output: Some(output.key.clone()),
                disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
            },
            inputs: inputs.iter().map(|input| input.artefact.clone()).collect(),
        },
        summary,
    })
}

fn persist_start(
    state: &AppState,
    run_id: &RunId,
    attempt_id: AttemptId,
    inputs: Vec<super::run::AttemptArtefactInput>,
    capabilities: crate::workflows::capabilities::AttemptCapabilities,
    sandbox: crate::workflows::run::AttemptSandboxRecord,
) -> Result<(), StoreError> {
    let at_ms = now_ms();
    state
        .workflow_runs
        .mutate(run_id, |run| {
            run.start_attempt(attempt_id, inputs, capabilities, sandbox, at_ms)
        })
        .map(|_| ())
}

fn persist_outcome(
    state: &AppState,
    run_id: &RunId,
    attempt_id: AttemptId,
    outcome: &StepOutcome,
) -> Result<(), StoreError> {
    let at_ms = now_ms();
    match outcome {
        StepOutcome::Completed => state
            .workflow_runs
            .mutate(run_id, |run| run.complete_attempt(attempt_id, at_ms))
            .map(|_| ()),
        StepOutcome::Failed { category, .. } => {
            persist_fail(state, run_id, Some(attempt_id), *category)
        }
        StepOutcome::Cancelled => persist_cancel(state, run_id),
    }
}

fn persist_fail(
    state: &AppState,
    run_id: &RunId,
    attempt_id: Option<AttemptId>,
    category: FailureCategory,
) -> Result<(), StoreError> {
    let at_ms = now_ms();
    state
        .workflow_runs
        .mutate(run_id, |run| {
            if let Some(attempt_id) = attempt_id.or(run.active_attempt()) {
                run.fail_attempt(attempt_id, category, at_ms)
            } else {
                Err(crate::workflows::run::TransitionError::Invalid)
            }
        })
        .map(|_| ())
}

fn persist_cancel(state: &AppState, run_id: &RunId) -> Result<(), StoreError> {
    let at_ms = now_ms();
    state
        .workflow_runs
        .mutate(run_id, |run| run.cancel(at_ms))
        .map(|_| ())
}

fn fail_operational(state: &AppState, workflow: &WorkflowJob) {
    settle_job(
        state,
        workflow,
        JobStatus::Failed,
        Some(OPERATIONAL_STORE_ERROR),
    );
}

fn finish_terminal_run(
    state: &AppState,
    job: &mut WorkflowJob,
    run: &crate::workflows::WorkflowRun,
) -> bool {
    let (status, error) = match &run.state {
        crate::workflows::run::RunState::Escalated {
            reason: crate::workflows::run::EscalationReason::Blocked,
            ..
        } => (
            JobStatus::Failed,
            Some("The review blocked this workflow run."),
        ),
        crate::workflows::run::RunState::Escalated {
            reason: crate::workflows::run::EscalationReason::AttemptLimit,
            ..
        } => (
            JobStatus::Failed,
            Some("The review attempt limit escalated this workflow run."),
        ),
        crate::workflows::run::RunState::Cancelled => (JobStatus::Cancelled, None),
        crate::workflows::run::RunState::Failed | crate::workflows::run::RunState::Interrupted => {
            (JobStatus::Failed, None)
        }
        _ => (JobStatus::Completed, None),
    };
    finish_driven_job(state, job, status, error)
}

fn finish_driven_job(
    state: &AppState,
    job: &mut WorkflowJob,
    status: JobStatus,
    error: Option<&str>,
) -> bool {
    match status {
        JobStatus::Completed if error.is_none() => settle_completed_job(state, job),
        _ => settle_job(state, job, status, error),
    }
    true
}

pub(crate) fn settle_completed_job(state: &AppState, workflow: &WorkflowJob) {
    settle_job(state, workflow, JobStatus::Completed, None);
}

pub(crate) fn settle_cancelled_job(state: &AppState, workflow: &WorkflowJob) {
    settle_job(state, workflow, JobStatus::Cancelled, None);
}

fn settle_job(state: &AppState, workflow: &WorkflowJob, status: JobStatus, error: Option<&str>) {
    let eligible = workflow
        .eligible_reply
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let mut reply = workflow.job.snapshot().output;
    if !eligible.is_empty() && reply.activity.is_empty() {
        reply.text = eligible;
    }
    settle_with_reply(state, workflow, status, error, &reply);
}

fn settle_with_reply(
    state: &AppState,
    workflow: &WorkflowJob,
    status: JobStatus,
    error: Option<&str>,
    reply: &crate::providers::AssistantReply,
) {
    let connection = workflow.active_connection();
    let secret = match &connection.auth {
        crate::providers::AuthMethod::ApiKey => Some(connection.api_key.expose()),
        crate::providers::AuthMethod::Plan => None,
    };
    let error = error
        .and_then(|text| crate::providers::sanitise_detail(&crate::tools::redact(text, secret)));
    let reply = crate::slices::bound_reply(reply);
    let conversation_reply = {
        state
            .workflow_runs
            .get(&workflow.run_id)
            .filter(|run| run.kind == super::run::RunKind::Configured)
            .map(|run| {
                let result = if run.completed_without_changes() {
                    "The task completed without changes. No commit was created."
                } else {
                    &reply.text
                };
                conversation_run_result(workflow.run_id, status, result)
            })
    };
    if let Some(conversation_id) = workflow.conversation_id {
        let message_status = match status {
            JobStatus::Completed => crate::conversations::MessageStatus::Complete,
            JobStatus::Cancelled => crate::conversations::MessageStatus::Interrupted,
            JobStatus::Failed | JobStatus::AwaitingDecision | JobStatus::Running => {
                crate::conversations::MessageStatus::Failed
            }
        };
        let _ = state.conversations.settle_message(
            &conversation_id,
            workflow.job.id(),
            conversation_reply
                .map(crate::providers::AssistantReply::from)
                .unwrap_or(reply),
            message_status,
            if message_status == crate::conversations::MessageStatus::Failed {
                error.clone()
            } else {
                None
            },
        );
        crate::conversations::titles::start(
            state,
            conversation_id,
            state.sessions.language(&workflow.session_id),
        );
        let _ = state.sessions.finish_conversation_job(
            &workflow.session_id,
            conversation_id,
            workflow.job.id(),
        );
    } else if let Some(key) = workflow.conversation_key() {
        match status {
            JobStatus::Completed => {
                let _ = state.sessions.finish_turn(
                    &workflow.session_id,
                    &key,
                    &workflow.job.id(),
                    reply,
                );
            }
            _ => {
                let _ =
                    state
                        .sessions
                        .fail_turn(&workflow.session_id, &key, &workflow.job.id(), reply);
            }
        }
    }
    state.host_approvals.invalidate_job(workflow.job.id());
    let _ = workflow.job.finish(status, error.as_deref());
}

fn conversation_run_result(run_id: RunId, status: JobStatus, response: &str) -> String {
    conversation_result(status, response, &format!("/runs/{}", run_id.as_hex()))
}

fn conversation_result(status: JobStatus, response: &str, href: &str) -> String {
    const MAXIMUM_CONCISE_RESULT_BYTES: usize = 2 * 1024;
    let outcome = match status {
        JobStatus::Completed => "completed",
        JobStatus::Cancelled => "was cancelled",
        JobStatus::Failed => "did not complete",
        JobStatus::Running | JobStatus::AwaitingDecision => "is still active",
    };
    let mut response = response.trim().to_owned();
    if response.len() > MAXIMUM_CONCISE_RESULT_BYTES {
        let mut end = MAXIMUM_CONCISE_RESULT_BYTES;
        while end > 0 && !response.is_char_boundary(end) {
            end -= 1;
        }
        response.truncate(end);
        response.push_str("\n[terminal result truncated]");
    }
    let result = if response.is_empty() {
        String::new()
    } else {
        format!("\n\nTerminal response:\n\n{response}")
    };
    format!(
        "Workflow {outcome}.{result}\n\n[Open the run record]({href}) for detailed activity, changes and result."
    )
}

fn recovery_project_path(
    _state: &AppState,
    run: &crate::workflows::WorkflowRun,
) -> Result<std::path::PathBuf, &'static str> {
    let error = "Power Plant could not recover a commit transaction.";
    let Some(settings) = run.directory_settings() else {
        return Err(error);
    };
    let Some(grant) = settings.git_destination_grant() else {
        return Err(error);
    };
    if grant.revalidate().is_err()
        || crate::workflows::artefacts::inspect_supported_worktree(&grant.host_path).is_err()
    {
        return Err(error);
    }
    Ok(grant.host_path.clone())
}

pub(crate) fn recover_apply_transactions(state: &AppState) -> Result<(), &'static str> {
    const ERROR: &str = "Power Plant could not recover a file application transaction.";
    for run in state.workflow_runs.active_runs() {
        let Some(attempt_id) = run.active_attempt() else {
            continue;
        };
        let Some(attempt) = run.attempts.iter().find(|attempt| attempt.id == attempt_id) else {
            return Err(ERROR);
        };
        let Some(mut transaction) = attempt.apply_transaction.clone() else {
            continue;
        };
        let journal = state
            .apply_journals
            .load(run.id, attempt_id)
            .map_err(|_| ERROR)?;
        journal.make_sure_binding(&transaction).map_err(|_| ERROR)?;
        let baseline = load_candidate_payload_reference(state, &run, &transaction.baseline)?;
        let candidate = load_candidate_payload_reference(state, &run, &transaction.candidate)?;
        let baseline_manifest = baseline.manifest_bytes().map_err(|_| ERROR)?;
        journal
            .make_sure_baseline(&baseline_manifest, transaction.baseline.artefact_hash)
            .map_err(|_| ERROR)?;
        let (
            crate::workflows::artefacts::CandidatePayload::Set(baseline),
            crate::workflows::artefacts::CandidatePayload::Set(candidate),
        ) = (&baseline, &candidate)
        else {
            mark_apply_uncertain(state, run.id, attempt_id, transaction)?;
            return Err(ERROR);
        };
        if baseline.roots.len() != transaction.roots.len()
            || candidate.roots.len() != transaction.roots.len()
        {
            mark_apply_uncertain(state, run.id, attempt_id, transaction)?;
            return Err(ERROR);
        }
        let mut operations = Vec::new();
        for ((root, before), after) in transaction
            .roots
            .iter()
            .zip(&baseline.roots)
            .zip(&candidate.roots)
        {
            if root.grant_id != before.grant_id
                || root.grant_id != after.grant_id
                || root.identity != before.identity
                || root.identity != after.identity
                || root.baseline_candidate != before.candidate.candidate_hash
                || root.candidate_hash != after.candidate.candidate_hash
                || root.exclusions != before.candidate.exclusions
                || root.exclusions != after.candidate.exclusions
            {
                mark_apply_uncertain(state, run.id, attempt_id, transaction)?;
                return Err(ERROR);
            }
            operations.extend(
                crate::workflows::artefacts::apply::changed_paths(
                    &before.candidate,
                    &after.candidate,
                )
                .into_iter()
                .map(|path| format!("{}/{}", root.alias, path)),
            );
        }
        let started_paths = journal.started_paths(&operations).map_err(|_| ERROR)?;
        if started_paths.is_empty() {
            let mut recovered = transaction;
            recovered.state = crate::workflows::apply::ApplyTransactionState::Recovered;
            state
                .workflow_runs
                .mutate(&run.id, |run| {
                    run.record_apply_transaction(attempt_id, recovered)
                })
                .map_err(|_| ERROR)?;
            continue;
        }
        transaction.recover_roots(
            baseline,
            candidate,
            &started_paths,
            &state.workflow_artefacts,
        );
        state
            .workflow_runs
            .mutate(&run.id, |run| {
                run.record_apply_transaction(attempt_id, transaction.clone())
            })
            .map_err(|_| ERROR)?;
        if !transaction.is_settled() {
            return Err(ERROR);
        }
        if !transaction.is_verified() {
            continue;
        }
        state
            .workflow_runs
            .mutate(&run.id, |run| {
                run.record_cleanup(
                    attempt_id,
                    crate::workflows::run::AttemptCleanupRecord::Complete,
                )
            })
            .map_err(|_| ERROR)?;
        state
            .workflow_runs
            .mutate(&run.id, |run| run.complete_attempt(attempt_id, now_ms()))
            .map_err(|_| ERROR)?;
    }
    Ok(())
}

fn mark_apply_uncertain(
    state: &AppState,
    run_id: RunId,
    attempt_id: AttemptId,
    mut transaction: crate::workflows::apply::ApplyTransaction,
) -> Result<(), &'static str> {
    transaction.state = crate::workflows::apply::ApplyTransactionState::RecoveryUncertain;
    for root in &mut transaction.roots {
        if root.outcome == crate::workflows::apply::ApplyRootOutcome::Pending {
            root.outcome = crate::workflows::apply::ApplyRootOutcome::Uncertain;
        }
    }
    state
        .workflow_runs
        .mutate(&run_id, |run| {
            run.record_apply_transaction(attempt_id, transaction)
        })
        .map(|_| ())
        .map_err(|_| "Power Plant could not retain uncertain file application evidence.")
}

fn capture_commit_candidate(
    state: &AppState,
    run: &crate::workflows::WorkflowRun,
    template: &crate::workflows::artefacts::CandidatePayload,
) -> Result<crate::workflows::artefacts::CandidatePayload, &'static str> {
    use crate::workflows::artefacts::{CandidateCapture, CandidatePayload};
    let error = "Power Plant could not capture the committed candidate.";
    match template {
        CandidatePayload::Set(_) => {
            let settings = run.directory_settings().ok_or(error)?;
            CandidateCapture::capture_set(
                &settings.directories,
                state.local_data.root(),
                &state.workflow_artefacts,
            )
            .map(CandidatePayload::Set)
            .map_err(|_| error)
        }
        CandidatePayload::Revision(revision) => {
            let project = recovery_project_path(state, run)?;
            crate::workflows::commit::capture_revision(
                &project,
                revision,
                &state.workflow_artefacts,
            )
            .map(CandidatePayload::Revision)
            .map_err(|_| error)
        }
    }
}

pub(crate) fn recover_commit_transactions(state: &AppState) -> Result<(), &'static str> {
    for run in state.workflow_runs.active_runs() {
        let Some(attempt_id) = run.active_attempt() else {
            continue;
        };
        let Some(attempt) = run.attempts.iter().find(|attempt| attempt.id == attempt_id) else {
            return Err("Power Plant could not recover a commit transaction.");
        };
        let Some(transaction) = attempt.commit_transaction.clone() else {
            continue;
        };
        let project = recovery_project_path(state, &run)?;
        if current_reference(&project).ok().as_deref() != Some(&transaction.expected_reference) {
            return Err("Power Plant could not recover a commit transaction.");
        }
        let initial_ref = match &run.source {
            crate::workflows::RunSource::Captured { source } => &source.initial,
            crate::workflows::RunSource::None | crate::workflows::RunSource::Pending => {
                return Err("Power Plant could not recover a commit transaction.");
            }
        };
        let initial_payload = load_candidate_payload_reference(state, &run, initial_ref)?;
        let target_payload = load_candidate_payload_reference(state, &run, &transaction.candidate)?;
        let (initial, target) =
            crate::workflows::commit::destination_pair(&run, &initial_payload, &target_payload)
                .map_err(|_| "Power Plant could not recover a commit transaction.")?;
        if target.repository != initial.repository || target.git_admin != initial.git_admin {
            return Err("Power Plant could not recover a commit transaction.");
        }
        let head = current_head(&project)?;
        let old = transaction.old_object.as_deref();
        let expected = transaction.expected_commit.as_deref();
        if head.as_deref() == old {
            let live = crate::workflows::commit::capture_revision(
                &project,
                initial,
                &state.workflow_artefacts,
            )
            .map_err(|_| "Power Plant could not recover a commit transaction.")?;
            if live.candidate_hash == target.candidate_hash
                && live.repository == initial.repository
                && live.git_admin == initial.git_admin
            {
                let journal = state
                    .commit_journals
                    .load(run.id, attempt_id)
                    .map_err(|_| "Power Plant could not recover a commit transaction.")?;
                restore_before_reference(state, &project, initial, target, &journal)
                    .map_err(|_| "Power Plant could not recover a commit transaction.")?;
            } else if live != *initial {
                return Err("Power Plant could not recover a commit transaction.");
            }
            remove_commit_journal(state, run.id, attempt_id)?;
            continue;
        }
        if head.as_deref() != expected || expected.is_none() {
            return Err("Power Plant could not recover a commit transaction.");
        }
        let journal = state
            .commit_journals
            .load(run.id, attempt_id)
            .map_err(|_| "Power Plant could not recover a commit transaction.")?;
        let target_index = journal
            .read_index_backup("target.index")
            .map_err(|_| "Power Plant could not recover a commit transaction.")?;
        crate::storage::write_private(&project.join(".git/index"), &target_index)
            .map_err(|_| "Power Plant could not recover a commit transaction.")?;
        let captured = capture_commit_candidate(state, &run, &target_payload)?;
        let revision = crate::workflows::commit::destination_revision(&run, &captured)
            .map_err(|_| "Power Plant could not recover a commit transaction.")?;
        let commit = expected.expect("checked").to_owned();
        if captured.candidate_hash() != target_payload.candidate_hash()
            || revision
                .repository
                .as_ref()
                .and_then(|repository| repository.head.as_ref())
                .map(|head| head.0.as_str())
                != Some(commit.as_str())
        {
            return Err("Power Plant could not recover a commit transaction.");
        }
        let mut verified = transaction;
        verified.state = crate::workflows::commit::CommitTransactionState::Verified {
            commit: commit.clone(),
        };
        state
            .workflow_runs
            .mutate(&run.id, |run| {
                run.record_commit_transaction(attempt_id, verified)
            })
            .map_err(|_| "Power Plant could not recover a commit transaction.")?;
        state
            .workflow_runs
            .mutate(&run.id, |run| {
                run.record_commit_result(
                    attempt_id,
                    crate::workflows::commit::CommitResult {
                        commit: commit.clone(),
                    },
                )
            })
            .map_err(|_| "Power Plant could not recover a commit transaction.")?;
        publish_recovered_commit(state, &run, attempt_id, &captured)?;
        remove_commit_journal(state, run.id, attempt_id)?;
        state
            .workflow_runs
            .mutate(&run.id, |run| {
                run.record_cleanup(
                    attempt_id,
                    crate::workflows::run::AttemptCleanupRecord::Complete,
                )
            })
            .map_err(|_| "Power Plant could not recover a commit transaction.")?;
        state
            .workflow_runs
            .mutate(&run.id, |run| run.complete_attempt(attempt_id, now_ms()))
            .map_err(|_| "Power Plant could not recover a commit transaction.")?;
    }
    Ok(())
}

fn load_candidate_payload_reference(
    state: &AppState,
    run: &crate::workflows::WorkflowRun,
    reference: &crate::workflows::artefacts::ArtefactReference,
) -> Result<crate::workflows::artefacts::CandidatePayload, &'static str> {
    let record = run
        .artefact(&reference.id)
        .filter(|record| record.artefact_hash == reference.artefact_hash)
        .ok_or("Power Plant could not recover the transaction candidate.")?;
    let bytes = state
        .workflow_artefacts
        .get(&record.object_hash)
        .map_err(|_| "Power Plant could not recover the transaction candidate.")?;
    crate::workflows::artefacts::CandidatePayload::from_manifest_bytes(&bytes)
        .ok_or("Power Plant could not recover the transaction candidate.")
}

fn current_head(project: &std::path::Path) -> Result<Option<String>, &'static str> {
    let output = std::process::Command::new("git")
        .current_dir(project)
        .args(["rev-parse", "--verify", "HEAD"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .map_err(|_| "Power Plant could not recover a commit transaction.")?;
    if !output.status.success() {
        return Ok(None);
    }
    String::from_utf8(output.stdout)
        .map(|head| Some(head.trim().to_owned()))
        .map_err(|_| "Power Plant could not recover a commit transaction.")
}

fn remove_commit_journal(
    state: &AppState,
    run_id: RunId,
    attempt_id: AttemptId,
) -> Result<(), &'static str> {
    state
        .commit_journals
        .remove(run_id, attempt_id)
        .map_err(|_| "Power Plant could not recover a commit transaction.")
}

fn publish_recovered_commit(
    state: &AppState,
    run: &crate::workflows::WorkflowRun,
    attempt_id: AttemptId,
    captured: &crate::workflows::artefacts::CandidatePayload,
) -> Result<(), &'static str> {
    let attempt = run
        .attempts
        .iter()
        .find(|attempt| attempt.id == attempt_id)
        .ok_or("Power Plant could not recover a commit transaction.")?;
    let step = run
        .pinned
        .definition
        .step(&attempt.step)
        .ok_or("Power Plant could not recover a commit transaction.")?;
    let output = step
        .required_outputs()
        .iter()
        .find(|output| output.kind == crate::workflows::definition::OutputKind::CandidateRevision)
        .ok_or("Power Plant could not recover a commit transaction.")?;
    if !attempt.outputs.is_empty() {
        let existing = attempt
            .outputs
            .iter()
            .find(|existing| existing.key == output.key)
            .ok_or("Power Plant could not recover a commit transaction.")?;
        let stored = load_candidate_payload_reference(state, run, &existing.artefact)?;
        if attempt.outputs.len() == 1 && stored == *captured {
            return Ok(());
        }
        return Err("Power Plant could not recover a commit transaction.");
    }
    let bytes = captured
        .manifest_bytes()
        .map_err(|_| "Power Plant could not recover a commit transaction.")?;
    let object = state
        .workflow_artefacts
        .publish(&bytes)
        .map_err(|_| "Power Plant could not recover a commit transaction.")?;
    let artefact_hash = crate::workflows::artefacts::artefact_hash_for(
        crate::workflows::definition::ArtefactKind::CandidateRevision,
        crate::workflows::artefacts::CANDIDATE_SCHEMA,
        &bytes,
    );
    let id = crate::workflows::ArtefactId::generate()
        .map_err(|_| "Power Plant could not recover a commit transaction.")?;
    let record = crate::workflows::artefacts::ArtefactRecord {
        id,
        kind: crate::workflows::definition::ArtefactKind::CandidateRevision,
        artefact_hash,
        object_hash: object,
        payload_bytes: bytes.len() as u64,
        created_at_ms: now_ms(),
        provenance: crate::workflows::artefacts::ArtefactProvenance {
            run_id: run.id,
            producer: crate::workflows::artefacts::ArtefactProducer::StepAttempt {
                attempt_id,
                step: step.key.clone(),
                output: Some(output.key.clone()),
                disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
            },
            inputs: attempt
                .inputs
                .iter()
                .map(|input| input.artefact.clone())
                .collect(),
        },
        summary: crate::workflows::artefacts::ArtefactSummary::Candidate {
            candidate: captured.candidate_hash(),
            entries: captured.entry_count(),
            bytes: captured.byte_count(),
            disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
        },
    };
    let reference = crate::workflows::artefacts::ArtefactReference {
        id,
        kind: record.kind,
        artefact_hash,
    };
    state
        .workflow_runs
        .mutate(&run.id, |run| {
            run.record_attempt_outputs(
                attempt_id,
                vec![record],
                vec![crate::workflows::run::AttemptArtefactOutput {
                    key: output.key.clone(),
                    artefact: reference.clone(),
                }],
                Some(reference.clone()),
                crate::workflows::run::ObservedCandidate::Exact {
                    artefact: reference,
                },
            )
        })
        .map(|_| ())
        .map_err(|_| "Power Plant could not recover a commit transaction.")
}

#[cfg(test)]
mod tests;
