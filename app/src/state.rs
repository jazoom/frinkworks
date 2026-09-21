use std::sync::Arc;

use crate::{
    agents::{AgentLeaseCoordinator, AgentStore},
    assets::AssetPaths,
    config::{RuntimeConfig, StartupConfig},
    conversations::ConversationStore,
    environments::{
        EnvironmentCatalogue, EnvironmentPreparationScheduler, EnvironmentSnapshotRepository,
    },
    execution::{
        AccessConsentStore, ConversationRuntime, DirectoryPicker, HostApprovalStore, OutputStore,
    },
    local_data::LocalDataReset,
    models::models_dev::ModelsDevCatalogue,
    plan_login::PlanLogin,
    preferences::Preferences,
    presets::PresetStore,
    providers::ChatBackend,
    sandbox::SandboxFleet,
    sessions::SessionStore,
    skills::SkillStore,
    vault::ProviderVault,
    workflows::{
        ApplyJournals, CommitJournals, WorkflowArtefactRepository, WorkflowCatalogue,
        WorkflowContinuationRegistry, WorkflowEvidenceStore, WorkflowExecution, WorkflowRunStore,
        workspace::WorkflowWorkspaces,
    },
};

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) config: Arc<RuntimeConfig>,
    pub(crate) assets: Arc<AssetPaths>,
    pub(crate) sessions: Arc<SessionStore>,
    pub(crate) vault: Arc<ProviderVault>,
    pub(crate) chat: Arc<ChatBackend>,
    pub(crate) models_dev: Arc<ModelsDevCatalogue>,
    pub(crate) plan_login: Arc<PlanLogin>,
    pub(crate) preferences: Arc<Preferences>,
    pub(crate) presets: Arc<PresetStore>,
    pub(crate) agents: Arc<AgentStore>,
    pub(crate) skills: Arc<SkillStore>,
    pub(crate) conversations: Arc<ConversationStore>,
    pub(crate) forks: Arc<crate::conversations::ForkDrafts>,
    pub(crate) directory_picker: DirectoryPicker,
    pub(crate) access_consent: Arc<AccessConsentStore>,
    pub(crate) host_approvals: Arc<HostApprovalStore>,
    pub(crate) conversation_runtime: Arc<ConversationRuntime>,
    pub(crate) local_data: LocalDataReset,
    pub(crate) sandboxes: Arc<SandboxFleet>,
    pub(crate) outputs: Arc<OutputStore>,
    pub(crate) agent_leases: Arc<AgentLeaseCoordinator>,
    pub(crate) workflows: Arc<WorkflowCatalogue>,
    pub(crate) workflow_runs: Arc<WorkflowRunStore>,
    pub(crate) workflow_evidence: Arc<WorkflowEvidenceStore>,
    pub(crate) workflow_artefacts: Arc<WorkflowArtefactRepository>,
    pub(crate) workflow_execution: Arc<WorkflowExecution>,
    pub(crate) gate_continuations: Arc<WorkflowContinuationRegistry>,
    pub(crate) handoff_drafts: Arc<crate::workflows::handoff::HandoffDrafts>,
    pub(crate) workflow_workspaces: Arc<WorkflowWorkspaces>,
    pub(crate) apply_journals: Arc<ApplyJournals>,
    pub(crate) commit_journals: Arc<CommitJournals>,
    pub(crate) environments: Arc<EnvironmentCatalogue>,
    pub(crate) environment_snapshots: Arc<EnvironmentSnapshotRepository>,
    pub(crate) environment_preparations: Arc<EnvironmentPreparationScheduler>,
    #[cfg(test)]
    scratch: Arc<std::sync::Mutex<Vec<tempfile::TempDir>>>,
}

