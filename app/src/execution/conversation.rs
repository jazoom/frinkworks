use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use serde::{Deserialize, Serialize};

use crate::{
    agents::DirectoryPolicy,
    conversations::{ConversationId, ConversationRecord, MessageStatus},
    providers::{AssistantReply, ChatTurn, ProviderConnection},
    sandbox::{GuestSandbox, MountSpec, SandboxSpec, TransientGuestRecovery},
    sessions::{Job, JobStatus, SessionId},
    state::AppState,
    workflows::{AttemptId, ExecutionGuard, RunId, WorkflowJob, workspace::WorkspaceRecovery},
};

use super::{
    AgentOutcome, AgentRunSpec, DirectoryAccess, ExecutionSettings, OutputScope,
    ProjectFreeAuthority, ToolLocation,
};

#[cfg(test)]
mod tests;

const RUNTIME_RECORD_VERSION: u32 = 1;
const MAXIMUM_RUNTIME_RECORD_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OrdinaryKind {
    Host,
    Sandbox,
    FileChange,
}

pub(crate) fn ordinary_kind(settings: &ExecutionSettings) -> Option<OrdinaryKind> {
    if crate::tools::advertised(&settings.tools, settings.location).is_empty() {
        return None;
    }
    match settings.location {
        ToolLocation::Host if settings.directories.is_empty() => Some(OrdinaryKind::Host),
        ToolLocation::Host => Some(OrdinaryKind::FileChange),
        ToolLocation::Sandbox
            if settings
                .directories
                .iter()
                .all(|grant| grant.access == DirectoryAccess::ReadOnly) =>
        {
            Some(OrdinaryKind::Sandbox)
        }
        ToolLocation::Sandbox => Some(OrdinaryKind::FileChange),
    }
}

pub(crate) struct ConversationRuntime {
    dir: Option<PathBuf>,
    inner: Mutex<BTreeMap<ConversationId, RuntimeRecord>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RuntimeRecord {
    job: crate::sessions::JobId,
    run: RunId,
    attempt: AttemptId,
    sandbox: bool,
    workspace: bool,
}

#[derive(Deserialize, Serialize)]
struct RuntimeFile {
    version: u32,
    conversation: String,
    job: String,
    run: String,
    attempt: String,
    sandbox: bool,
    workspace: bool,
}

impl ConversationRuntime {
    pub(crate) fn open(dir: PathBuf) -> Result<Self, &'static str> {
        crate::storage::ensure_private_dir(&dir).map_err(|_| persist_error())?;
        let records = load_dir(&dir)?;
        Ok(Self {
            dir: Some(dir),
            inner: Mutex::new(records),
        })
    }

    pub(crate) fn unsettled(&self, conversation: ConversationId) -> bool {
        self.lock().contains_key(&conversation)
    }

