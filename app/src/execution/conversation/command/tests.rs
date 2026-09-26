//! Authority and no-replay cases for the direct command action.

use std::path::Path;

use crate::{
    agents::ToolId,
    conversations::{ConversationId, ConversationModelConfiguration, MessageRole, MessageStatus},
    execution::{
        DirectoryGrant, ExecutionSettings, HostApprovalPolicy, ToolLocation,
        conversation::command::DirectCommandRun,
    },
    providers::{ModelSelection, ProviderKind},
};

fn settings(tools: Vec<ToolId>, directory: Option<&Path>) -> ExecutionSettings {
    let mut settings = ExecutionSettings::new(
        ModelSelection::new(ProviderKind::Deepseek, "deepseek-chat".to_owned(), None)
            .expect("selection"),
        String::new(),
        tools,
        crate::tests::test_environment_id(),
    )
    .expect("settings")
    .with_location(ToolLocation::Host)
    .with_host_approval(HostApprovalPolicy::Automatic);
    if let Some(directory) = directory {
        let mut grant = DirectoryGrant::from_selected(directory, &[]).expect("grant");
        grant.access = crate::execution::DirectoryAccess::Write;
        settings = settings.with_directories(vec![grant]).expect("directories");
    }
    settings
}

fn model(settings: ExecutionSettings) -> ConversationModelConfiguration {
    ConversationModelConfiguration {
        settings,
        preset: None,
    }
}

fn state_and_session() -> (crate::state::AppState, crate::sessions::SessionId) {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let token = crate::sessions::generate_session_token().expect("session token");
    let session = token.id();
    state.sessions.insert(session);
    (state, session)
}

fn approve_host(
    state: &crate::state::AppState,
    session: crate::sessions::SessionId,
    conversation: ConversationId,
    settings: &ExecutionSettings,
) {
    let request = state
        .access_consent
        .request_host_conversation(session, conversation, settings)
        .expect("host request");
    state
        .access_consent
        .approve_host_conversation(&request, session, conversation, settings)
        .expect("host approval");
}

#[allow(clippy::too_many_arguments)]
fn run_for(
    state: &crate::state::AppState,
    session: crate::sessions::SessionId,
    record: crate::conversations::ConversationRecord,
    job: std::sync::Arc<crate::sessions::Job>,
    message: crate::conversations::MessageId,
    command: String,
    included: bool,
    directory: std::path::PathBuf,
) -> DirectCommandRun {
    let _ = state;
    DirectCommandRun {
        session,
        record,
        job,
        message,
        command,
        included,
        directory,
        secret: None,
        execution: state.workflow_execution.acquire().expect("execution guard"),
    }
}

#[tokio::test]
async fn unresolved_preparation_cleanup_blocks_execution_and_reset_after_settlement() {
    for remains in [false, true] {
        let (state, session) = state_and_session();
        let record = state
            .conversations
            .create_saved(
                ConversationId::generate().unwrap(),
                None,
                Some(model(settings(vec![ToolId::Run], None))),
                vec![],
            )
            .unwrap();
        let job = state
            .sessions
            .begin_conversation_job(&session, record.id)
            .unwrap();
        let started = state
            .conversations
            .begin_command(
                &record.id,
                record.revision,
                None,
                job.id(),
                "pwd".into(),
                true,
                "/".into(),
            )
            .unwrap();
        let message = started.messages.last().unwrap().id;
        let work = run_for(
            &state,
            session,
            started,
            job,
            message,
            "pwd".into(),
            true,
            "/".into(),
        );
        state
            .conversation_runtime
            .remember(
                record.id,
                work.job.id(),
                crate::workflows::RunId::generate().unwrap(),
                crate::workflows::AttemptId::generate().unwrap(),
            )
            .unwrap();
        state
            .conversation_runtime
            .finish(record.id, remains, remains)
            .unwrap();
        super::settle(
            &state,
            &work,
            None,
            MessageStatus::Failed,
            Some("Preparation failed".into()),
        );
        drop(work);
        assert_eq!(state.workflow_execution.acquire().is_err(), remains);
        assert_eq!(
            state.workflow_execution.acquire_exclusive().is_err(),
            remains
        );
    }
}

