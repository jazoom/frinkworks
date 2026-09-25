use super::{OrdinaryKind, ordinary_kind, sandbox_spec};
use crate::agents::{NetworkAccess, ToolId};
use crate::execution::{DirectoryAccess, ExecutionSettings, ProjectFreeAuthority, ToolLocation};
use crate::providers::ProviderKind;

impl super::ConversationRuntime {
    pub(crate) fn ephemeral() -> Self {
        Self {
            dir: None,
            inner: std::sync::Mutex::new(std::collections::BTreeMap::new()),
        }
    }
}

#[test]
fn failed_preparation_retains_reservations_until_cleanup_settles() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let session = crate::sessions::generate_session_token().unwrap().id();
    state.sessions.insert(session);
    let record = state.conversations.create("Cleanup".to_owned()).unwrap();
    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .unwrap();
    let record = state
        .conversations
        .begin_message_with_model(
            &record.id,
            record.revision,
            None,
            job.id(),
            "Read the project".to_owned(),
        )
        .unwrap();
    state
        .conversation_runtime
        .remember(
            record.id,
            job.id(),
            crate::workflows::RunId::generate().unwrap(),
            crate::workflows::AttemptId::generate().unwrap(),
        )
        .unwrap();
    state
        .conversation_runtime
        .finish(record.id, false, true)
        .unwrap();
    let execution = state.workflow_execution.acquire().unwrap();
    super::settle(
        &state,
        super::OrdinaryRun {
            session,
            record: record.clone(),
            connection: crate::providers::ProviderConnection::with_key(
                ProviderKind::Xai,
                "test-key",
                "grok-4.6",
            ),
            job,
            execution,
            kind: OrdinaryKind::Sandbox,
            turns: Vec::new(),
            file: None,
        },
        crate::providers::AssistantReply::default(),
        super::AgentOutcome::AuthorityFailure,
        Some("The environment is unavailable.".to_owned()),
        None,
        false,
        None,
    );
    assert!(state.sessions.conversation_reserved(record.id));
    assert!(state.workflow_execution.acquire_exclusive().is_err());
    assert!(state.conversation_runtime.unsettled(record.id));
    #[cfg(feature = "dev")]
    {
        assert!(!state.sessions.has_active_work());
        assert!(!state.workflow_execution.has_active_work());
    }
}

#[test]
fn recovery_requires_absent_guest_and_workspace_before_release() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = super::ConversationRuntime::open(dir.path().to_owned()).unwrap();
    let conversation = crate::conversations::ConversationId::generate().unwrap();
    let run = crate::workflows::RunId::generate().unwrap();
    let attempt = crate::workflows::AttemptId::generate().unwrap();
    runtime
        .remember(
            conversation,
            crate::sessions::JobId::generate().unwrap(),
            run,
            attempt,
        )
        .unwrap();
    let runtime = super::ConversationRuntime::open(dir.path().to_owned()).unwrap();
    let mut guests = crate::sandbox::TransientGuestRecovery::default();
    let execution = std::sync::Arc::new(crate::workflows::WorkflowExecution::new());
    runtime.recover(&guests, &[], &execution).unwrap();
    assert!(runtime.unsettled(conversation));
    assert!(execution.acquire_exclusive().is_err());
    guests.inventory_complete = true;
    let workspace = crate::workflows::workspace::WorkspaceRecovery {
        run,
        attempt,
        remains: true,
    };
    runtime
        .recover(
            &guests,
            &[workspace],
            &std::sync::Arc::new(crate::workflows::WorkflowExecution::new()),
        )
        .unwrap();
    assert!(runtime.unsettled(conversation));
    runtime
        .recover(
            &guests,
            &[],
            &std::sync::Arc::new(crate::workflows::WorkflowExecution::new()),
        )
        .unwrap();
    assert!(
        !super::ConversationRuntime::open(dir.path().to_owned())
            .unwrap()
            .unsettled(conversation)
    );
}