    pub(crate) fn recover(
        &self,
        guests: &TransientGuestRecovery,
        workspaces: &[WorkspaceRecovery],
        execution: &Arc<crate::workflows::WorkflowExecution>,
    ) -> Result<(), &'static str> {
        let mut remaining = false;
        let mut records = self.lock();
        let conversations: Vec<_> = records.keys().copied().collect();
        for conversation in conversations {
            let Some(record) = records.get(&conversation).cloned() else {
                continue;
            };
            let sandbox = !guests.inventory_complete
                || guests.attempts_remaining.contains(&record.attempt)
                || guests.runs_remaining.contains(&record.run);
            let workspace = workspaces.iter().any(|item| {
                item.run == record.run && item.attempt == record.attempt && item.remains
            });
            if !sandbox && !workspace {
                remove_record(self.dir.as_deref(), conversation)?;
                records.remove(&conversation);
                continue;
            }
            let updated = RuntimeRecord {
                sandbox,
                workspace,
                ..record
            };
            persist_record(self.dir.as_deref(), conversation, &updated)?;
            records.insert(conversation, updated);
            remaining = true;
        }
        drop(records);
        if remaining {
            let guard = execution.acquire()?;
            guard.require_recovery();
        }
        Ok(())
    }

    fn remember(
        &self,
        conversation: ConversationId,
        job: crate::sessions::JobId,
        run: RunId,
        attempt: AttemptId,
    ) -> Result<(), &'static str> {
        let record = RuntimeRecord {
            job,
            run,
            attempt,
            sandbox: true,
            workspace: true,
        };
        persist_record(self.dir.as_deref(), conversation, &record)?;
        self.lock().insert(conversation, record);
        Ok(())
    }

    fn finish(
        &self,
        conversation: ConversationId,
        sandbox: bool,
        workspace: bool,
    ) -> Result<(), &'static str> {
        if !sandbox && !workspace {
            remove_record(self.dir.as_deref(), conversation)?;
            self.lock().remove(&conversation);
            return Ok(());
        }
        let mut records = self.lock();
        let Some(mut record) = records.get(&conversation).cloned() else {
            return Ok(());
        };
        record.sandbox = sandbox;
        record.workspace = workspace;
        persist_record(self.dir.as_deref(), conversation, &record)?;
        records.insert(conversation, record);
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<ConversationId, RuntimeRecord>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub(crate) struct FileChangeWork {
    pub(crate) run_id: RunId,
    pub(crate) project_free: ProjectFreeAuthority,
}

pub(crate) struct OrdinaryRun {
    pub(crate) session: SessionId,
    pub(crate) record: ConversationRecord,
    pub(crate) connection: ProviderConnection,
    pub(crate) job: Arc<Job>,
    pub(crate) execution: ExecutionGuard,
    pub(crate) kind: OrdinaryKind,
    pub(crate) turns: Vec<ChatTurn>,
    pub(crate) file: Option<FileChangeWork>,
}

pub(crate) async fn run(state: AppState, work: OrdinaryRun) {
    if work.kind == OrdinaryKind::FileChange {
        run_file_change(state, work).await;
        return;
    }
    let conversation = work.record.id;
    let secret = match work.connection.auth {
        crate::providers::AuthMethod::ApiKey => Some(work.connection.api_key.expose().to_owned()),
        crate::providers::AuthMethod::Plan => None,
    };
    let secret = secret.as_deref();
    let prepared = match prepare(&state, &work, secret).await {
        Ok(prepared) => prepared,
        Err(failed) => {
            settle(
                &state,
                work,
                AssistantReply::default(),
                failed.outcome,
                Some(failed.error),
                secret,
                false,
                None,
            );
            return;
        }
    };
    let ended =
        crate::execution::run_agent_action(&state, prepared.spec, prepared.turns, work.job.clone())
            .await;
    let mut outcome = ended.outcome;
    let mut error = ended.error;
    let mut cleanup_failed = false;
    if let Some(guest) = prepared.guest {
        let sandbox_gone = dispose_guest(&state, &guest.sandbox, guest.attempt).await;
        let workspace_gone = sandbox_gone && guest.workspace.destroy().is_ok();
        if state
            .conversation_runtime
            .finish(conversation, !sandbox_gone, !workspace_gone)
            .is_err()
            || !sandbox_gone
            || !workspace_gone
        {
            cleanup_failed = true;
            outcome = AgentOutcome::PersistenceFailure;
            error = Some(
                "Power Plant could not clean up the sandbox. This operation retains its reservations."
                    .to_owned(),
            );
        }
    }
    settle(
        &state,
        work,
        ended.reply,
        outcome,
        error,
        secret,
        cleanup_failed,
        ended.budget,
    );
}