#[tokio::test]
async fn sandbox_selection_is_rejected_before_dispatch() {
    let (state, session) = state_and_session();
    let settings = ExecutionSettings::new(
        ModelSelection::new(ProviderKind::Deepseek, "deepseek-chat".to_owned(), None)
            .expect("selection"),
        String::new(),
        vec![ToolId::Run],
        crate::tests::test_environment_id(),
    )
    .expect("settings");
    let record = state
        .conversations
        .create_saved(
            ConversationId::generate().expect("id"),
            Some("Command".to_owned()),
            Some(model(settings)),
            Vec::new(),
        )
        .expect("create");
    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .expect("job");
    let run = run_for(
        &state,
        session,
        record,
        job,
        crate::conversations::MessageId::generate().expect("message"),
        "echo hi".to_owned(),
        true,
        std::path::PathBuf::from("/"),
    );
    let settings = run.record.model.as_ref().expect("model").settings.clone();
    assert!(super::validate(&state, &run, &settings).is_err());
}

#[tokio::test]
async fn missing_run_capability_is_rejected() {
    let (state, session) = state_and_session();
    let settings = settings(Vec::new(), None);
    let record = state
        .conversations
        .create_saved(
            ConversationId::generate().expect("id"),
            Some("Command".to_owned()),
            Some(model(settings)),
            Vec::new(),
        )
        .expect("create");
    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .expect("job");
    let run = run_for(
        &state,
        session,
        record,
        job,
        crate::conversations::MessageId::generate().expect("message"),
        "echo hi".to_owned(),
        true,
        std::path::PathBuf::from("/"),
    );
    let settings = run.record.model.as_ref().expect("model").settings.clone();
    assert!(super::validate(&state, &run, &settings).is_err());
}

#[tokio::test]
async fn path_approval_allows_a_command_after_directory_replacement() {
    let (state, session) = state_and_session();
    let directory = tempfile::tempdir().expect("directory");
    let settings = settings(vec![ToolId::Run], Some(directory.path()));
    let configuration = model(settings.clone());
    let record = state
        .conversations
        .create_saved(
            ConversationId::generate().expect("id"),
            Some("Command".to_owned()),
            Some(configuration),
            Vec::new(),
        )
        .expect("create");
    approve_host(&state, session, record.id, &settings);
    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .expect("job");
    let previous = tempfile::tempdir().expect("previous directory");
    std::fs::rename(directory.path(), previous.path().join("original")).unwrap();
    std::fs::create_dir(directory.path()).unwrap();
    let command = "printf written > marker.txt".to_owned();
    let started = state
        .conversations
        .begin_command(
            &record.id,
            record.revision,
            None,
            job.id(),
            command.clone(),
            true,
            directory.path().to_string_lossy().into_owned(),
        )
        .expect("begin command");
    let message = started
        .messages
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::Command)
        .map(|message| message.id)
        .expect("command message");
    super::run(
        state.clone(),
        run_for(
            &state,
            session,
            started,
            job.clone(),
            message,
            command,
            true,
            directory.path().to_path_buf(),
        ),
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(directory.path().join("marker.txt")).expect("marker"),
        "written"
    );
    let settled = state.conversations.get(&record.id).expect("conversation");
    assert!(settled.active_job.is_none());
    let entry = settled
        .messages
        .iter()
        .find(|message| message.role == MessageRole::Command)
        .expect("command entry");
    assert_eq!(entry.status, MessageStatus::Complete);
    let command = entry.command.as_ref().expect("command metadata");
    assert!(command.output.as_ref().unwrap().is_success());
}

