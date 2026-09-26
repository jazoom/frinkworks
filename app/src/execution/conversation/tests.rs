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

fn settings() -> ExecutionSettings {
    crate::workflows::tests::settings()
}

#[test]
fn failed_preparation_retains_reservations_until_cleanup_settles() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let session = crate::sessions::generate_session_token().unwrap().id();
    state.sessions.insert(session);
    let record = state.conversations.create("Cleanup".into()).unwrap();
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
            "Read the project".into(),
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
            execution: state.workflow_execution.acquire().unwrap(),
            kind: OrdinaryKind::Sandbox,
            turns: Vec::new(),
        },
        crate::providers::AssistantReply::default(),
        super::AgentOutcome::AuthorityFailure,
        Some("The environment is unavailable.".into()),
        None,
        false,
        None,
    );
    assert!(state.sessions.conversation_reserved(record.id));
    assert!(state.workflow_execution.acquire_exclusive().is_err());
    assert!(state.conversation_runtime.unsettled(record.id));
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
    runtime
        .recover(
            &guests,
            &[crate::workflows::workspace::WorkspaceRecovery {
                run,
                attempt,
                remains: true,
            }],
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

#[test]
fn write_uses_the_same_ordinary_sandbox_path_as_read() {
    let root = tempfile::tempdir().unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(root.path(), &[]).unwrap();
    for access in [DirectoryAccess::Read, DirectoryAccess::Write] {
        let mut settings = settings();
        settings.tools = vec![ToolId::Run];
        settings.directories = vec![crate::execution::DirectoryGrant {
            access,
            ..grant.clone()
        }];
        assert_eq!(ordinary_kind(&settings), Some(OrdinaryKind::Sandbox));
        let authority = ProjectFreeAuthority::from_settings(1, &settings).unwrap();
        let spec = sandbox_spec(root.path(), &authority, NetworkAccess::None).unwrap();
        assert_eq!(spec.mounts.len(), 1);
        assert_eq!(spec.mounts[0].host, grant.host_path);
        assert_eq!(spec.mounts[0].read_only, access == DirectoryAccess::Read);
        settings.location = ToolLocation::Host;
        assert_eq!(
            ProjectFreeAuthority::from_settings(1, &settings).is_ok(),
            access == DirectoryAccess::Write
        );
    }
}

#[test]
fn private_workspace_has_no_host_backed_writable_alias() {
    let root = tempfile::tempdir().unwrap();
    let authority = ProjectFreeAuthority::from_settings(1, &settings()).unwrap();
    let spec = sandbox_spec(root.path(), &authority, NetworkAccess::None).unwrap();
    assert!(spec.mounts.is_empty());
    assert_eq!(spec.workdir, crate::execution::GUEST_WORKSPACE);
}

#[cfg(unix)]
#[test]
fn redirected_directory_cannot_mint_authority() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("project");
    std::fs::create_dir(&path).unwrap();
    let mut settings = settings();
    settings.directories =
        vec![crate::execution::DirectoryGrant::from_selected(&path, &[]).unwrap()];
    let original = root.path().join("old");
    std::fs::rename(&path, &original).unwrap();
    std::os::unix::fs::symlink(&original, &path).unwrap();
    assert!(ProjectFreeAuthority::from_settings(1, &settings).is_err());
}

#[tokio::test]
async fn failed_guest_disposal_retains_the_handle_and_reports_an_orphan() {
    for stop in [false, true] {
        let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
        let attempt = crate::workflows::AttemptId::generate().unwrap();
        let guest = state
            .sandboxes
            .attempt_handle(crate::workflows::RunId::generate().unwrap(), attempt);
        if stop {
            guest.fail_next_stop();
        } else {
            guest.fail_next_remove();
        }
        assert!(!super::dispose_guest(&state, &guest, attempt).await);
        assert!(state.sandboxes.guest_named(attempt));
        assert_eq!(state.sandboxes.orphans().len(), 1);
        assert!(super::dispose_guest(&state, &guest, attempt).await);
        assert!(!state.sandboxes.guest_named(attempt));
    }
}