#[test]
fn runtime_records_reject_wrong_identity_and_oversized_content() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = super::ConversationRuntime::open(dir.path().to_owned()).unwrap();
    let conversation = crate::conversations::ConversationId::generate().unwrap();
    runtime
        .remember(
            conversation,
            crate::sessions::JobId::generate().unwrap(),
            crate::workflows::RunId::generate().unwrap(),
            crate::workflows::AttemptId::generate().unwrap(),
        )
        .unwrap();
    let path = super::record_path(dir.path(), conversation);
    let wrong = dir.path().join("wrong.json");
    std::fs::rename(&path, &wrong).unwrap();
    assert!(super::ConversationRuntime::open(dir.path().to_owned()).is_err());
    std::fs::remove_file(wrong).unwrap();
    std::fs::write(path, vec![b' '; super::MAXIMUM_RUNTIME_RECORD_BYTES + 1]).unwrap();
    assert!(super::ConversationRuntime::open(dir.path().to_owned()).is_err());
}

fn execution_settings(tools: Vec<ToolId>, location: ToolLocation) -> ExecutionSettings {
    ExecutionSettings::new(
        crate::providers::ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None)
            .unwrap(),
        String::new(),
        tools,
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_location(location)
}

#[test]
fn ordinary_kind_covers_host_sandbox_and_file_change() {
    assert_eq!(
        ordinary_kind(&execution_settings(vec![ToolId::Run], ToolLocation::Host)),
        Some(OrdinaryKind::Host)
    );
    assert_eq!(
        ordinary_kind(&execution_settings(
            vec![ToolId::Run],
            ToolLocation::Sandbox
        )),
        Some(OrdinaryKind::Sandbox)
    );
    assert_eq!(
        ordinary_kind(&execution_settings(Vec::new(), ToolLocation::Host)),
        None
    );
    let directory = tempfile::tempdir().unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(directory.path(), &[]).unwrap();
    let host_with_directory = execution_settings(vec![ToolId::Run], ToolLocation::Host)
        .with_directories(vec![grant.clone()])
        .unwrap();
    assert_eq!(
        ordinary_kind(&host_with_directory),
        Some(OrdinaryKind::FileChange)
    );
    let read_only = execution_settings(vec![ToolId::Read], ToolLocation::Sandbox)
        .with_directories(vec![grant.clone()])
        .unwrap();
    assert_eq!(ordinary_kind(&read_only), Some(OrdinaryKind::Sandbox));
    let mut writable = grant.clone();
    writable.access = DirectoryAccess::DirectWrite;
    let sandbox_write = execution_settings(vec![ToolId::Read], ToolLocation::Sandbox)
        .with_directories(vec![writable])
        .unwrap();
    assert_eq!(
        ordinary_kind(&sandbox_write),
        Some(OrdinaryKind::FileChange)
    );
    let mut reviewed = grant;
    reviewed.access = DirectoryAccess::ReviewBeforeApply;
    let sandbox_review = execution_settings(vec![ToolId::Read], ToolLocation::Sandbox)
        .with_directories(vec![reviewed])
        .unwrap();
    assert_eq!(
        ordinary_kind(&sandbox_review),
        Some(OrdinaryKind::FileChange)
    );
}

#[test]
fn sandbox_spec_binds_grant_identity_and_keeps_scratch_distinct() {
    let project = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(project.path(), &[]).unwrap();
    let configured = execution_settings(vec![ToolId::Read], ToolLocation::Sandbox)
        .with_directories(vec![grant.clone()])
        .unwrap();
    let authority = ProjectFreeAuthority::from_settings(1, &configured).unwrap();
    let spec = sandbox_spec(scratch.path(), &authority, NetworkAccess::None).unwrap();
    assert_eq!(spec.workdir, grant.guest_path());
    assert_eq!(spec.mounts[0].guest, crate::execution::GUEST_WORKSPACE);
    assert!(!spec.mounts[0].read_only);
    assert_eq!(spec.mounts[0].host, scratch.path());
    assert_eq!(spec.mounts[1].guest, grant.guest_path());
    assert_eq!(spec.mounts[1].host, grant.host_path);
    assert!(spec.mounts[1].read_only);
}

