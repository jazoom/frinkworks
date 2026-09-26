use std::sync::Arc;
use std::time::Duration;

use crate::agents::{AccessMode, DirectoryPolicy, EffectiveAuthority, LeaseGuard};
use crate::conversations::ConversationId;

use crate::execution::{AgentOutcome, AgentRunSpec};
use crate::providers::{ChatTurn, ProviderConnection};
use crate::sandbox::{GUEST_PROJECT, GuestExec, GuestSandbox};
use crate::sessions::{Job, JobStatus, SessionId};
use crate::state::AppState;

use super::definition::{AgentStep, StepAction, StepDefinition, SystemCommandId};
use super::execution::ExecutionGuard;
use super::id::{AttemptId, RunId};
use super::run::{FailureCategory, now_ms};
use super::store::StoreError;

pub(crate) const OPERATIONAL_STORE_ERROR: &str =
    "Frinkworks could not store the workflow run. Try again.";

const COMMAND_DEADLINE: Duration = if cfg!(test) {
    Duration::from_millis(50)
} else {
    Duration::from_secs(10)
};

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

    fn protect_cleanup_failure(
        &self,
        job: &Job,
        agent: Option<LeaseGuard>,
        execution: ExecutionGuard,
    ) {
        let _ = job.finish(
            JobStatus::Failed,
            Some("Frinkworks could not clean up the sandbox. This operation retains its reservations."),
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
    job: WorkflowJob,
    agent_lease: Option<LeaseGuard>,
    execution_lease: ExecutionGuard,
) {
    drive_attempts(state, job, agent_lease, execution_lease).await;
}

async fn drive_attempts(
    state: AppState,
    mut job: WorkflowJob,
    agent_lease: Option<LeaseGuard>,
    execution_lease: ExecutionGuard,
) {
    loop {
        let Some(run) = state.workflow_runs.get(&job.run_id) else {
            fail_operational(&state, &job);
            return;
        };
        if run.is_terminal() {
            settle_terminal_job(&state, &job, &run);
            return;
        }
        if job.job.cancel_requested() {
            if persist_cancel(&state, &job.run_id).is_err() {
                fail_operational(&state, &job);
            } else {
                settle_cancelled_job(&state, &job);
            }
            return;
        }
        if let Err(error) = confirm_run_authority(&state, &job) {
            settle_job(&state, &job, JobStatus::Failed, Some(&error));
            return;
        }
        let resumed = match &run.state {
            super::run::RunState::Active { step, attempt }
                if run
                    .attempts
                    .iter()
                    .any(|item| item.id == *attempt && item.continuation.is_some()) =>
            {
                Some((step.clone(), *attempt))
            }
            _ => None,
        };
        let Some(step) = resumed
            .as_ref()
            .map(|(step, _)| step)
            .or_else(|| run.ready_step())
            .and_then(|key| run.pinned.definition.step(key))
            .cloned()
        else {
            fail_operational(&state, &job);
            return;
        };
        let connection = match phase_connection(&state, &job, &run, &step) {
            Ok(connection) => connection,
            Err(error) => {
                settle_job(&state, &job, JobStatus::Failed, Some(&error));
                return;
            }
        };
        set_active_connection(&job, connection);
        let inputs = match resolve_inputs(&run, &step) {
            Ok(inputs) => inputs,
            Err(error) => {
                settle_job(&state, &job, JobStatus::Failed, Some(error));
                return;
            }
        };
        if let Err(error) =
            super::input_context::verify_inputs(&run, &step, &inputs, &state.workflow_artefacts)
        {
            settle_job(&state, &job, JobStatus::Failed, Some(error.message()));
            return;
        }
        if matches!(&step.action, StepAction::HumanGate(_)) {
            let Some(plan) = inputs
                .iter()
                .find(|input| input.artefact.kind == super::definition::ArtefactKind::Plan)
                .map(|input| input.artefact.clone())
            else {
                settle_job(
                    &state,
                    &job,
                    JobStatus::Failed,
                    Some("A plan checkpoint needs a plan input."),
                );
                return;
            };
            let Ok(gate_id) = super::GateId::generate() else {
                fail_operational(&state, &job);
                return;
            };
            if state
                .workflow_runs
                .mutate(&job.run_id, |run| {
                    run.open_plan_gate(gate_id, plan, now_ms()).map(|_| ())
                })
                .is_err()
            {
                fail_operational(&state, &job);
                return;
            }
            let _ = job.job.set_awaiting_decision();
            if !state.gate_continuations.insert(job) {
                let _ = state
                    .workflow_runs
                    .mutate(&run.id, |run| run.interrupt(now_ms()));
            }
            return;
        }
        let attempt_id = match resumed
            .as_ref()
            .map(|(_, attempt)| Ok(*attempt))
            .or_else(|| {
                run.revision_reservation
                    .as_ref()
                    .map(|reservation| Ok(reservation.attempt))
            })
            .unwrap_or_else(AttemptId::generate)
        {
            Ok(id) => id,
            Err(_) => {
                fail_operational(&state, &job);
                return;
            }
        };
        let default_settings = run.directory_settings();
        let capabilities = match run.phase_settings(&step.key).or(default_settings.as_ref()) {
            Some(settings) => {
                crate::execution::ProjectFreeAuthority::from_settings(job.agent_revision, settings)
                    .map_err(|_| super::capabilities::CapabilityError::Authority)
                    .and_then(|authority| {
                        super::capabilities::AttemptCapabilities::derive_project_free(
                            &step, &authority,
                        )
                    })
            }
            None => match phase_authority(&state, &job, &run, &step.key) {
                Ok(Some(authority)) => {
                    super::capabilities::AttemptCapabilities::derive_for_authority(
                        &step, &authority,
                    )
                }
                _ => Err(super::capabilities::CapabilityError::Authority),
            },
        };
        let capabilities = match capabilities {
            Ok(capabilities) => capabilities,
            Err(error) => {
                if persist_fail(&state, &job.run_id, None, FailureCategory::Authority).is_err() {
                    fail_operational(&state, &job);
                    return;
                }
                settle_job(&state, &job, JobStatus::Failed, Some(error.message()));
                return;
            }
        };
        let sandbox_record = if step_is_host(&run, &step) {
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
            super::run::AttemptSandboxRecord {
                kind: super::run::AttemptSandboxKind::IsolatedAttempt,
                snapshot_digest,
            }
        };
        if resumed.is_none()
            && persist_start(
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
        let IsolatedRun::Finished {
            mut outcome,
            cleanup,
            drafts,
        } = isolate_and_run(&state, &job, &step, attempt_id, &capabilities).await;
        if matches!(outcome, StepOutcome::Paused { .. }) && job.job.cancel_requested() {
            outcome = StepOutcome::Cancelled;
        }
        record_missing_terminal_evidence(&state, &job, &step, attempt_id, &outcome);
        if persist_cleanup(&state, &job.run_id, attempt_id, cleanup.clone()).is_err()
            || cleanup != super::run::AttemptCleanupRecord::Complete
        {
            state.gate_continuations.protect_cleanup_failure(
                &job.job,
                agent_lease,
                execution_lease,
            );
            return;
        }
        if matches!(outcome, StepOutcome::Completed) {
            if let Err(error) = publish_success(&state, &job, &step, attempt_id, &inputs, &drafts) {
                outcome = StepOutcome::Failed {
                    category: FailureCategory::Definition,
                    error: Some(error.to_owned()),
                };
            } else {
                continue;
            }
        }
        match outcome {
            StepOutcome::Completed => unreachable!(),
            StepOutcome::Failed { category, error } => {
                if persist_fail(&state, &job.run_id, Some(attempt_id), category).is_err() {
                    fail_operational(&state, &job);
                } else {
                    settle_job(&state, &job, JobStatus::Failed, error.as_deref());
                }
                return;
            }
            StepOutcome::Cancelled => {
                if persist_cancel(&state, &job.run_id).is_err() {
                    fail_operational(&state, &job);
                } else {
                    settle_cancelled_job(&state, &job);
                }
                return;
            }
            StepOutcome::Paused { budget, reply } => {
                pause_driven_job(&state, &mut job, attempt_id, &step, budget, *reply, &drafts);
                return;
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
    Paused {
        budget: crate::execution::BudgetSnapshot,
        reply: Box<crate::providers::AssistantReply>,
    },
}

enum IsolatedRun {
    Finished {
        outcome: StepOutcome,
        cleanup: crate::workflows::run::AttemptCleanupRecord,
        drafts: std::sync::Arc<std::sync::Mutex<crate::workflows::artefacts::output::OutputDrafts>>,
    },
}

async fn isolate_and_run(
    state: &AppState,
    job: &WorkflowJob,
    step: &StepDefinition,
    attempt_id: AttemptId,
    capabilities: &super::capabilities::AttemptCapabilities,
) -> IsolatedRun {
    let drafts = Arc::new(std::sync::Mutex::new(
        super::artefacts::output::OutputDrafts::default(),
    ));
    if let Some(run) = state.workflow_runs.get(&job.run_id) {
        if let Some(attempt) = run.attempts.iter().find(|item| item.id == attempt_id)
            && drafts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .restore(step.required_outputs(), &attempt.paused_drafts)
                .is_err()
        {
            return IsolatedRun::Finished {
                outcome: StepOutcome::Failed {
                    category: FailureCategory::Operational,
                    error: Some("The paused output drafts are invalid.".to_owned()),
                },
                cleanup: super::run::AttemptCleanupRecord::Complete,
                drafts,
            };
        }
        if step_is_host(&run, step) {
            let outcome = match &step.action {
                StepAction::Agent(action) => {
                    run_agent_step(state, job, action, None, drafts.clone()).await
                }
                _ => StepOutcome::Failed {
                    category: FailureCategory::Definition,
                    error: Some("Host steps must be model steps.".to_owned()),
                },
            };
            return IsolatedRun::Finished {
                outcome,
                cleanup: super::run::AttemptCleanupRecord::Complete,
                drafts,
            };
        }
    }
    let workspace = match state
        .workflow_workspaces
        .create_attempt(job.run_id, attempt_id)
    {
        Ok(workspace) => workspace,
        Err(error) => {
            return IsolatedRun::Finished {
                outcome: StepOutcome::Failed {
                    category: FailureCategory::Operational,
                    error: Some("Frinkworks cannot create the attempt workspace.".to_owned()),
                },
                cleanup: if error.orphaned {
                    super::run::AttemptCleanupRecord::Orphaned {
                        sandbox: false,
                        workspace: true,
                        journal: false,
                    }
                } else {
                    super::run::AttemptCleanupRecord::Complete
                },
                drafts,
            };
        }
    };
    let sandbox = state.sandboxes.attempt_handle(job.run_id, attempt_id);
    if let Err(error) =
        start_attempt_sandbox(state, job, step, capabilities, &workspace, sandbox.clone()).await
    {
        let outcome = StepOutcome::Failed {
            category: FailureCategory::Operational,
            error: Some(error.to_owned()),
        };
        let (outcome, cleanup) =
            cleanup_after_start_failure(state, attempt_id, sandbox, workspace, outcome).await;
        return IsolatedRun::Finished {
            outcome,
            cleanup,
            drafts,
        };
    }
    let outcome = dispatch_step(state, job, step, &sandbox, drafts.clone()).await;
    let sandbox_gone = sandbox.stop().await.is_ok() && sandbox.remove().await.is_ok();
    if sandbox_gone {
        state.sandboxes.drop_attempt(attempt_id);
    } else {
        state.sandboxes.expose_orphan(sandbox.name().to_owned());
    }
    let workspace_gone = sandbox_gone && workspace.destroy().is_ok();
    let cleanup = if sandbox_gone && workspace_gone {
        super::run::AttemptCleanupRecord::Complete
    } else {
        super::run::AttemptCleanupRecord::Orphaned {
            sandbox: !sandbox_gone,
            workspace: !workspace_gone,
            journal: false,
        }
    };
    IsolatedRun::Finished {
        outcome: fail_for_orphan(outcome, &cleanup),
        cleanup,
        drafts,
    }
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
            error: Some("Frinkworks could not clean up the isolated sandbox.".to_owned()),
        }
    }
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
            SystemCommandId::RepositoryStatus => {
                run_system_exec(state, job, step, sandbox, action.command).await
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
            || (action.directory_access.access().is_writable()
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
                            "A phase directory is unavailable at its saved path.".to_owned(),
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
    let policy = DirectoryPolicy::from_grants_with_workspace(
        phase_policy
            .grants()
            .iter()
            .cloned()
            .map(|mut grant| {
                grant.access = run
                    .attempts
                    .iter()
                    .find(|attempt| Some(attempt.id) == run.active_attempt())
                    .and_then(|attempt| {
                        attempt
                            .capabilities
                            .directories
                            .iter()
                            .find(|directory| directory.alias == grant.alias)
                    })
                    .map(|directory| directory.access)
                    .unwrap_or(AccessMode::ReadOnly);
                grant
            })
            .collect(),
        phase_policy.primary_alias().to_owned(),
    );
    let policy =
        policy.with_skill_root(crate::execution::global_skill_root(state.skills.host_dir()));
    let policy = if let Some(settings) = run.phase_settings(&step_key)
        && settings.location == crate::execution::ToolLocation::Host
    {
        policy.on_host(&crate::execution::command_directory(&settings.directories))
    } else {
        policy
    };
    let Some(role) = run.pinned.definition.role(&action.role).cloned() else {
        return StepOutcome::Failed {
            category: FailureCategory::Definition,
            error: Some(OPERATIONAL_STORE_ERROR.to_owned()),
        };
    };
    let agent_instructions = run
        .phase_model(&step_key)
        .map(|selection| selection.instructions.clone())
        .unwrap_or_else(|| {
            if let Some(conversation_id) = job.conversation_id {
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
        composed.push_str("\n\nTools run on this computer as the Frinkworks process user. ");
        if run
            .phase_settings(&step_key)
            .is_some_and(|settings| settings.automatic_host_commands())
        {
            composed.push_str("This run uses Automatic (YOLO) command approval. ");
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
    let packet = run
        .attempts
        .iter()
        .find(|attempt| attempt.id == attempt_id && attempt.continuation.is_some())
        .and_then(|attempt| attempt.initial_context.clone())
        .unwrap_or(packet);
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
    let host = {
        let settings = run
            .phase_settings(&step_key)
            .cloned()
            .or_else(|| run.directory_settings())
            .ok_or_else(|| StepOutcome::Failed {
                category: FailureCategory::Authority,
                error: Some("The pinned command settings are unavailable.".to_owned()),
            });
        let settings = match settings {
            Ok(settings) => settings,
            Err(outcome) => return outcome,
        };
        let Some(conversation) = job.conversation_id else {
            return StepOutcome::Failed {
                category: FailureCategory::Authority,
                error: Some("Workflow commands need a conversation.".to_owned()),
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
    };
    let skills = match crate::execution::discover_skills(
        sandbox.map(AsRef::as_ref),
        &policy,
        secret,
    )
    .await
    {
        Ok(skills) => skills,
        Err(error) => {
            return StepOutcome::Failed {
                category: FailureCategory::Authority,
                error: Some(error.message().to_owned()),
            };
        }
    };
    let resource_context =
        crate::execution::context::compose_resources(&packet.prompt, &[], &skills);
    let spec = AgentRunSpec {
        agent_id: job.agent_id,
        revision: 0,
        preamble: resource_context.text,
        context_prefix_len: packet.request_messages().len(),
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
        output_scope: Some(crate::execution::OutputScope {
            conversation: job.conversation_id,
            run: Some(run.id),
            attempt: Some(attempt_id),
        }),
        conversation: job.conversation_id,
        steering_session: None,
        budget: crate::execution::BudgetPolicy::ordinary(),
        sources: packet.project_instructions.sources.clone(),
        advertised: resource_context.advertised,
    };
    // Conversation workflow output belongs to attempt evidence, not the conversation reply.
    if job.conversation_id.is_some() && run.kind == super::run::RunKind::Configured {
        job.job.set_output_visible(false);
    }
    let mut turns = packet.request_messages();
    if let Some(attempt) = run.attempts.iter().find(|attempt| attempt.id == attempt_id)
        && attempt.continuation.is_some()
    {
        let Some(stored) = state.workflow_evidence.get(&run.id, &attempt_id) else {
            return StepOutcome::Failed {
                category: FailureCategory::Operational,
                error: Some("The paused phase history is unavailable.".to_owned()),
            };
        };
        let mut history = stored.history;
        if let Some(compaction) = stored.compaction {
            match crate::conversations::compaction::project_turns(
                &history,
                compaction.covered_through as usize,
                &compaction.text,
                &compaction.requests,
            ) {
                Ok(projected) => history = projected,
                Err(error) => {
                    return StepOutcome::Failed {
                        category: FailureCategory::Operational,
                        error: Some(error.message().to_owned()),
                    };
                }
            }
        }
        turns.extend(history);
    }
    let ended = crate::execution::run_agent_action(state, spec, turns, job.job.clone()).await;
    if ended.outcome != AgentOutcome::BudgetExhausted {
        let terminal_state = match ended.outcome {
            AgentOutcome::Completed => crate::workflows::evidence::TerminalState::Completed,
            AgentOutcome::ProviderFailure
            | AgentOutcome::ToolFailure
            | AgentOutcome::AuthorityFailure
            | AgentOutcome::PersistenceFailure
            | AgentOutcome::UncertainEffect
            | AgentOutcome::ContextBlocked => crate::workflows::evidence::TerminalState::Failed,
            AgentOutcome::Cancelled => crate::workflows::evidence::TerminalState::Cancelled,
            AgentOutcome::BudgetExhausted => unreachable!("budget pause skips terminal evidence"),
        };
        evidence.terminal(terminal_state, &ended.reply, ended.error.as_deref(), secret);
    }
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
        AgentOutcome::AuthorityFailure => StepOutcome::Failed {
            category: FailureCategory::Authority,
            error: ended.error,
        },
        AgentOutcome::PersistenceFailure => StepOutcome::Failed {
            category: FailureCategory::Operational,
            error: ended.error,
        },
        AgentOutcome::UncertainEffect => StepOutcome::Failed {
            category: FailureCategory::Command,
            error: ended.error,
        },
        AgentOutcome::Cancelled => StepOutcome::Cancelled,
        AgentOutcome::BudgetExhausted => StepOutcome::Paused {
            budget: ended.budget.expect("budget pause carries a snapshot"),
            reply: Box::new(ended.reply),
        },
        AgentOutcome::ContextBlocked => StepOutcome::Failed {
            category: FailureCategory::Provider,
            error: ended.error,
        },
    }
}

fn record_missing_terminal_evidence(
    state: &AppState,
    job: &WorkflowJob,
    step: &StepDefinition,
    attempt_id: AttemptId,
    outcome: &StepOutcome,
) {
    if matches!(outcome, StepOutcome::Paused { .. }) {
        return;
    }
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
        StepOutcome::Paused { .. } => {
            (crate::workflows::evidence::TerminalState::Interrupted, None)
        }
    }
}

async fn run_system_exec(
    state: &AppState,
    work: &WorkflowJob,
    step: &StepDefinition,
    sandbox: &GuestSandbox,
    command: SystemCommandId,
) -> StepOutcome {
    let execute = async {
        let run = state
            .workflow_runs
            .get(&work.run_id)
            .ok_or(OPERATIONAL_STORE_ERROR)?;
        let settings = run
            .directory_settings()
            .ok_or("The command settings are unavailable.")?;
        let attempt = run.active_attempt().ok_or(OPERATIONAL_STORE_ERROR)?;
        let conversation = work
            .conversation_id
            .ok_or("Command approval needs a conversation.")?;
        let directory = run
            .attempts
            .iter()
            .find(|record| record.id == attempt)
            .and_then(|record| record.capabilities.primary())
            .map(|directory| directory.guest_path.as_str())
            .unwrap_or(crate::execution::GUEST_WORKSPACE);
        let connection = work.active_connection();
        let secret = match connection.auth {
            crate::providers::AuthMethod::ApiKey => Some(connection.api_key.expose()),
            _ => None,
        };
        let mut request = crate::execution::HostCommandRequest {
            token: String::new(),
            session: work.session_id,
            job: work.job.id(),
            conversation,
            execution_revision: work.agent_revision,
            command: "git status --porcelain=v1".to_owned(),
            directory: directory.into(),
            explanation: command.consequence().to_owned(),
            run: Some(run.id.as_hex()),
            step: Some(step.key.as_str().to_owned()),
            attempt: Some(attempt.as_hex()),
        };
        crate::execution::approval::approve_command(
            state,
            &mut request,
            &settings,
            &work.job,
            secret,
        )
        .await?;
        confirm_run_authority(state, work)
            .map_err(|_| "The command authority changed before dispatch.")?;
        state
            .workflow_evidence
            .host_command(&request, "dispatching", "", secret, None)
            .map_err(|_| OPERATIONAL_STORE_ERROR)?;
        let key = crate::execution::OutputKey {
            scope: crate::execution::OutputScope {
                conversation: Some(conversation),
                run: Some(run.id),
                attempt: Some(attempt),
            },
            job: work.job.id(),
            tool_call: request.token.clone(),
            model_hidden: false,
        };
        let result = crate::execution::command::capture_sandbox_command(
            sandbox,
            guest_command(command).in_dir(directory),
            &work.job,
            secret,
            COMMAND_DEADLINE,
            &request.token,
            Some((&state.outputs, &key)),
        )
        .await;
        let output = match &result {
            Ok(output) => output,
            Err(failure) => &failure.result,
        };
        state
            .workflow_evidence
            .host_command(
                &request,
                if output.is_success() {
                    "completed"
                } else {
                    "failed"
                },
                &output.report(),
                secret,
                Some(output),
            )
            .map_err(|_| OPERATIONAL_STORE_ERROR)?;
        if output.is_success() {
            Ok(())
        } else {
            Err("The command did not succeed.")
        }
    };
    match execute.await {
        Ok(()) => StepOutcome::Completed,
        Err(_) if work.job.cancel_requested() => StepOutcome::Cancelled,
        Err(error) => StepOutcome::Failed {
            category: FailureCategory::Command,
            error: Some(error.to_owned()),
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
    }
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
    let phase_authority = run
        .phase_settings(&step.key)
        .map(|settings| {
            crate::execution::ProjectFreeAuthority::from_settings(job.agent_revision, settings)
        })
        .transpose()
        .map_err(|_| "The directory authority changed.")?;
    let spec = if let Some(authority) = phase_authority
        .as_ref()
        .or(job.project_free_authority.as_ref())
    {
        let spec =
            project_free_attempt_spec(capabilities, workspace, authority, state.skills.host_dir())?;
        for grant in authority.policy.grants() {
            crate::sandbox::confirm_host_write_access(&spec, &grant.host_path, grant.access)
                .map_err(|error| error.message())?;
        }
        spec
    } else {
        let user_project = user_project.ok_or("Choose a project directory.")?;
        let spec = attempt_spec(
            capabilities,
            workspace,
            &user_project.host_path,
            &job.host_policy,
            state.skills.host_dir(),
        )?;
        crate::sandbox::confirm_host_write_access(
            &spec,
            &user_project.host_path,
            user_project.access,
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
    _workspace: &crate::workflows::workspace::AttemptWorkspace,
    authority: &crate::execution::ProjectFreeAuthority,
    global_skills: Option<&std::path::Path>,
) -> Result<crate::sandbox::SandboxSpec, &'static str> {
    let mut mounts = Vec::new();
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
        mounts.push(crate::sandbox::MountSpec {
            guest: grant.guest_path.clone(),
            host: if directory.access.is_writable() && !grant.access.is_writable() {
                return Err("The step exceeds its directory access.");
            } else {
                grant.host_path.clone()
            },
            read_only: !directory.access.is_writable(),
        });
    }
    add_global_skills_mount(&mut mounts, global_skills);
    Ok(crate::sandbox::SandboxSpec {
        mounts,
        workdir: capabilities
            .directories
            .first()
            .map(|directory| directory.guest_path.clone())
            .unwrap_or_else(|| crate::execution::GUEST_WORKSPACE.to_owned()),
        network: capabilities.sandbox_network(),
    })
}

fn attempt_spec(
    capabilities: &crate::workflows::capabilities::AttemptCapabilities,
    _workspace: &crate::workflows::workspace::AttemptWorkspace,
    user_project: &std::path::Path,
    host: &DirectoryPolicy,
    global_skills: Option<&std::path::Path>,
) -> Result<crate::sandbox::SandboxSpec, &'static str> {
    let mut mounts = Vec::new();
    let Some(primary) = capabilities.primary() else {
        return Err("A sandbox-backed step needs a primary source.");
    };
    mounts.push(crate::sandbox::MountSpec {
        guest: primary.guest_path.clone(),
        host: user_project.to_path_buf(),
        read_only: !primary.access.is_writable(),
    });
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
    add_global_skills_mount(&mut mounts, global_skills);
    Ok(crate::sandbox::SandboxSpec {
        mounts,
        workdir: primary.guest_path.clone(),
        network: capabilities.sandbox_network(),
    })
}

/// Every sandbox attempt receives the read-only global skill mount. The mount
/// grants no directory authority and the model still needs the read tool.
fn add_global_skills_mount(
    mounts: &mut Vec<crate::sandbox::MountSpec>,
    global_skills: Option<&std::path::Path>,
) {
    if let Some(dir) = global_skills {
        mounts.push(crate::sandbox::MountSpec {
            guest: crate::execution::GUEST_GLOBAL_SKILLS.to_owned(),
            host: dir.to_path_buf(),
            read_only: true,
        });
    }
}

fn confirm_run_authority(state: &AppState, job: &WorkflowJob) -> Result<(), String> {
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
                "The workflow belongs to another conversation or needs recovery.".to_owned(),
            );
        }
        let settings = run
            .directory_settings()
            .or_else(|| record.model.as_ref().map(|model| model.settings.clone()))
            .ok_or_else(|| "The conversation settings are unavailable.".to_owned())?;
        let pinned_authority =
            crate::execution::ProjectFreeAuthority::from_settings(authority.revision, &settings)
                .map_err(|_| "A pinned directory is unavailable at its saved path.".to_owned())?;
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
                if !settings.host_access_allowed() {
                    return true;
                }
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
                    grant.requires_access_consent(state.local_data.root())
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
        return Ok(());
    }
    Err("That project is not in the catalogue.".to_owned())
}

fn resolve_inputs(
    run: &crate::workflows::WorkflowRun,
    step: &StepDefinition,
) -> Result<Vec<super::run::AttemptArtefactInput>, &'static str> {
    let mut inputs = Vec::new();
    for input in &step.inputs {
        let artefact = match &input.source {
            crate::workflows::definition::ArtefactSource::RunCurrentPlan => {
                match run.current_plan() {
                    Some(plan) => plan,
                    None if step.accepts_missing_initial_plan(input) => continue,
                    None => return Err("The current plan is missing."),
                }
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
                    if run.accepts_missing_initial_review(step, input, run.attempts.len()) {
                        continue;
                    }
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

fn publish_success(
    state: &AppState,
    job: &WorkflowJob,
    step: &StepDefinition,
    attempt_id: AttemptId,
    inputs: &[super::run::AttemptArtefactInput],
    drafts: &std::sync::Mutex<super::artefacts::output::OutputDrafts>,
) -> Result<(), &'static str> {
    let connection = job.active_connection();
    let secret = match connection.auth {
        crate::providers::AuthMethod::ApiKey => Some(connection.api_key.expose()),
        _ => None,
    };
    let mut held = drafts.lock().unwrap_or_else(|error| error.into_inner());
    let mut artefacts = Vec::new();
    let mut outputs = Vec::new();
    for output in step.required_outputs() {
        if output.kind == super::definition::OutputKind::AssistantReply {
            continue;
        }
        let draft = held
            .take(&output.key)
            .ok_or("A required output is missing.")?;
        let record = publish_draft(state, job, attempt_id, step, output, draft, inputs, secret)?;
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
    state
        .workflow_runs
        .mutate(&job.run_id, |run| {
            run.record_attempt_outputs(attempt_id, artefacts, outputs)?;
            run.complete_attempt(attempt_id, now_ms())
        })
        .map(|_| ())
        .map_err(|_| OPERATIONAL_STORE_ERROR)
}

#[allow(clippy::too_many_arguments)]
fn publish_draft(
    state: &AppState,
    job: &WorkflowJob,
    attempt_id: AttemptId,
    step: &StepDefinition,
    output: &crate::workflows::definition::RequiredOutput,
    draft: crate::workflows::artefacts::output::OutputDraft,
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
            let (bytes, object, hash) =
                crate::workflows::artefacts::payload::encode_review(verdict, &markdown, secret)
                    .map_err(|_| "That review output is not valid.")?;
            (
                bytes,
                object,
                hash,
                crate::workflows::definition::ArtefactKind::ReviewReport,
                crate::workflows::artefacts::ArtefactSummary::Review { verdict },
            )
        }
        OutputDraft::Test { outcome, markdown } => {
            let (bytes, object, hash) =
                crate::workflows::artefacts::payload::encode_test(outcome, &markdown, secret)
                    .map_err(|_| "That test output is not valid.")?;
            (
                bytes,
                object,
                hash,
                crate::workflows::definition::ArtefactKind::TestReport,
                crate::workflows::artefacts::ArtefactSummary::Test { outcome },
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
                run.fail_before_attempt()
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

pub(crate) fn settle_cancelled_job(state: &AppState, workflow: &WorkflowJob) {
    settle_job(state, workflow, JobStatus::Cancelled, None);
}

#[allow(clippy::too_many_arguments)]
fn pause_driven_job(
    state: &AppState,
    job: &mut WorkflowJob,
    attempt_id: AttemptId,
    step: &StepDefinition,
    budget: crate::execution::BudgetSnapshot,
    reply: crate::providers::AssistantReply,
    drafts: &std::sync::Arc<std::sync::Mutex<crate::workflows::artefacts::output::OutputDrafts>>,
) {
    let Ok(checkpoint) = crate::conversations::CheckpointId::generate() else {
        fail_operational(state, job);
        return;
    };
    let drafts = drafts
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .snapshot();
    if state
        .workflow_runs
        .mutate(&job.run_id, |run| {
            run.pause_attempt(attempt_id, checkpoint, drafts.clone(), now_ms())
        })
        .is_err()
    {
        fail_operational(state, job);
        return;
    }
    let Some(conversation) = job.conversation_id else {
        job.job.finish(JobStatus::Completed, None);
        return;
    };
    let Some(record) = state.conversations.get(&conversation) else {
        fail_operational(state, job);
        return;
    };
    let Some(model) = record.model.clone() else {
        fail_operational(state, job);
        return;
    };
    let Ok(boundary) = crate::conversations::MessageId::generate() else {
        fail_operational(state, job);
        return;
    };
    let stored = crate::conversations::ContinuationCheckpoint {
        id: checkpoint,
        boundary,
        pinned: state
            .workflow_runs
            .get(&job.run_id)
            .and_then(|run| run.phase_settings(&step.key).cloned())
            .unwrap_or(model.settings),
        budget,
        run: Some(job.run_id),
        attempt: Some(attempt_id),
        step: Some(step.key.as_str().to_owned()),
        drafts,
        created_at_ms: now_ms(),
    };
    if state
        .conversations
        .pause_for_budget(&conversation, job.job.id(), reply, stored)
        .is_err()
    {
        fail_operational(state, job);
        return;
    }
    job.job.finish(JobStatus::Completed, None);
    state
        .sessions
        .finish_conversation_job(&job.session_id, conversation, job.job.id());
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
    // A direct command owns a command entry, not an assistant message. It also
    // starts no title request. The outcome is taken from the command itself so
    // a gate decision cannot rewrite it.

    let connection = workflow.active_connection();
    let secret = match &connection.auth {
        crate::providers::AuthMethod::ApiKey => Some(connection.api_key.expose()),
        crate::providers::AuthMethod::Plan => None,
    };
    let error = error
        .and_then(|text| crate::providers::sanitise_detail(&crate::tools::redact(text, secret)));
    let reply = crate::execution::bound_reply(reply);
    let conversation_reply = {
        state
            .workflow_runs
            .get(&workflow.run_id)
            .filter(|run| run.kind == super::run::RunKind::Configured)
            .map(|_run| {
                let result = &reply.text;
                conversation_run_result(workflow.run_id, status, result)
            })
    };
    if let Some(conversation_id) = workflow.conversation_id {
        let message_status = match status {
            JobStatus::Completed => crate::conversations::MessageStatus::Complete,
            JobStatus::Cancelled => crate::conversations::MessageStatus::Interrupted,
            JobStatus::Failed
            | JobStatus::AwaitingDecision
            | JobStatus::AwaitingQuestion
            | JobStatus::Running => crate::conversations::MessageStatus::Failed,
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
    state
        .conversations
        .invalidate_questions_for_job(workflow.job.id());
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
        JobStatus::Running | JobStatus::AwaitingDecision | JobStatus::AwaitingQuestion => {
            "is still active"
        }
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

#[cfg(test)]
mod tests;