#[tokio::test]
async fn large_direct_output_settles_with_a_preview_and_scoped_retention() {
    for included in [true, false] {
        let (state, session) = state_and_session();
        let directory = tempfile::tempdir().unwrap();
        let settings = settings(vec![ToolId::Run], Some(directory.path()));
        let record = state
            .conversations
            .create_saved(
                ConversationId::generate().unwrap(),
                None,
                Some(model(settings.clone())),
                vec![],
            )
            .unwrap();
        approve_host(&state, session, record.id, &settings);
        let job = state
            .sessions
            .begin_conversation_job(&session, record.id)
            .unwrap();
        let command = "seq 1 20000".to_owned();
        let started = state
            .conversations
            .begin_command(
                &record.id,
                record.revision,
                None,
                job.id(),
                command.clone(),
                included,
                directory.path().to_string_lossy().into_owned(),
            )
            .unwrap();
        let message = started.messages.last().unwrap().id;
        super::run(
            state.clone(),
            run_for(
                &state,
                session,
                started,
                job,
                message,
                command,
                included,
                directory.path().to_path_buf(),
            ),
        )
        .await;
        let settled = state.conversations.get(&record.id).unwrap();
        assert!(settled.active_job.is_none());
        let entry = settled.messages.last().unwrap();
        assert_eq!(entry.status, MessageStatus::Complete);
        let output = entry.command.as_ref().unwrap().output.as_ref().unwrap();
        assert!(output.is_success());
        assert_eq!(
            output.combined().len(),
            crate::execution::OUTPUT_PREVIEW_BYTES
        );
        let retained = output.retained.as_ref().unwrap();
        let scope = crate::execution::OutputScope::conversation(record.id);
        let (full, truncated) = state.outputs.full(&retained.reference, &scope).unwrap();
        assert!(!truncated);
        let expected: String = (1..=20000).map(|number| format!("{number}\n")).collect();
        assert_eq!(
            full.iter()
                .map(|chunk| chunk.text.as_str())
                .collect::<String>(),
            expected
        );
        assert_eq!(retained.bytes, expected.len());
        let turns = crate::conversations::history::project(&settled.messages, None).unwrap();
        if included {
            assert_eq!(turns.len(), 1);
            assert!(turns[0].text.contains(&retained.reference));
            assert!(turns[0].text.contains("read_output"));
            assert!(turns[0].text.contains("Storage truncated: false"));
            assert!(!turns[0].text.contains("\n19999\n20000\n"));
        } else {
            assert!(turns.is_empty());
        }
        let page = state.outputs.model_page(
            &retained.reference,
            &scope,
            crate::tools::read::parse_request(None, None).unwrap(),
        );
        assert_eq!(page.is_ok(), included);
    }
}

#[tokio::test]
async fn cancellation_settles_the_transcript_and_releases_ownership_without_reversal() {
    for dispatched in [false, true] {
        let (state, session) = state_and_session();
        let directory = tempfile::tempdir().unwrap();
        let settings =
            settings(vec![ToolId::Run], Some(directory.path())).with_host_approval(if dispatched {
                HostApprovalPolicy::Automatic
            } else {
                HostApprovalPolicy::AskEachTime
            });
        let record = state
            .conversations
            .create_saved(
                ConversationId::generate().unwrap(),
                None,
                Some(model(settings.clone())),
                Vec::new(),
            )
            .unwrap();
        approve_host(&state, session, record.id, &settings);
        let job = state
            .sessions
            .begin_conversation_job(&session, record.id)
            .unwrap();
        let command = "printf written > marker.txt; sleep 30".to_owned();
        let started = state
            .conversations
            .begin_command(
                &record.id,
                record.revision,
                None,
                job.id(),
                command.clone(),
                true,
                directory.path().to_string_lossy().into_owned(),
            )
            .unwrap();
        let message = started.messages.last().unwrap().id;
        let work = run_for(
            &state,
            session,
            started,
            job.clone(),
            message,
            command,
            true,
            directory.path().to_path_buf(),
        );
        let task = tokio::spawn(super::run(state.clone(), work));
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let ready = if dispatched {
                    directory.path().join("marker.txt").exists()
                } else {
                    state
                        .host_approvals
                        .pending_for(record.id, job.id())
                        .is_some()
                };
                if ready {
                    break;
                }
                assert!(!task.is_finished());
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        job.request_cancel();
        state.host_approvals.invalidate_job(job.id());
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        let settled = state.conversations.get(&record.id).unwrap();
        assert!(settled.active_job.is_none());
        let entry = settled.messages.last().unwrap();
        assert_eq!(entry.status, MessageStatus::Interrupted);
        assert!(entry.error.is_none());
        let output = &entry.command.as_ref().unwrap().output;
        if dispatched {
            assert_eq!(
                output.as_ref().unwrap().termination,
                crate::execution::CommandTermination::Cancelled
            );
            assert_eq!(
                std::fs::read_to_string(directory.path().join("marker.txt")).unwrap(),
                "written"
            );
        } else {
            assert_eq!(
                output.as_ref().unwrap().termination,
                crate::execution::CommandTermination::NotDispatched
            );
            assert!(!directory.path().join("marker.txt").exists());
        }
        assert!(
            state
                .host_approvals
                .pending_for(record.id, job.id())
                .is_none()
        );
        assert_eq!(job.snapshot().status, crate::sessions::JobStatus::Cancelled);
        assert!(
            state
                .sessions
                .begin_conversation_job(&session, record.id)
                .is_ok()
        );
    }
}