async fn run_file_change(state: AppState, mut work: OrdinaryRun) {
    let Some(file) = work.file.take() else {
        settle(
            &state,
            work,
            AssistantReply::default(),
            AgentOutcome::PersistenceFailure,
            Some(persist_error().to_owned()),
            None,
            false,
            None,
        );
        return;
    };
    let Some(run) = state.workflow_runs.get(&file.run_id) else {
        settle(
            &state,
            work,
            AssistantReply::default(),
            AgentOutcome::PersistenceFailure,
            Some(persist_error().to_owned()),
            None,
            false,
            None,
        );
        return;
    };
    if run.conversation_id != Some(work.record.id) {
        settle(
            &state,
            work,
            AssistantReply::default(),
            AgentOutcome::AuthorityFailure,
            Some(
                "The prepared changes belong to another conversation or need recovery.".to_owned(),
            ),
            None,
            false,
            None,
        );
        return;
    }
    let host_policy = file.project_free.policy.clone();
    let job = WorkflowJob {
        run_id: file.run_id,
        session_id: work.session,
        agent_id: run.agent_id,
        agent_revision: work.record.revision,
        conversation_id: Some(work.record.id),
        authority: None,
        project_free_authority: Some(file.project_free),
        grant_alias: String::new(),
        connection: work.connection.clone(),
        phase_providers: run
            .model_phases()
            .map(|phase| phase.selection.provider)
            .collect(),
        active_connection: std::sync::Arc::new(std::sync::Mutex::new(None)),
        host_policy,
        turns: work.turns.clone(),
        job: work.job.clone(),
        eligible_reply: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
    };
    crate::workflows::drive_ordinary_file_run(state, job, work.execution).await;
}

struct Prepared {
    spec: AgentRunSpec,
    turns: Vec<crate::providers::ChatTurn>,
    guest: Option<PreparedGuest>,
}

struct PreparedGuest {
    sandbox: Arc<GuestSandbox>,
    attempt: AttemptId,
    workspace: crate::workflows::workspace::AttemptWorkspace,
}

struct PrepareFailure {
    outcome: AgentOutcome,
    error: String,
}

async fn prepare(
    state: &AppState,
    work: &OrdinaryRun,
    secret: Option<&str>,
) -> Result<Prepared, PrepareFailure> {
    let Some(model) = work.record.model.as_ref() else {
        return Err(prepare_failure(
            AgentOutcome::AuthorityFailure,
            "This conversation has no execution settings.",
        ));
    };
    let settings = model.settings.clone();
    let mut preamble = settings.instructions.trim().to_owned();
    append_block(
        &mut preamble,
        &crate::workflows::input_context::authorised_source_text(&settings, false),
    );
    if work.kind == OrdinaryKind::Host {
        append_block(&mut preamble, &host_policy_text(&settings));
    }
    let (policy, sandbox, guest, location) = match work.kind {
        OrdinaryKind::Host => (
            DirectoryPolicy::from_grants(Vec::new(), String::new()),
            None,
            None,
            ToolLocation::Host,
        ),
        OrdinaryKind::Sandbox => {
            let prepared =
                prepare_sandbox(state, work.record.id, work.job.id(), &settings, secret).await?;
            append_block(&mut preamble, &prepared.instructions);
            (
                prepared.policy,
                Some(prepared.sandbox.clone()),
                Some(PreparedGuest {
                    sandbox: prepared.sandbox,
                    attempt: prepared.attempt,
                    workspace: prepared.workspace,
                }),
                ToolLocation::Sandbox,
            )
        }
        OrdinaryKind::FileChange => {
            return Err(prepare_failure(
                AgentOutcome::PersistenceFailure,
                persist_error(),
            ));
        }
    };
    if let Some(language) = state.sessions.language(&work.session) {
        language.append_instructions(&mut preamble);
    }
    let tools = crate::tools::advertised(&settings.tools, location);
    let host = (location == ToolLocation::Host).then(|| crate::tools::HostRunSpec {
        session: work.session,
        conversation: work.record.id,
        execution_revision: work.record.revision,
        directory: crate::execution::command_directory(&settings.directories),
        settings,
        run: None,
        step: None,
        attempt: None,
    });
    Ok(Prepared {
        spec: AgentRunSpec {
            agent_id: None,
            revision: work.record.revision,
            preamble,
            tools: crate::tools::definitions_for(&tools, location),
            tool_ids: tools,
            policy,
            connection: work.connection.clone(),
            location,
            sandbox,
            host,
            output_drafts: None,
            required_outputs: Vec::new(),
            evidence: None,
            output_scope: Some(OutputScope::conversation(work.record.id)),
            conversation: Some(work.record.id),
            steering_session: Some(work.session),
            budget: crate::execution::BudgetPolicy::ordinary(),
        },
        turns: work.turns.clone(),
        guest,
    })
}