#[test]
fn sandbox_spec_rejects_writable_host_grants_and_scratch_overlap() {
    let project = tempfile::tempdir().unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(project.path(), &[]).unwrap();
    let configured = execution_settings(vec![ToolId::Read], ToolLocation::Sandbox)
        .with_directories(vec![grant.clone()])
        .unwrap();
    let mut writable = ProjectFreeAuthority::from_snapshot(1, &configured).unwrap();
    writable.policy = crate::agents::DirectoryPolicy::from_grants_with_workspace(
        vec![crate::agents::PolicyGrant {
            alias: grant.alias.clone(),
            guest_path: grant.guest_path(),
            host_path: grant.host_path.clone(),
            access: crate::agents::AccessMode::ReadWrite,
        }],
        grant.alias.clone(),
    );
    assert!(sandbox_spec(project.path(), &writable, NetworkAccess::None).is_err());
    let readonly = ProjectFreeAuthority::from_snapshot(1, &configured).unwrap();
    assert!(sandbox_spec(grant.host_path.as_path(), &readonly, NetworkAccess::None).is_err());
}

#[test]
fn changed_directory_identity_cannot_mint_authority() {
    let project = tempfile::tempdir().unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(project.path(), &[]).unwrap();
    let mut stale = grant.clone();
    stale.identity.inode = stale.identity.inode.wrapping_add(1);
    let stale_settings = execution_settings(vec![ToolId::Read], ToolLocation::Sandbox)
        .with_directories(vec![stale])
        .unwrap();
    assert!(ProjectFreeAuthority::from_settings(1, &stale_settings).is_err());
    let live = execution_settings(vec![ToolId::Read], ToolLocation::Sandbox)
        .with_directories(vec![grant])
        .unwrap();
    assert!(ProjectFreeAuthority::from_settings(1, &live).is_ok());
}

#[test]
fn scratch_only_spec_has_private_workspace_and_no_host_mount() {
    let scratch = tempfile::tempdir().unwrap();
    let configured = execution_settings(vec![ToolId::Run], ToolLocation::Sandbox);
    let authority = ProjectFreeAuthority::from_settings(1, &configured).unwrap();
    let spec = sandbox_spec(scratch.path(), &authority, NetworkAccess::None).unwrap();
    assert_eq!(spec.mounts.len(), 1);
    assert_eq!(spec.mounts[0].guest, crate::execution::GUEST_WORKSPACE);
    assert!(!spec.mounts[0].read_only);
    assert_eq!(spec.workdir, crate::execution::GUEST_WORKSPACE);
}