#[tokio::test]
async fn revoked_consent_blocks_dispatch_after_command_approval() {
    let (state, session) = state_and_session();
    let directory = tempfile::tempdir().expect("directory");
    let settings = settings(vec![ToolId::Run], Some(directory.path()))
        .with_host_approval(HostApprovalPolicy::AskEachTime);
    let record = state
        .conversations
        .create_saved(
            ConversationId::generate().expect("id"),
            None,
            Some(model(settings.clone())),
            Vec::new(),
        )
        .expect("conversation");
    approve_host(&state, session, record.id, &settings);
    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .expect("job");
    let command = "printf unsafe > marker.txt".to_owned();
    let started = state
        .conversations
        .begin_command(
            &record.id,
            record.revision,
            None,
            job.id(),
            command.clone(),
            true,
            directory.path().to_string_lossy().into_owned(),
        )
        .expect("command");
    let message = started.messages.last().expect("entry").id;
    let work = run_for(
        &state,
        session,
        started,
        job.clone(),
        message,
        command,
        true,
        directory.path().to_path_buf(),
    );
    let task = tokio::spawn(super::run(state.clone(), work));
    let request = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if let Some(request) = state.host_approvals.pending_for(record.id, job.id()) {
                break request;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("approval request");
    state.access_consent.invalidate_conversation(record.id);
    state
        .host_approvals
        .decide(&request, crate::execution::HostCommandDecision::Approved)
        .expect("approve command");
    task.await.expect("settlement");
    assert!(!directory.path().join("marker.txt").exists());
    let settled = state.conversations.get(&record.id).expect("conversation");
    assert_eq!(
        settled.messages.last().expect("entry").status,
        MessageStatus::Failed
    );
}

#[tokio::test]
async fn restart_marks_a_pending_command_interrupted_without_replay() {
    let (state, session) = state_and_session();
    let settings = settings(vec![ToolId::Run], None);
    let record = state
        .conversations
        .create_saved(
            ConversationId::generate().expect("id"),
            Some("Command".to_owned()),
            Some(model(settings)),
            Vec::new(),
        )
        .expect("create");
    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .expect("job");
    state
        .conversations
        .begin_command(
            &record.id,
            record.revision,
            None,
            job.id(),
            "echo sentinel".to_owned(),
            false,
            "/".to_owned(),
        )
        .expect("begin command");
    state
        .conversations
        .interrupt_requests()
        .expect("interrupt recovered requests");
    let recovered = state.conversations.get(&record.id).expect("conversation");
    assert!(recovered.active_job.is_none());
    let entry = recovered
        .messages
        .iter()
        .find(|message| message.role == MessageRole::Command)
        .expect("command entry");
    assert_eq!(entry.status, MessageStatus::Interrupted);
    // The included flag survives so a `!!` sentinel never enters model context.
    assert!(
        entry
            .command
            .as_ref()
            .is_some_and(|command| !command.included && command.output.is_none())
    );
}

#[tokio::test]
async fn sandbox_command_uses_live_mounts_without_a_workflow_run() {
    use crate::environments::{EnvironmentDraft, SnapshotAvailability};

    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let (environment, preparation) = state
        .environments
        .create(EnvironmentDraft {
            name: "Alpine Git".to_owned(),
            oci_image: "alpine/git".to_owned(),
            setup_script: String::new(),
        })
        .expect("environment");
    state.environments.claim_oldest_queued().expect("claim");
    let snapshot = crate::tests::sample_snapshot(preparation.id);
    state.environment_snapshots.mark(
        snapshot.artifact_key.clone(),
        SnapshotAvailability::Available,
    );
    state
        .environments
        .finish_ready(&preparation.id, snapshot, preparation.log)
        .expect("ready");
    let token = crate::sessions::generate_session_token().expect("session token");
    let session = token.id();
    state.sessions.insert(session);
    let directory = tempfile::tempdir().expect("directory");
    std::fs::write(directory.path().join("note.txt"), "before\n").expect("note");
    let mut grant = DirectoryGrant::from_selected(directory.path(), &[]).expect("grant");
    grant.access = crate::execution::DirectoryAccess::Read;
    let settings = ExecutionSettings::new(
        ModelSelection::new(ProviderKind::Deepseek, "deepseek-chat".to_owned(), None)
            .expect("selection"),
        String::new(),
        vec![ToolId::Run],
        environment.id,
    )
    .expect("settings")
    .with_location(ToolLocation::Sandbox)
    .with_host_approval(HostApprovalPolicy::Automatic)
    .with_directories(vec![grant])
    .expect("directories");
    let record = state
        .conversations
        .create_saved(
            ConversationId::generate().expect("id"),
            Some("Command".to_owned()),
            Some(model(settings.clone())),
            Vec::new(),
        )
        .expect("create");
    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .expect("job");
    let command = "printf after > note.txt".to_owned();
    let started = state
        .conversations
        .begin_command(
            &record.id,
            record.revision,
            Some(model(settings)),
            job.id(),
            command.clone(),
            true,
            "/workspace".to_owned(),
        )
        .expect("begin command");
    let message = started.messages.last().expect("entry").id;
    super::run(
        state.clone(),
        run_for(
            &state,
            session,
            started,
            job,
            message,
            command,
            true,
            std::path::PathBuf::from("/workspace"),
        ),
    )
    .await;
    assert!(state.workflow_runs.all_summaries().is_empty());
    assert_eq!(
        std::fs::read_to_string(directory.path().join("note.txt")).expect("note"),
        "before\n"
    );
    let settled = state.conversations.get(&record.id).expect("conversation");
    let entry = settled.messages.last().expect("entry");
    assert_eq!(entry.role, MessageRole::Command);
    assert_eq!(entry.status, MessageStatus::Complete);
    let output = entry
        .command
        .as_ref()
        .and_then(|command| command.output.as_ref())
        .expect("command output");
    assert!(output.combined().contains("printf after > note.txt"));
    assert!(settled.active_job.is_none());
    assert!(!state.conversation_runtime.unsettled(record.id));
}

#[tokio::test]
async fn sandbox_command_rejects_a_revoked_run_capability() {
    let (state, session) = state_and_session();
    let settings = ExecutionSettings::new(
        ModelSelection::new(ProviderKind::Deepseek, "deepseek-chat".to_owned(), None)
            .expect("selection"),
        String::new(),
        Vec::new(),
        crate::tests::test_environment_id(),
    )
    .expect("settings")
    .with_location(ToolLocation::Sandbox);
    let record = state
        .conversations
        .create_saved(
            ConversationId::generate().expect("id"),
            Some("Command".to_owned()),
            Some(model(settings)),
            Vec::new(),
        )
        .expect("create");
    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .expect("job");
    let run = run_for(
        &state,
        session,
        record,
        job,
        crate::conversations::MessageId::generate().expect("message"),
        "echo hi".to_owned(),
        true,
        std::path::PathBuf::from("/workspace"),
    );
    let settings = run.record.model.as_ref().expect("model").settings.clone();
    assert_eq!(
        super::validate(&state, &run, &settings),
        Err("The Run tool is not enabled for this conversation.".to_owned())
    );
}