struct SandboxPrepared {
    policy: DirectoryPolicy,
    sandbox: Arc<GuestSandbox>,
    attempt: AttemptId,
    workspace: crate::workflows::workspace::AttemptWorkspace,
    instructions: String,
}

async fn prepare_sandbox(
    state: &AppState,
    conversation: ConversationId,
    job: crate::sessions::JobId,
    settings: &ExecutionSettings,
    secret: Option<&str>,
) -> Result<SandboxPrepared, PrepareFailure> {
    let authority = ProjectFreeAuthority::from_settings(1, settings)
        .map_err(|error| prepare_failure(AgentOutcome::AuthorityFailure, error.message()))?;
    let run = RunId::generate()
        .map_err(|_| prepare_failure(AgentOutcome::PersistenceFailure, persist_error()))?;
    let attempt = AttemptId::generate()
        .map_err(|_| prepare_failure(AgentOutcome::PersistenceFailure, persist_error()))?;
    state
        .conversation_runtime
        .remember(conversation, job, run, attempt)
        .map_err(|error| prepare_failure(AgentOutcome::PersistenceFailure, error))?;
    let workspace = match state.workflow_workspaces.create_attempt(run, attempt) {
        Ok(workspace) => workspace,
        Err(error) => {
            let leftover = error.orphaned;
            let _ = state
                .conversation_runtime
                .finish(conversation, false, leftover);
            return Err(prepare_failure(
                AgentOutcome::PersistenceFailure,
                "Power Plant could not create the attempt workspace.",
            ));
        }
    };
    let spec = match sandbox_spec(&workspace.project, &authority, settings.network.clone()) {
        Ok(spec) => spec,
        Err(error) => {
            let workspace_gone = workspace.destroy().is_ok();
            let _ = state
                .conversation_runtime
                .finish(conversation, false, !workspace_gone);
            return Err(prepare_failure(AgentOutcome::AuthorityFailure, error));
        }
    };
    for grant in authority.policy.grants() {
        if let Err(error) =
            crate::sandbox::confirm_host_write_access(&spec, &grant.host_path, grant.access)
        {
            let workspace_gone = workspace.destroy().is_ok();
            let _ = state
                .conversation_runtime
                .finish(conversation, false, !workspace_gone);
            return Err(prepare_failure(
                AgentOutcome::AuthorityFailure,
                error.message(),
            ));
        }
    }
    if let Err(error) = crate::workflows::validate_replacement_environment(
        &state.environments,
        &state.environment_snapshots,
        settings.environment,
    )
    .await
    {
        let workspace_gone = workspace.destroy().is_ok();
        let _ = state
            .conversation_runtime
            .finish(conversation, false, !workspace_gone);
        return Err(prepare_failure(
            AgentOutcome::AuthorityFailure,
            error.message(),
        ));
    }
    let pointer = match state.environments.copy_ready_pointer(&settings.environment) {
        Ok(pointer) => pointer,
        Err(crate::environments::ReadyPointerError::Missing) => {
            let workspace_gone = workspace.destroy().is_ok();
            let _ = state
                .conversation_runtime
                .finish(conversation, false, !workspace_gone);
            return Err(prepare_failure(
                AgentOutcome::AuthorityFailure,
                "That environment is no longer in the catalogue.",
            ));
        }
        Err(crate::environments::ReadyPointerError::NotReady) => {
            let workspace_gone = workspace.destroy().is_ok();
            let _ = state
                .conversation_runtime
                .finish(conversation, false, !workspace_gone);
            return Err(prepare_failure(
                AgentOutcome::AuthorityFailure,
                "That environment is not ready.",
            ));
        }
    };
    let path = match state
        .environment_snapshots
        .restore_path(&pointer.snapshot.artifact_key)
    {
        Ok(path) => path,
        Err(_) => {
            let workspace_gone = workspace.destroy().is_ok();
            let _ = state
                .conversation_runtime
                .finish(conversation, false, !workspace_gone);
            return Err(prepare_failure(
                AgentOutcome::AuthorityFailure,
                "That environment snapshot is unavailable.",
            ));
        }
    };
    let sandbox = state.sandboxes.attempt_handle(run, attempt);
    if let Err(error) = sandbox
        .start_from_snapshot(&path, pointer.snapshot.snapshot_digest.as_str(), spec)
        .await
    {
        let sandbox_gone = dispose_guest(state, &sandbox, attempt).await;
        let workspace_gone = sandbox_gone && workspace.destroy().is_ok();
        let _ = state
            .conversation_runtime
            .finish(conversation, !sandbox_gone, !workspace_gone);
        return Err(prepare_failure(
            AgentOutcome::AuthorityFailure,
            error.message(),
        ));
    }
    let instructions = match crate::workflows::input_context::read_directory_instructions(
        &sandbox, &authority, secret,
    )
    .await
    {
        Ok(crate::workflows::input_context::ProjectInstructions::Absent) => String::new(),
        Ok(crate::workflows::input_context::ProjectInstructions::Present(text)) => text,
        Err(error) => {
            let sandbox_gone = dispose_guest(state, &sandbox, attempt).await;
            let workspace_gone = sandbox_gone && workspace.destroy().is_ok();
            let _ = state
                .conversation_runtime
                .finish(conversation, !sandbox_gone, !workspace_gone);
            return Err(prepare_failure(
                AgentOutcome::AuthorityFailure,
                error.message(),
            ));
        }
    };
    Ok(SandboxPrepared {
        policy: authority.policy,
        sandbox,
        attempt,
        workspace,
        instructions,
    })
}