pub(crate) async fn build(
    config: StartupConfig,
    assets: AssetPaths,
    local_data: LocalDataReset,
) -> Result<AppState, String> {
    let data_dir = local_data.root();
    let agents =
        AgentStore::open(data_dir.join("agents")).map_err(|error| error.message().to_owned())?;
    let skills =
        SkillStore::open(data_dir.join("skills")).map_err(|error| error.message().to_owned())?;
    let conversations = ConversationStore::open(data_dir.join("conversations"))
        .map_err(|error| error.message().to_owned())?;
    let presets =
        PresetStore::open(data_dir.join("presets")).map_err(|error| error.message().to_owned())?;
    let environments = EnvironmentCatalogue::open(
        data_dir.join("environments.json"),
        data_dir.join("environment-preparation-logs"),
    )
    .map_err(|error| error.message().to_owned())?;
    let seeds = environments
        .seed_id(crate::environments::seeds::ALPINE_GIT_V1)
        .map(crate::workflows::seeds::production_seeds)
        .unwrap_or_default();
    let workflows = WorkflowCatalogue::open_with_seeds(data_dir.join("workflows.json"), &seeds)
        .map_err(|error| error.message().to_owned())?;
    let workflow_artefacts = Arc::new(
        WorkflowArtefactRepository::open(data_dir.join("workflow-artefacts"))
            .map_err(|error| error.message().to_owned())?,
    );
    let workflow_runs = WorkflowRunStore::open(data_dir.join("workflow-runs"))
        .map_err(|error| error.message().to_owned())?;
    let workflow_evidence = WorkflowEvidenceStore::open(data_dir.join("workflow-evidence"))
        .map_err(|error| error.message().to_owned())?;
    let environment_snapshots =
        EnvironmentSnapshotRepository::open(data_dir.join("environment-snapshots"))
            .map_err(|_| "The environment snapshot store is unreadable.".to_owned())?;
    let sandboxes = SandboxFleet::prepare().await;
    let outputs = OutputStore::open(data_dir.join("command-output"))
        .map_err(|error| error.message().to_owned())?;
    outputs
        .remove_abandoned(|scope| {
            if let Some(conversation) = scope.conversation {
                conversations.contains(&conversation)
            } else {
                scope.run.zip(scope.attempt).is_some_and(|(run, attempt)| {
                    workflow_runs
                        .get(&run)
                        .is_some_and(|run| run.attempts.iter().any(|record| record.id == attempt))
                })
            }
        })
        .map_err(|error| error.message().to_owned())?;
    let guest_recovery = sandboxes.recover_transient_guests().await;
    for environment in environments.list() {
        let Some(ready) = environment.ready_preparation else {
            continue;
        };
        let Some(preparation) = environments.preparation(&ready) else {
            continue;
        };
        let Some(snapshot) = preparation.snapshot else {
            continue;
        };
        let _ = environment_snapshots.inspect(&snapshot).await;
    }
    let environments = Arc::new(environments);
    let environment_snapshots = Arc::new(environment_snapshots);
    let environment_preparations =
        EnvironmentPreparationScheduler::start(environments.clone(), environment_snapshots.clone());
    environment_preparations.wake();
    let apply_journals = ApplyJournals::open(data_dir.join("workflow-apply-journals"))
        .map_err(|_| "The workflow application journal store is unreadable.".to_owned())?;
    let commit_journals = CommitJournals::open(data_dir.join("workflow-commit-journals"))
        .map_err(|_| "The workflow commit journal store is unreadable.".to_owned())?;
    let workflow_workspaces = WorkflowWorkspaces::open(data_dir.join("workflow-workspaces"))
        .map_err(|_| "The workflow workspace store is unreadable.".to_owned())?;
    let vault = ProviderVault::open(data_dir.join("providers.json"))
        .map_err(|_| "The provider vault is unreadable.".to_owned())?;
    let preferences = Preferences::open(data_dir.join("preferences.json"));
    let models_dev = ModelsDevCatalogue::open(data_dir.join("models-dev-catalogue.json"))?;
    let state = AppState {
        config: Arc::new(config.runtime),
        assets: Arc::new(assets),
        sessions: Arc::new(SessionStore::new()),
        vault: Arc::new(vault),
        chat: Arc::new(ChatBackend::Rig),
        models_dev: Arc::new(models_dev),
        plan_login: Arc::new(PlanLogin::new()),
        preferences: Arc::new(preferences),
        presets: Arc::new(presets),
        agents: Arc::new(agents),
        skills: Arc::new(skills),
        conversations: Arc::new(conversations),
        forks: Arc::new(crate::conversations::ForkDrafts::default()),
        directory_picker: DirectoryPicker::native(),
        access_consent: Arc::new(AccessConsentStore::new()),
        host_approvals: Arc::new(HostApprovalStore::new()),
        conversation_runtime: Arc::new(
            ConversationRuntime::open(data_dir.join("conversation-runtime"))
                .map_err(str::to_owned)?,
        ),
        local_data,
        sandboxes: Arc::new(sandboxes),
        outputs: Arc::new(outputs),
        agent_leases: Arc::new(AgentLeaseCoordinator::new()),
        workflows: Arc::new(workflows),
        workflow_runs: Arc::new(workflow_runs),
        workflow_evidence: Arc::new(workflow_evidence),
        workflow_artefacts,
        workflow_execution: Arc::new(WorkflowExecution::new()),
        gate_continuations: Arc::new(WorkflowContinuationRegistry::new()),
        handoff_drafts: Arc::new(crate::workflows::handoff::HandoffDrafts::default()),
        workflow_workspaces: Arc::new(workflow_workspaces),
        apply_journals: Arc::new(apply_journals),
        commit_journals: Arc::new(commit_journals),
        environments,
        environment_snapshots,
        environment_preparations,
        #[cfg(test)]
        scratch: Arc::new(std::sync::Mutex::new(Vec::new())),
    };
    if state.local_data.is_pending() {
        return Err("Power Plant could not reset local data.".to_owned());
    }
    let active_commit_attempts: Vec<_> = state
        .workflow_runs
        .active_runs()
        .into_iter()
        .filter_map(|run| {
            let attempt = run.active_attempt()?;
            run.attempts
                .iter()
                .find(|record| record.id == attempt)
                .and_then(|record| record.commit_transaction.as_ref())
                .map(|_| attempt)
        })
        .collect();
    if !active_commit_attempts.is_empty()
        && (!guest_recovery.inventory_complete
            || active_commit_attempts
                .iter()
                .any(|attempt| guest_recovery.attempts_remaining.contains(attempt)))
    {
        return Err("Power Plant could not recover a commit transaction.".to_owned());
    }
    crate::workflows::recover_apply_transactions(&state).map_err(str::to_owned)?;
    crate::workflows::recover_commit_transactions(&state).map_err(str::to_owned)?;
    state
        .workflow_runs
        .interrupt_active()
        .map_err(|_| "Power Plant could not record workflow recovery.".to_owned())?;
    state
        .conversations
        .recover_handoffs(&state.workflow_runs)
        .map_err(|error| error.message().to_owned())?;
    state
        .conversations
        .interrupt_requests()
        .map_err(|error| error.message().to_owned())?;
    let workspace_recovery = state
        .workflow_workspaces
        .recover_leftovers(
            |run, attempt| {
                state
                    .workflow_runs
                    .get(run)
                    .is_some_and(|record| record.active_attempt() == Some(*attempt))
            },
            |run, attempt| {
                !guest_recovery.inventory_complete
                    || guest_recovery.attempts_remaining.contains(attempt)
                    || guest_recovery.runs_remaining.contains(run)
            },
        )
        .map_err(|_| "Power Plant could not recover workflow workspaces.".to_owned())?;
    for (run_id, attempt_id) in state.workflow_runs.pending_cleanup_attempts() {
        let cleanup = recovered_cleanup_record(
            guest_recovery.inventory_complete,
            &guest_recovery.attempts_remaining,
            &guest_recovery.runs_remaining,
            &workspace_recovery,
            run_id,
            attempt_id,
        );
        state
            .workflow_runs
            .mutate(&run_id, |run| run.record_cleanup(attempt_id, cleanup))
            .map_err(|_| "Power Plant could not record workflow recovery.".to_owned())?;
    }
    state
        .conversation_runtime
        .recover(
            &guest_recovery,
            &workspace_recovery,
            &state.workflow_execution,
        )
        .map_err(str::to_owned)?;
    Ok(state)
}

fn recovered_cleanup_record(
    inventory_complete: bool,
    guests_remaining: &std::collections::BTreeSet<crate::workflows::AttemptId>,
    runs_remaining: &std::collections::BTreeSet<crate::workflows::RunId>,
    workspaces: &[crate::workflows::workspace::WorkspaceRecovery],
    run: crate::workflows::RunId,
    attempt: crate::workflows::AttemptId,
) -> crate::workflows::run::AttemptCleanupRecord {
    let sandbox =
        !inventory_complete || guests_remaining.contains(&attempt) || runs_remaining.contains(&run);
    let workspace = workspaces
        .iter()
        .any(|item| item.run == run && item.attempt == attempt && item.remains);
    if sandbox || workspace {
        crate::workflows::run::AttemptCleanupRecord::Orphaned {
            sandbox,
            workspace,
            journal: false,
        }
    } else {
        crate::workflows::run::AttemptCleanupRecord::Complete
    }
}

#[cfg(test)]
pub(super) mod tests;