#[tokio::test]
async fn ordinary_host_run_keeps_tool_history_without_a_workflow() {
    use crate::config::RuntimeConfig;
    use crate::providers::{
        ChatBackend, ChatTurn, CompletionReason, ModelEvent, ProviderConnection,
    };
    let mut state = crate::tests::test_state(RuntimeConfig::development());
    let records = tempfile::tempdir().unwrap();
    state.conversations = std::sync::Arc::new(
        crate::conversations::ConversationStore::open(records.path().to_owned()).unwrap(),
    );
    let global = tempfile::tempdir().unwrap();
    state.skills =
        std::sync::Arc::new(crate::skills::SkillStore::open(global.path().to_owned()).unwrap());
    let skill = state
        .skills
        .create("---\nname: global-guidance\ndescription: Guidance without a project\n---\nGlobal skill body.".to_owned())
        .unwrap();
    let skill_path = global.path().join(&skill.directory).join("SKILL.md");
    let backend = crate::tests::ScriptedBackend::rounds(vec![
        vec![
            Ok(ModelEvent::ToolCall {
                id: "call-1".to_owned(),
                name: "run".to_owned(),
                arguments: serde_json::json!({
                    "command": "printf one",
                    "explanation": "Print the first marker",
                }),
            }),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::ToolCalls,
            }),
        ],
        vec![
            Ok(ModelEvent::ToolCall {
                id: "call-2".to_owned(),
                name: "read".to_owned(),
                arguments: serde_json::json!({"path": skill_path}),
            }),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::ToolCalls,
            }),
        ],
        vec![
            Ok(ModelEvent::Text("Both commands finished.".to_owned())),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::Stop,
            }),
        ],
    ]);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend));
    let connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    state.vault.put(connection.clone()).unwrap();
    let token = crate::sessions::generate_session_token().unwrap();
    let session = token.id();
    state.sessions.insert(session);
    let record = state.conversations.create("Host".to_owned()).unwrap();
    let host = execution_settings(vec![ToolId::Run, ToolId::Read], ToolLocation::Host)
        .with_host_approval(crate::execution::HostApprovalPolicy::Automatic);
    let record = state
        .conversations
        .update_execution_settings(&record.id, record.revision, host.clone())
        .unwrap();
    state
        .access_consent
        .approve_host_conversation(
            &state
                .access_consent
                .request_host_conversation(session, record.id, &host)
                .unwrap(),
            session,
            record.id,
            &host,
        )
        .unwrap();
    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .unwrap();
    let record = state
        .conversations
        .begin_message_with_model(
            &record.id,
            record.revision,
            None,
            job.id(),
            "Run both commands".to_owned(),
        )
        .unwrap();
    let execution = state.workflow_execution.acquire().unwrap();
    super::run(
        state.clone(),
        super::OrdinaryRun {
            session,
            record: record.clone(),
            connection,
            job,
            execution,
            kind: OrdinaryKind::Host,
            turns: vec![ChatTurn::user("Run both commands".to_owned())],
            file: None,
        },
    )
    .await;
    assert!(state.workflow_runs.summaries().is_empty());
    let reopened =
        crate::conversations::ConversationStore::open(records.path().to_owned()).unwrap();
    let settled = reopened.get(&record.id).unwrap();
    assert!(settled.active_job.is_none());
    let history = crate::conversations::history::project(&settled.messages, None).unwrap();
    assert_eq!(history.iter().flat_map(|turn| &turn.calls).count(), 2);
    assert_eq!(
        history
            .iter()
            .flat_map(|turn| &turn.calls)
            .filter(|call| {
                call.result
                    .as_ref()
                    .is_some_and(|result| result.command.is_some())
            })
            .count(),
        1
    );
    let source = history
        .iter()
        .flat_map(|turn| &turn.calls)
        .filter_map(|call| call.result.as_ref()?.resource.as_ref())
        .next()
        .unwrap();
    assert_eq!(source.kind, crate::execution::ResourceKind::Skill);
    assert_eq!(source.path, skill_path.to_str().unwrap());
    assert!(!state.conversation_runtime.unsettled(record.id));
}