pub(crate) fn sandbox_spec(
    scratch: &Path,
    authority: &ProjectFreeAuthority,
    network: crate::agents::NetworkAccess,
) -> Result<SandboxSpec, &'static str> {
    let mut mounts = vec![MountSpec {
        guest: crate::execution::GUEST_WORKSPACE.to_owned(),
        host: scratch.to_path_buf(),
        read_only: false,
    }];
    for grant in authority.policy.grants() {
        if grant.access.is_writable() {
            return Err("Host write access requires an authorised Direct write grant.");
        }
        if grant.host_path == scratch {
            return Err("Private scratch access is distinct from host directory grants.");
        }
        mounts.push(MountSpec {
            guest: grant.guest_path.clone(),
            host: grant.host_path.clone(),
            read_only: true,
        });
    }
    Ok(SandboxSpec {
        workdir: authority
            .policy
            .grants()
            .first()
            .map(|grant| grant.guest_path.clone())
            .unwrap_or_else(|| crate::execution::GUEST_WORKSPACE.to_owned()),
        mounts,
        network,
    })
}

async fn dispose_guest(state: &AppState, sandbox: &Arc<GuestSandbox>, attempt: AttemptId) -> bool {
    let stopped = sandbox.stop().await.is_ok();
    let gone = stopped && sandbox.remove().await.is_ok();
    if gone {
        state.sandboxes.drop_attempt(attempt);
    } else {
        state.sandboxes.expose_orphan(sandbox.name().to_owned());
    }
    gone
}

