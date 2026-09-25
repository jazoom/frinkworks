use serde_json::json;

use crate::execution::command::{CommandChunk, CommandResult, CommandStream, CommandTermination};
use crate::execution::{OutputKey, OutputScope, OutputStore};
use crate::providers::{AssistantActivity, ToolOutput};
use crate::sessions::{JobId, SessionStore};

use super::{ForkDrafts, ForkError, materialise, snapshot};
use crate::conversations::{
    ConversationMessage, ConversationRecord, MessageId, MessageRole, MessageStatus,
};

fn message(
    role: MessageRole,
    text: &str,
    activity: Vec<AssistantActivity>,
    request: Option<JobId>,
) -> ConversationMessage {
    let id = MessageId::generate().expect("message id");
    ConversationMessage {
        parent: None,
        id,
        role,
        response: (role == MessageRole::Assistant).then_some(id),
        final_phase: role == MessageRole::Assistant,
        text: text.to_owned(),
        input: None,
        command: None,
        attachments: Vec::new(),
        activity,
        continuation: Vec::new(),
        status: MessageStatus::Complete,
        error: None,
        request,
        completion: None,
        requests: Vec::new(),
    }
}

fn assistant(text: &str, activity: Vec<AssistantActivity>) -> ConversationMessage {
    message(
        MessageRole::Assistant,
        text,
        activity,
        Some(JobId::generate().expect("job")),
    )
}

fn user(text: &str) -> ConversationMessage {
    message(MessageRole::User, text, Vec::new(), None)
}

fn record(messages: Vec<ConversationMessage>) -> ConversationRecord {
    ConversationRecord {
        id: crate::conversations::ConversationId::generate().expect("conversation"),
        revision: 1,
        title: "Source".to_owned(),
        title_pending: false,
        network: crate::agents::NetworkAccess::None,
        model: None,
        directory_approvals: Vec::new(),
        forked_from: None,
        messages,
        active_job: None,
        continuation: None,
        compaction: None,
        summary_requests: Vec::new(),
        queue: crate::conversations::ConversationQueue::default(),
        created_at_ms: 0,
        updated_at_ms: 0,
    }
}

#[test]
fn accepts_a_complete_exchange_boundary() {
    let reply = assistant("Answer", Vec::new());
    let record = record(vec![user("Question"), reply.clone()]);
    let snapshot = snapshot(&record, reply.id).expect("snapshot");
    assert_eq!(snapshot.source, record.id);
    assert_eq!(snapshot.boundary, reply.id);
    assert_eq!(snapshot.messages.len(), 2);
    assert_eq!(snapshot.source_revision, record.revision);
}

#[test]
fn rejects_a_boundary_that_splits_an_exchange() {
    let reply = assistant("Answer", Vec::new());
    let second = user("Second question");
    let record = record(vec![user("First question"), reply, second.clone()]);
    assert_eq!(
        snapshot(&record, second.id).err(),
        Some(ForkError::Boundary)
    );
}

#[test]
fn rejects_an_unsettled_tool_call() {
    let reply = assistant(
        "",
        vec![AssistantActivity::ToolCall {
            id: "call-1".to_owned(),
            name: "run".to_owned(),
            arguments: json!({ "command": "ls" }),
            result: None,
        }],
    );
    let record = record(vec![user("Run it"), reply.clone()]);
    assert_eq!(
        snapshot(&record, reply.id).err(),
        Some(ForkError::Uncertain)
    );
}

#[test]
fn rejects_an_unknown_command_outcome() {
    let command = CommandResult::new(Vec::new(), CommandTermination::Unknown);
    let reply = assistant(
        "",
        vec![AssistantActivity::ToolCall {
            id: "call-1".to_owned(),
            name: "run".to_owned(),
            arguments: json!({ "command": "ls" }),
            result: Some(ToolOutput {
                resource: None,
                label: "run".to_owned(),
                output: String::new(),
                command: Some(command),
            }),
        }],
    );
    let record = record(vec![user("Run it"), reply.clone()]);
    assert_eq!(
        snapshot(&record, reply.id).err(),
        Some(ForkError::Uncertain)
    );
}

#[test]
fn a_fork_preserves_settings_without_copying_consent() {
    let mut record = record(vec![user("Review"), assistant("Reviewed", Vec::new())]);
    let grant = crate::execution::DirectoryGrant {
        id: crate::execution::DirectoryGrantId::generate().expect("grant"),
        host_path: std::path::PathBuf::from("/tmp/example"),
        identity: crate::execution::CanonicalDirectoryIdentity {
            device: 1,
            inode: 2,
        },
        alias: "example".to_owned(),
        access: crate::execution::DirectoryAccess::Write,
    };
    record.model = Some(crate::conversations::ConversationModelConfiguration {
        settings: crate::execution::ExecutionSettings::new(
            crate::providers::ModelSelection::new(
                crate::providers::ProviderKind::Xai,
                "grok-4.6".to_owned(),
                None,
            )
            .expect("selection"),
            String::new(),
            vec![crate::agents::ToolId::Run],
            crate::tests::test_environment_id(),
        )
        .expect("settings")
        .with_directories(vec![grant])
        .expect("directories")
        .with_location(crate::execution::ToolLocation::Host),
        preset: None,
    });
    let boundary = record.messages.last().expect("reply").id;
    let snapshot = snapshot(&record, boundary).expect("snapshot");
    assert_eq!(snapshot.model, record.model);
}