fn host_file_change_settings(
    state: &crate::state::AppState,
    grant: crate::execution::DirectoryGrant,
) -> ExecutionSettings {
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    ExecutionSettings::new(
        crate::providers::ModelSelection::new(
            ProviderKind::Xai,
            "grok-4.6".to_owned(),
            Some(effort),
        )
        .unwrap(),
        String::new(),
        vec![ToolId::Run],
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_location(ToolLocation::Host)
    .with_directories(vec![grant])
    .unwrap()
    .with_host_approval(crate::execution::HostApprovalPolicy::Automatic)
}

fn start_file_change_conversation(
    state: &crate::state::AppState,
    session: crate::sessions::SessionId,
    settings: ExecutionSettings,
    text: &str,
) -> (
    crate::conversations::ConversationRecord,
    std::sync::Arc<crate::sessions::Job>,
) {
    let record = state
        .conversations
        .create("File change".to_owned())
        .unwrap();
    let record = state
        .conversations
        .update_execution_settings(&record.id, record.revision, settings.clone())
        .unwrap();
    state
        .access_consent
        .approve_host_conversation(
            &state
                .access_consent
                .request_host_conversation(session, record.id, &settings)
                .unwrap(),
            session,
            record.id,
            &settings,
        )
        .unwrap();
    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .unwrap();
    let record = state
        .conversations
        .begin_message_with_model(&record.id, record.revision, None, job.id(), text.to_owned())
        .unwrap();
    (record, job)
}

fn file_change_run(
    state: &crate::state::AppState,
    conversation: crate::conversations::ConversationId,
    settings: &ExecutionSettings,
) -> (
    crate::workflows::RunId,
    crate::execution::ProjectFreeAuthority,
) {
    let pinned = crate::workflows::pin_agent_work(settings).unwrap();
    let project_free = crate::execution::ProjectFreeAuthority::from_snapshot(1, settings).unwrap();
    let run_id = crate::workflows::RunId::generate().unwrap();
    let phase_models = pinned
        .definition
        .steps()
        .iter()
        .filter(|step| {
            matches!(
                &step.action,
                crate::workflows::definition::StepAction::Agent(_)
            )
        })
        .map(|step| crate::workflows::PhaseModelSelection {
            step: step.key.clone(),
            selection: settings.model.clone(),
            instructions: settings.instructions.clone(),
            preset: None,
            settings: Some(settings.clone()),
        })
        .collect();
    let environments = crate::tests::test_environment_set(&pinned.definition);
    let mut run = crate::workflows::WorkflowRun::create_source_free_for_conversation(
        run_id,
        crate::workflows::now_ms(),
        conversation,
        pinned,
        environments,
        phase_models,
    );
    run.launch_brief = "Change the file".to_owned();
    state.workflow_runs.create(run).unwrap();
    (run_id, project_free)
}

#[tokio::test]
async fn file_change_host_run_binds_baseline_to_owned_conversation() {
    use crate::providers::{ChatBackend, ChatTurn, CompletionReason, ModelEvent};
    let mut state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let backend = crate::tests::ScriptedBackend::rounds(vec![
        vec![
            Ok(ModelEvent::ToolCall {
                id: "read-skill".to_owned(),
                name: "read".to_owned(),
                arguments: serde_json::json!({"path": ".agents/skills/local/SKILL.md"}),
            }),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::ToolCalls,
            }),
        ],
        vec![
            Ok(ModelEvent::ToolCall {
                id: "write-note".to_owned(),
                name: "write".to_owned(),
                arguments: serde_json::json!({
                    "path": "note.txt",
                    "contents": "after\n",
                }),
            }),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::ToolCalls,
            }),
        ],
        vec![
            Ok(ModelEvent::Text("Done.".to_owned())),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::Stop,
            }),
        ],
    ]);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend.clone()));
    let connection =
        crate::providers::ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    state.vault.put(connection.clone()).unwrap();
    let token = crate::sessions::generate_session_token().unwrap();
    let session = token.id();
    state.sessions.insert(session);
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("note.txt"), "before\n").unwrap();
    let skill_path = directory.path().join(".agents/skills/local/SKILL.md");
    std::fs::create_dir_all(skill_path.parent().unwrap()).unwrap();
    std::fs::write(
        &skill_path,
        "---\nname: local\ndescription: Local guidance\n---\nPrivate skill body.\n",
    )
    .unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(directory.path(), &[]).unwrap();
    let mut settings = host_file_change_settings(&state, grant);
    settings.tools = vec![ToolId::Read, ToolId::Write];
    settings.host_approval = crate::execution::HostApprovalPolicy::AskEachTime;
    let (record, job) =
        start_file_change_conversation(&state, session, settings.clone(), "Change the file");
    let (run_id, project_free) = file_change_run(&state, record.id, &settings);
    let execution = state.workflow_execution.acquire().unwrap();
    super::run(
        state.clone(),
        super::OrdinaryRun {
            session,
            record: record.clone(),
            connection,
            job,
            execution,
            kind: OrdinaryKind::FileChange,
            turns: vec![ChatTurn::user("Change the file".to_owned())],
            file: Some(super::FileChangeWork {
                run_id,
                project_free,
            }),
        },
    )
    .await;
    let preamble = backend.last_preamble().unwrap();
    assert!(preamble.contains(skill_path.to_str().unwrap()));
    assert!(!preamble.contains("Private skill body."));
    let tools = backend.last_tools();
    assert!(tools.contains(&"read".to_owned()));
    assert!(tools.contains(&"write".to_owned()));
    assert!(!tools.contains(&"run".to_owned()));
    let run = state.workflow_runs.get(&run_id).unwrap();
    assert_eq!(run.conversation_id, Some(record.id));
    assert_eq!(run.attempts.len(), 1);
    let changes = run.attempts[0].direct_changes.as_ref().unwrap();
    let diff = changes.diff(&state).unwrap();
    assert_eq!(
        diff.object(0, "base", &state.workflow_artefacts).unwrap().1,
        b"before\n"
    );
    assert_eq!(
        diff.object(0, "target", &state.workflow_artefacts)
            .unwrap()
            .1,
        b"after\n"
    );
    assert_eq!(
        std::fs::read(directory.path().join("note.txt")).unwrap(),
        b"after\n"
    );
    std::fs::write(directory.path().join("note.txt"), "later\n").unwrap();
    assert_eq!(
        diff.object(0, "target", &state.workflow_artefacts)
            .unwrap()
            .1,
        b"after\n"
    );
    let settled = state.conversations.get(&record.id).unwrap();
    assert!(settled.active_job.is_none());
    assert!(!state.conversation_runtime.unsettled(record.id));
}

