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
        settings = settings
            .with_directories(vec![
                DirectoryGrant::from_selected(directory, &[]).expect("grant"),
            ])
            .expect("directories");
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
async fn named_directory_write_settles_with_baseline_and_output_evidence() {
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
    assert!(command.before.is_some(), "baseline evidence is recorded");
    assert!(command.after.is_some(), "final evidence is recorded");
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
async fn sandbox_command_uses_guest_capture_without_a_host_write() {
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
    grant.access = crate::execution::DirectoryAccess::ReviewBeforeApply;
    let settings = ExecutionSettings::new(
        ModelSelection::new(ProviderKind::Deepseek, "deepseek-chat".to_owned(), None)
            .expect("selection"),
        String::new(),
        vec![ToolId::Run],
        environment.id,
    )
    .expect("settings")
    .with_location(ToolLocation::Sandbox)
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
    let run = state
        .workflow_runs
        .all_summaries()
        .into_iter()
        .find_map(|summary| state.workflow_runs.get(&summary.id))
        .expect("run");
    // The scripted guest makes no change, so the run completes without a gate.
    assert_eq!(run.attempts.len(), 1);
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
        super::validate_sandbox(&state, &run, &settings),
        Err("The Run capability is not enabled for this conversation.".to_owned())
    );
}