#[test]
fn draft_tokens_bind_to_one_session() {
    let drafts = ForkDrafts::default();
    let sessions = SessionStore::new();
    let owner = crate::sessions::generate_session_token().expect("token");
    let other = crate::sessions::generate_session_token().expect("token");
    sessions.insert(owner.id());
    sessions.insert(other.id());
    let record = record(vec![user("Question"), assistant("Answer", Vec::new())]);
    let boundary = record.messages.last().expect("reply").id;
    let snapshot = snapshot(&record, boundary).expect("snapshot");
    drafts
        .insert(owner.id(), "nonce".to_owned(), snapshot, &sessions)
        .expect("insert");
    assert!(drafts.get(owner.id(), "nonce").is_some());
    assert!(drafts.get(other.id(), "nonce").is_none());
    assert!(drafts.claim(other.id(), "nonce").is_none());
    let claim = drafts.claim(owner.id(), "nonce").expect("claim");
    assert!(drafts.claim(owner.id(), "nonce").is_none());
    drop(claim);
    drafts.claim(owner.id(), "nonce").expect("retry").commit();
    assert!(drafts.claim(owner.id(), "nonce").is_none());
}

#[test]
fn materialise_rebinds_retained_output_to_the_destination() {
    let outputs = OutputStore::ephemeral();
    let source = crate::conversations::ConversationId::generate().expect("source");
    let job = JobId::generate().expect("job");
    let command = CommandResult::new(
        vec![CommandChunk {
            stream: CommandStream::Stdout,
            text: "hello".to_owned(),
        }],
        CommandTermination::Exited(0),
    );
    let retained = outputs
        .store(
            &OutputKey {
                scope: OutputScope::conversation(source),
                job,
                tool_call: "call-1".to_owned(),
                model_hidden: false,
            },
            &command,
        )
        .expect("store");
    let reply = message(
        MessageRole::Assistant,
        "",
        vec![AssistantActivity::ToolCall {
            id: "call-1".to_owned(),
            name: "run".to_owned(),
            arguments: json!({ "command": "echo hello" }),
            result: Some(ToolOutput {
                resource: None,
                label: "run".to_owned(),
                output: "hello".to_owned(),
                command: Some(command.retain(retained.clone())),
            }),
        }],
        Some(job),
    );
    let mut record = record(vec![user("Run it"), reply.clone()]);
    record.id = source;
    let snapshot = snapshot(&record, reply.id).expect("snapshot");
    let destination = crate::conversations::ConversationId::generate().expect("destination");
    let messages = materialise(&snapshot, destination, &outputs).expect("materialise");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].id, reply.id);
    let copied = match &messages[1].activity[0] {
        AssistantActivity::ToolCall {
            result: Some(tool), ..
        } => tool.command.as_ref().expect("command"),
        other => panic!("unexpected activity: {other:?}"),
    };
    let rebound = copied.retained.as_ref().expect("retained");
    assert_ne!(rebound.reference, retained.reference);
    let request = crate::tools::read::parse_request(None, None).expect("request");
    assert!(
        outputs
            .page(
                &rebound.reference,
                &OutputScope::conversation(destination),
                request
            )
            .is_ok()
    );
    assert!(
        outputs
            .page(
                &retained.reference,
                &OutputScope::conversation(source),
                request
            )
            .is_ok()
    );
    assert!(
        outputs
            .page(
                &rebound.reference,
                &OutputScope::conversation(source),
                request
            )
            .is_err()
    );
}

#[test]
fn a_fork_with_excluded_commands_remains_durable() {
    let mut command = message(
        MessageRole::Command,
        "printf excluded",
        Vec::new(),
        Some(JobId::generate().expect("job")),
    );
    command.response = None;
    command.final_phase = false;
    command.command = Some(crate::conversations::history::CommandEntry {
        included: false,
        directory: "/".to_owned(),
        output: Some(CommandResult::new(
            Vec::new(),
            CommandTermination::Exited(0),
        )),
    });
    let reply = assistant("Settled reply", Vec::new());
    let source = record(vec![command, user("Continue"), reply.clone()]);
    let snapshot = snapshot(&source, reply.id).expect("snapshot");
    let destination = crate::conversations::ConversationId::generate().expect("destination");
    let messages =
        materialise(&snapshot, destination, &OutputStore::ephemeral()).expect("materialise");
    let directory = tempfile::tempdir().expect("store directory");
    let store = crate::conversations::ConversationStore::open(directory.path().to_path_buf())
        .expect("store");
    store
        .create_fork(destination, None, None, Vec::new(), messages, &snapshot)
        .expect("persist fork");
    drop(store);
    let store = crate::conversations::ConversationStore::open(directory.path().to_path_buf())
        .expect("reload");
    let loaded = store.get(&destination).expect("fork");
    assert!(
        !loaded.messages[0]
            .command
            .as_ref()
            .expect("command")
            .included
    );
    assert!(
        crate::conversations::history::project(&loaded.messages, None)
            .expect("projection")
            .iter()
            .all(|turn| !turn.text.contains("excluded"))
    );
}

#[test]
fn an_intermediate_phase_is_not_a_fork_boundary() {
    let mut intermediate = assistant("First phase", Vec::new());
    intermediate.final_phase = false;
    let mut final_phase = assistant("Last phase", Vec::new());
    final_phase.response = Some(intermediate.response.unwrap());
    // The intermediate phase completes an exchange, but its logical response
    // has not settled.
    assert!(!super::forkable(&[user("Question"), intermediate], 1));
    // The final phase of the same response is a safe boundary.
    assert!(super::forkable(&[user("Question"), final_phase], 1));
}