#[tokio::test]
async fn stale_directory_identity_cannot_bind_file_change_work() {
    use crate::providers::{ChatBackend, ChatTurn, CompletionReason, ModelEvent};
    let mut state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let backend = crate::tests::ScriptedBackend::rounds(vec![vec![
        Ok(ModelEvent::Text("Done.".to_owned())),
        Ok(ModelEvent::Complete {
            reason: CompletionReason::Stop,
        }),
    ]]);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend));
    let connection =
        crate::providers::ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    state.vault.put(connection.clone()).unwrap();
    let token = crate::sessions::generate_session_token().unwrap();
    let session = token.id();
    state.sessions.insert(session);
    let directory = tempfile::tempdir().unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(directory.path(), &[]).unwrap();
    let settings = host_file_change_settings(&state, grant.clone());
    let (record, job) =
        start_file_change_conversation(&state, session, settings.clone(), "Change the file");
    let mut stale = settings.clone();
    stale.directories[0].identity.inode = grant.identity.inode.wrapping_add(1);
    let (run_id, project_free) = file_change_run(&state, record.id, &stale);
    let execution = state.workflow_execution.acquire().unwrap();
    super::run(
        state.clone(),
        super::OrdinaryRun {
            session,
            record: record.clone(),
            connection,
            job,
            execution,
            kind: OrdinaryKind::FileChange,
            turns: vec![ChatTurn::user("Change the file".to_owned())],
            file: Some(super::FileChangeWork {
                run_id,
                project_free,
            }),
        },
    )
    .await;
    let settled = state.conversations.get(&record.id).unwrap();
    assert!(settled.active_job.is_none());
    let run = state.workflow_runs.get(&run_id).unwrap();
    assert!(run.is_terminal());
    assert_ne!(
        run.state.as_label(),
        crate::workflows::run::RunState::Completed.as_label()
    );
}