fn host_policy_text(settings: &ExecutionSettings) -> String {
    let approval = if settings.automatic_host_commands() {
        "This conversation authorises Run without approval."
    } else {
        "Each shell command waits for user approval bound to this conversation, job and settings revision."
    };
    format!(
        "Tools run on this computer as the Power Plant process user. {approval} Approval does not inspect script internals. Command output is sent to the hosted model. Sandbox guest paths such as /access/<alias> and /workspace from earlier turns are not host paths and grant no authority."
    )
}

fn append_block(preamble: &mut String, block: &str) {
    let block = block.trim();
    if block.is_empty() {
        return;
    }
    if !preamble.is_empty() {
        preamble.push_str("\n\n");
    }
    preamble.push_str(block);
}

#[allow(clippy::too_many_arguments)]
fn settle(
    state: &AppState,
    work: OrdinaryRun,
    reply: AssistantReply,
    outcome: AgentOutcome,
    error: Option<String>,
    secret: Option<&str>,
    retain: bool,
    budget: Option<crate::execution::BudgetSnapshot>,
) {
    let OrdinaryRun {
        session,
        record,
        job,
        execution,
        ..
    } = work;
    let conversation = record.id;
    if outcome == AgentOutcome::BudgetExhausted {
        if retain || state.conversation_runtime.unsettled(conversation) {
            execution.require_recovery();
            let _ = job.finish(
                JobStatus::Failed,
                Some(
                    "Execution remains unsettled. This operation retains its reservations until recovery and cleanup finish.",
                ),
            );
            return;
        }
        settle_pause(
            state,
            session,
            &record,
            &job,
            Some(execution),
            reply,
            budget,
        );
        return;
    }
    let (status, message_status) = match outcome {
        AgentOutcome::Completed => (JobStatus::Completed, MessageStatus::Complete),
        AgentOutcome::Cancelled => (JobStatus::Cancelled, MessageStatus::Interrupted),
        AgentOutcome::ProviderFailure
        | AgentOutcome::ToolFailure
        | AgentOutcome::AuthorityFailure
        | AgentOutcome::PersistenceFailure
        | AgentOutcome::UncertainEffect
        | AgentOutcome::BudgetExhausted
        | AgentOutcome::ContextBlocked => (JobStatus::Failed, MessageStatus::Failed),
    };
    let error = error
        .and_then(|text| crate::providers::sanitise_detail(&crate::tools::redact(&text, secret)));
    let settlement = state.conversations.settle_message(
        &conversation,
        job.id(),
        reply,
        message_status,
        error.clone(),
    );
    if settlement.is_ok() {
        crate::conversations::titles::start(state, conversation, state.sessions.language(&session));
    }
    if retain || state.conversation_runtime.unsettled(conversation) || settlement.is_err() {
        execution.require_recovery();
        let _ = job.finish(
            JobStatus::Failed,
            Some(
                "Execution remains unsettled. This operation retains its reservations until recovery and cleanup finish.",
            ),
        );
        return;
    }
    job.finish(status, error.as_deref());
    state
        .sessions
        .finish_conversation_job(&session, conversation, job.id());
    drop(execution);
}