#[tokio::test]
async fn file_change_run_rejects_foreign_conversation_owner() {
    use crate::providers::ChatTurn;
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let connection =
        crate::providers::ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    let token = crate::sessions::generate_session_token().unwrap();
    let session = token.id();
    state.sessions.insert(session);
    let directory = tempfile::tempdir().unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(directory.path(), &[]).unwrap();
    let settings = host_file_change_settings(&state, grant);
    let (record, job) =
        start_file_change_conversation(&state, session, settings.clone(), "Change the file");
    let foreign = state.conversations.create("Other".to_owned()).unwrap();
    let (run_id, project_free) = file_change_run(&state, foreign.id, &settings);
    let execution = state.workflow_execution.acquire().unwrap();
    super::run(
        state.clone(),
        super::OrdinaryRun {
            session,
            record: record.clone(),
            connection,
            job,
            execution,
            kind: OrdinaryKind::FileChange,
            turns: vec![ChatTurn::user("Change the file".to_owned())],
            file: Some(super::FileChangeWork {
                run_id,
                project_free,
            }),
        },
    )
    .await;
    let settled = state.conversations.get(&record.id).unwrap();
    assert!(settled.active_job.is_none());
    assert_eq!(
        settled.messages.last().unwrap().status,
        crate::conversations::MessageStatus::Failed
    );
    let run = state.workflow_runs.get(&run_id).unwrap();
    assert_eq!(run.conversation_id, Some(foreign.id));
    assert!(run.attempts.is_empty());
}

#[tokio::test]
async fn cancelled_file_change_retains_written_files_and_final_snapshot() {
    use crate::providers::{ChatBackend, ChatTurn, CompletionReason, ModelEvent};
    let mut state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let connection =
        crate::providers::ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    state.vault.put(connection.clone()).unwrap();
    let token = crate::sessions::generate_session_token().unwrap();
    let session = token.id();
    state.sessions.insert(session);
    let directory = tempfile::tempdir().unwrap();
    let note = directory.path().join("note.txt");
    std::fs::write(&note, "before\n").unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(directory.path(), &[]).unwrap();
    let settings = host_file_change_settings(&state, grant);
    let (record, job) =
        start_file_change_conversation(&state, session, settings.clone(), "Change the file");
    let (run_id, project_free) = file_change_run(&state, record.id, &settings);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(
        crate::tests::ScriptedBackend::rounds(vec![vec![
            Ok(ModelEvent::ToolCall {
                id: "write-then-wait".to_owned(),
                name: "run".to_owned(),
                arguments: serde_json::json!({
                    "command": "printf 'after\\n' > note.txt; sleep 30",
                    "explanation": "Write the note, then wait",
                }),
            }),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::ToolCalls,
            }),
        ]]),
    ));
    let execution = state.workflow_execution.acquire().unwrap();
    let running = tokio::spawn(super::run(
        state.clone(),
        super::OrdinaryRun {
            session,
            record: record.clone(),
            connection,
            job: job.clone(),
            execution,
            kind: OrdinaryKind::FileChange,
            turns: vec![ChatTurn::user("Change the file".to_owned())],
            file: Some(super::FileChangeWork {
                run_id,
                project_free,
            }),
        },
    ));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while std::fs::read_to_string(&note).unwrap() != "after\n" {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    job.request_cancel();
    tokio::time::timeout(std::time::Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(std::fs::read_to_string(&note).unwrap(), "after\n");
    let run = state.workflow_runs.get(&run_id).unwrap();
    let diff = run.attempts[0]
        .direct_changes
        .as_ref()
        .unwrap()
        .diff(&state)
        .unwrap();
    assert_eq!(
        diff.object(0, "base", &state.workflow_artefacts).unwrap().1,
        b"before\n"
    );
    assert_eq!(
        diff.object(0, "target", &state.workflow_artefacts)
            .unwrap()
            .1,
        b"after\n"
    );
    let run = state.workflow_runs.get(&run_id).unwrap();
    assert_eq!(
        run.state.as_label(),
        crate::workflows::run::RunState::Cancelled.as_label()
    );
    let settled = state.conversations.get(&record.id).unwrap();
    assert!(settled.active_job.is_none());
}