pub(crate) fn settle_pause(
    state: &AppState,
    session: crate::sessions::SessionId,
    record: &ConversationRecord,
    job: &Arc<Job>,
    execution: Option<ExecutionGuard>,
    reply: AssistantReply,
    budget: Option<crate::execution::BudgetSnapshot>,
) {
    let conversation = record.id;
    let fail = |execution: Option<ExecutionGuard>, job: &Arc<Job>| {
        if let Some(execution) = execution {
            execution.require_recovery();
        }
        let _ = job.finish(
            JobStatus::Failed,
            Some("The execution budget pause could not be recorded."),
        );
    };
    if job.cancel_requested() {
        if state
            .conversations
            .settle_message(
                &conversation,
                job.id(),
                reply,
                MessageStatus::Interrupted,
                None,
            )
            .is_err()
        {
            fail(execution, job);
            return;
        }
        job.finish(JobStatus::Cancelled, None);
        state
            .sessions
            .finish_conversation_job(&session, conversation, job.id());
        return;
    }
    let Some(budget) = budget.filter(|snapshot| snapshot.valid()) else {
        fail(execution, job);
        return;
    };
    let Some(model) = record.model.clone() else {
        fail(execution, job);
        return;
    };
    let Ok(id) = crate::conversations::CheckpointId::generate() else {
        fail(execution, job);
        return;
    };
    let Ok(boundary) = crate::conversations::MessageId::generate() else {
        fail(execution, job);
        return;
    };
    let checkpoint = crate::conversations::ContinuationCheckpoint {
        id,
        boundary,
        pinned: model.settings,
        budget,
        run: None,
        attempt: None,
        step: None,
        drafts: Vec::new(),
        created_at_ms: crate::workflows::now_ms(),
    };
    if state
        .conversations
        .pause_for_budget(&conversation, job.id(), reply, checkpoint)
        .is_err()
    {
        fail(execution, job);
        return;
    }
    job.finish(JobStatus::Completed, None);
    state
        .sessions
        .finish_conversation_job(&session, conversation, job.id());
    drop(execution);
}

fn prepare_failure(outcome: AgentOutcome, error: impl Into<String>) -> PrepareFailure {
    PrepareFailure {
        outcome,
        error: error.into(),
    }
}

fn persist_error() -> &'static str {
    "Power Plant could not store conversation execution state."
}

fn load_dir(dir: &Path) -> Result<BTreeMap<ConversationId, RuntimeRecord>, &'static str> {
    let mut records = BTreeMap::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(records),
        Err(_) => return Err(persist_error()),
    };
    for entry in entries {
        let entry = entry.map_err(|_| persist_error())?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let bytes = crate::storage::read_private_bounded(&path, MAXIMUM_RUNTIME_RECORD_BYTES)
            .map_err(|_| persist_error())?;
        let file: RuntimeFile = serde_json::from_slice(&bytes).map_err(|_| persist_error())?;
        if file.version != RUNTIME_RECORD_VERSION {
            return Err(persist_error());
        }
        let conversation = ConversationId::parse(&file.conversation).ok_or(persist_error())?;
        if path != record_path(dir, conversation) || records.contains_key(&conversation) {
            return Err(persist_error());
        }
        let job = crate::sessions::JobId::parse(&file.job).ok_or(persist_error())?;
        let run = RunId::parse(&file.run).ok_or(persist_error())?;
        let attempt = AttemptId::parse(&file.attempt).ok_or(persist_error())?;
        records.insert(
            conversation,
            RuntimeRecord {
                job,
                run,
                attempt,
                sandbox: file.sandbox,
                workspace: file.workspace,
            },
        );
    }
    Ok(records)
}

fn persist_record(
    dir: Option<&Path>,
    conversation: ConversationId,
    record: &RuntimeRecord,
) -> Result<(), &'static str> {
    let Some(dir) = dir else {
        return Ok(());
    };
    let file = RuntimeFile {
        version: RUNTIME_RECORD_VERSION,
        conversation: conversation.to_string(),
        job: record.job.to_string(),
        run: record.run.to_string(),
        attempt: record.attempt.to_string(),
        sandbox: record.sandbox,
        workspace: record.workspace,
    };
    let bytes = serde_json::to_vec(&file).map_err(|_| persist_error())?;
    crate::storage::write_private(&record_path(dir, conversation), &bytes)
        .map_err(|_| persist_error())
}

fn remove_record(dir: Option<&Path>, conversation: ConversationId) -> Result<(), &'static str> {
    let Some(dir) = dir else {
        return Ok(());
    };
    crate::storage::remove_private(&record_path(dir, conversation)).map_err(|_| persist_error())
}

fn record_path(dir: &Path, conversation: ConversationId) -> PathBuf {
    dir.join(format!("{conversation}.json"))
}
