use std::time::{Duration, Instant};

use super::{
    MAXIMUM_MODEL_REPLY_BYTES, MAXIMUM_THINKING_PROGRESS_BYTES, THINKING_INITIAL_DELAY,
    THINKING_PROGRESS_INTERVAL, ThinkingProgress, append_model_piece, publish_reply_before_tools,
    visible_tool_output,
};
use crate::{
    providers::{AssistantReply, ToolOutput},
    sessions::{Job, JobEventKind, JobId},
    workflows::RunId,
};

#[test]
fn streamed_credentials_remain_redacted_at_every_utf8_split() {
    let secret = "sk-sécret";
    let input = format!("Before {secret} after {secret}.");
    for split in input.char_indices().map(|(index, _)| index) {
        let mut redactor = super::StreamRedactor::new(Some(secret));
        let mut output = redactor.push(&input[..split]);
        assert!(!output.contains(secret));
        output.push_str(&redactor.push(&input[split..]));
        output.push_str(&redactor.finish());
        assert_eq!(output, "Before [redacted] after [redacted].");
    }
    let mut redactor = super::StreamRedactor::new(Some(secret));
    let mut output = String::new();
    for character in input.chars() {
        output.push_str(&redactor.push(&character.to_string()));
    }
    output.push_str(&redactor.finish());
    assert_eq!(output, "Before [redacted] after [redacted].");
    let mut redactor = super::StreamRedactor::new(Some(secret));
    assert_eq!(redactor.push("Before sk-"), "Before ");
    assert_eq!(redactor.finish_boundary(), "[redacted]");
    assert_eq!(redactor.push("Next block"), "Next block");
}

#[test]
fn a_large_tool_result_does_not_consume_the_model_reply_limit() {
    let mut visible_tool_bytes = 0;
    let tool = visible_tool_output(
        "read `/project/large.txt`".to_owned(),
        &"x".repeat(crate::tools::MAXIMUM_TOOL_BYTES),
        None,
        &mut visible_tool_bytes,
    )
    .expect("visible tool output");

    let mut reply = AssistantReply::default();
    let mut model_reply_bytes = 0;
    assert!(!append_model_piece(
        &mut reply,
        &"a".repeat(MAXIMUM_MODEL_REPLY_BYTES),
        &mut model_reply_bytes,
    ));
    assert!(tool.output.ends_with("[output truncated]"));
    assert_eq!(reply.text.len(), MAXIMUM_MODEL_REPLY_BYTES);
}

#[test]
fn command_status_survives_an_exhausted_display_budget() {
    use crate::execution::command::{
        CommandChunk, CommandResult, CommandStream, CommandTermination,
    };
    let mut bytes = super::MAXIMUM_VISIBLE_TOOL_BYTES;
    let tool = visible_tool_output(
        "run".to_owned(),
        "partial output",
        Some(CommandResult::new(
            vec![CommandChunk {
                stream: CommandStream::Stderr,
                text: "partial output".to_owned(),
            }],
            CommandTermination::Cancelled,
        )),
        &mut bytes,
    )
    .expect("retain the outcome even without output space");
    assert_eq!(bytes, super::MAXIMUM_VISIBLE_TOOL_BYTES);
    let mut reply = AssistantReply::default();
    reply.push_tool(tool);
    let reply = super::bound_reply(&reply);
    let command = reply.tools[0].command.as_ref().unwrap();
    assert_eq!(command.termination, CommandTermination::Cancelled);
    assert!(command.chunks.is_empty());
}

fn job() -> std::sync::Arc<Job> {
    Job::new(
        JobId::generate().expect("job id"),
        RunId::generate().expect("run id"),
        1,
    )
}

fn thinking_deltas(job: &Job) -> Vec<String> {
    job.events_after(0)
        .into_iter()
        .filter_map(|event| match event.kind {
            JobEventKind::Thinking { delta } => Some(delta),
            _ => None,
        })
        .collect()
}

#[test]
fn initial_thinking_tokens_are_coalesced_before_publication() {
    let job = job();
    let started = Instant::now();
    let mut progress = ThinkingProgress::default();
    let mut thinking = "First".to_owned();

    progress.note_pending(&thinking, started);
    assert!(!progress.publish_due(&job, &thinking, started));
    thinking.push_str(" thought");
    progress.note_pending(&thinking, started + Duration::from_millis(20));
    assert!(!progress.publish_due(
        &job,
        &thinking,
        started + THINKING_INITIAL_DELAY - Duration::from_millis(1),
    ));
    assert!(progress.publish_due(&job, &thinking, started + THINKING_INITIAL_DELAY));

    assert_eq!(thinking_deltas(&job), vec!["First thought"]);
}

#[test]
fn steady_thinking_updates_follow_the_progress_interval() {
    let job = job();
    let started = Instant::now();
    let mut progress = ThinkingProgress::default();
    let mut thinking = "First thought".to_owned();

    progress.note_pending(&thinking, started);
    let first_emit = started + THINKING_INITIAL_DELAY;
    assert!(progress.publish_due(&job, &thinking, first_emit));
    thinking.push_str(" and the next thought");
    progress.note_pending(&thinking, first_emit + Duration::from_millis(1));
    assert!(!progress.publish_due(
        &job,
        &thinking,
        first_emit + THINKING_PROGRESS_INTERVAL - Duration::from_millis(1),
    ));
    assert!(progress.publish_due(&job, &thinking, first_emit + THINKING_PROGRESS_INTERVAL,));

    assert_eq!(
        thinking_deltas(&job),
        vec!["First thought", " and the next thought"]
    );
}

#[test]
fn thinking_backlog_catches_up_in_bounded_updates() {
    let job = job();
    let started = Instant::now();
    let mut progress = ThinkingProgress::default();
    let thinking = "x".repeat(MAXIMUM_THINKING_PROGRESS_BYTES * 3 + 7);

    progress.note_pending(&thinking, started);
    let first_emit = started + THINKING_INITIAL_DELAY;
    assert!(progress.publish_due(&job, &thinking, first_emit));
    assert!(progress.published < thinking.len());
    assert!(progress.publish_due(&job, &thinking, first_emit + THINKING_PROGRESS_INTERVAL,));

    let deltas = thinking_deltas(&job);
    assert_eq!(deltas.len(), 2);
    assert!(
        deltas
            .iter()
            .all(|delta| delta.len() <= MAXIMUM_THINKING_PROGRESS_BYTES)
    );
    assert_eq!(deltas.concat(), thinking[..progress.published]);
}

#[test]
fn pending_thinking_is_published_before_a_tool() {
    let job = job();
    let started = Instant::now();
    let mut progress = ThinkingProgress::default();
    let reply = AssistantReply {
        thinking: "thought ".repeat(MAXIMUM_THINKING_PROGRESS_BYTES / 2),
        ..AssistantReply::default()
    };
    let mut published_response = 0;

    progress.note_pending(&reply.thinking, started);
    assert!(progress.publish_due(&job, &reply.thinking, started + THINKING_INITIAL_DELAY));
    publish_reply_before_tools(&job, &reply, &mut published_response, &mut progress);
    job.push_tool(ToolOutput {
        label: "read `/project/file`".to_owned(),
        output: "contents".to_owned(),
        command: None,
    });

    let events = job.events_after(0);
    let tool_index = events
        .iter()
        .position(|event| matches!(event.kind, JobEventKind::ToolFinished { .. }))
        .expect("tool event");
    assert_eq!(tool_index, events.len() - 1);
    assert!(events[..tool_index].iter().all(|event| {
        matches!(
            &event.kind,
            JobEventKind::Thinking { delta }
                if delta.len() <= MAXIMUM_THINKING_PROGRESS_BYTES
        )
    }));
    assert_eq!(thinking_deltas(&job).concat(), reply.thinking);
}

#[test]
fn model_reply_overflow_remains_an_error() {
    let mut reply = AssistantReply::default();
    let mut model_reply_bytes = 0;
    assert!(append_model_piece(
        &mut reply,
        &"a".repeat(MAXIMUM_MODEL_REPLY_BYTES + 1),
        &mut model_reply_bytes,
    ));
    assert_eq!(reply.text.len(), MAXIMUM_MODEL_REPLY_BYTES);
}

fn conversation_job(
    state: &crate::state::AppState,
) -> (
    crate::conversations::ConversationRecord,
    std::sync::Arc<Job>,
) {
    use crate::sessions::generate_session_token;
    let token = generate_session_token().expect("session");
    state.sessions.insert(token.id());
    let record = state
        .conversations
        .create("Discussion".to_owned())
        .expect("conversation");
    let job = state
        .sessions
        .begin_conversation_job(&token.id(), record.id)
        .expect("job");
    let record = state
        .conversations
        .begin_message_with_model(
            &record.id,
            record.revision,
            None,
            job.id(),
            "Read the file".to_owned(),
        )
        .expect("message");
    (record, job)
}

fn read_spec(
    connection: crate::providers::ProviderConnection,
    conversation: crate::conversations::ConversationId,
    revision: u32,
) -> super::AgentRunSpec {
    use crate::agents::{DirectoryPolicy, ToolId};
    use crate::execution::ToolLocation;
    super::AgentRunSpec {
        agent_id: None,
        revision,
        preamble: String::new(),
        tools: crate::tools::definitions_for(&[ToolId::Read], ToolLocation::Sandbox),
        tool_ids: vec![ToolId::Read],
        policy: DirectoryPolicy::from_grants(Vec::new(), String::new()),
        connection,
        location: ToolLocation::Sandbox,
        sandbox: None,
        host: None,
        output_drafts: None,
        required_outputs: Vec::new(),
        evidence: None,
        output_scope: None,
        conversation: Some(conversation),
        steering_session: None,
    }
}

#[tokio::test]
async fn truncated_tool_arguments_do_not_dispatch() {
    use crate::config::RuntimeConfig;
    use crate::providers::{
        AssistantActivity, ChatBackend, ChatTurn, CompletionReason, ModelEvent, ProviderConnection,
        ProviderKind,
    };
    use crate::sessions::JobEventKind;
    let mut state = crate::tests::test_state(RuntimeConfig::development());
    let backend = crate::tests::ScriptedBackend::events(vec![
        Ok(ModelEvent::ToolCall {
            id: "call-1".to_owned(),
            name: "read".to_owned(),
            arguments: serde_json::json!({"path": "main.rs"}),
        }),
        Ok(ModelEvent::Complete {
            reason: CompletionReason::Length,
        }),
    ]);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend));
    let connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    state.vault.put(connection.clone()).expect("provider");
    let (record, job) = conversation_job(&state);
    let ended = super::run_agent_action(
        &state,
        read_spec(connection, record.id, record.revision),
        vec![ChatTurn::user("Read the file".to_owned())],
        job.clone(),
    )
    .await;
    assert_eq!(ended.outcome, super::AgentOutcome::ProviderFailure);
    assert_eq!(ended.reply.completion, Some(CompletionReason::Length));
    assert!(ended.reply.tools.is_empty());
    assert!(
        ended.reply.activity.iter().any(|activity| {
            matches!(activity, AssistantActivity::ToolCall { result: None, .. })
        })
    );
    assert!(
        !job.events_after(0)
            .iter()
            .any(|event| { matches!(event.kind, JobEventKind::ToolFinished { .. }) })
    );
}

#[tokio::test]
async fn unsafe_batches_do_not_dispatch_any_call() {
    use crate::config::RuntimeConfig;
    use crate::providers::{
        ChatBackend, ChatTurn, CompletionReason, ModelEvent, ProviderConnection, ProviderKind,
    };
    use crate::sessions::JobEventKind;
    let mut state = crate::tests::test_state(RuntimeConfig::development());
    for (id, name, arguments) in [
        ("call-1", "read", serde_json::json!({"path": "other.rs"})),
        ("call-2", "read", serde_json::json!({"path": 42})),
        ("call-2", "write", serde_json::json!({"path": "other.rs"})),
        ("call-2", "read", serde_json::json!({"path": "test-key"})),
        (
            "call-2",
            "read",
            serde_json::json!({"path": "x".repeat(65537)}),
        ),
    ] {
        let backend = crate::tests::ScriptedBackend::events(vec![
            Ok(ModelEvent::ToolCall {
                id: "call-1".to_owned(),
                name: "read".to_owned(),
                arguments: serde_json::json!({"path": "main.rs"}),
            }),
            Ok(ModelEvent::ToolCall {
                id: id.to_owned(),
                name: name.to_owned(),
                arguments,
            }),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::ToolCalls,
            }),
        ]);
        state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend));
        let connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
        state.vault.put(connection.clone()).expect("provider");
        let (record, job) = conversation_job(&state);
        let mut spec = read_spec(connection, record.id, record.revision);
        spec.conversation = None;
        let ended = super::run_agent_action(
            &state,
            spec,
            vec![ChatTurn::user("Read the file".to_owned())],
            job.clone(),
        )
        .await;
        assert_eq!(ended.outcome, super::AgentOutcome::ProviderFailure);
        assert!(ended.reply.tools.is_empty());
        assert!(
            !job.events_after(0)
                .iter()
                .any(|event| { matches!(event.kind, JobEventKind::ToolFinished { .. }) })
        );
        assert!(!format!("{:?}", ended.reply).contains("test-key"));
        assert!(!format!("{:?}", job.events_after(0)).contains("test-key"));
    }
}

fn granted_read_spec(
    connection: crate::providers::ProviderConnection,
    conversation: crate::conversations::ConversationId,
    revision: u32,
) -> super::AgentRunSpec {
    use crate::agents::{
        AccessMode, AgentId, AgentRecord, DirectoryGrant, DirectoryPolicy, ToolId,
    };
    use crate::execution::ToolLocation;
    let policy = DirectoryPolicy::from_record_with_primary(
        &AgentRecord {
            id: AgentId::generate().expect("id"),
            revision: 1,
            name: "Agent".to_owned(),
            instructions: String::new(),
            selection: None,
            tools: vec![ToolId::Read],
            network: crate::agents::NetworkAccess::None,
            directories: vec![DirectoryGrant {
                alias: "project".to_owned(),
                host_path: "/tmp/project".into(),
                access: AccessMode::ReadWrite,
            }],
            primary_directory: "project".to_owned(),
        },
        "project",
    );
    let mut spec = read_spec(connection, conversation, revision);
    spec.policy = policy;
    spec.location = ToolLocation::Sandbox;
    spec
}

fn tool_finished_count(job: &Job) -> usize {
    job.events_after(0)
        .iter()
        .filter(|event| matches!(event.kind, JobEventKind::ToolFinished { .. }))
        .count()
}

#[tokio::test]
async fn ordinary_read_error_then_a_corrected_call() {
    use crate::config::RuntimeConfig;
    use crate::providers::{
        ChatBackend, ChatTurn, CompletionReason, ModelEvent, ProviderConnection, ProviderKind,
    };
    let mut state = crate::tests::test_state(RuntimeConfig::development());
    let backend = crate::tests::ScriptedBackend::rounds(vec![
        vec![
            Ok(ModelEvent::ToolCall {
                id: "call-1".to_owned(),
                name: "read".to_owned(),
                arguments: serde_json::json!({"path": ""}),
            }),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::ToolCalls,
            }),
        ],
        vec![
            Ok(ModelEvent::ToolCall {
                id: "call-2".to_owned(),
                name: "read".to_owned(),
                arguments: serde_json::json!({"path": "src/main.rs"}),
            }),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::ToolCalls,
            }),
        ],
        vec![
            Ok(ModelEvent::Text("Read the later path.".to_owned())),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::Stop,
            }),
        ],
    ]);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend));
    let connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    state.vault.put(connection.clone()).expect("provider");
    let (record, job) = conversation_job(&state);
    let ended = super::run_agent_action(
        &state,
        granted_read_spec(connection, record.id, record.revision),
        vec![ChatTurn::user("Read the file".to_owned())],
        job.clone(),
    )
    .await;
    assert_eq!(ended.outcome, super::AgentOutcome::Completed);
    assert_eq!(ended.reply.tools.len(), 2);
    assert!(
        ended.reply.tools[0]
            .output
            .contains("Choose a file to read.")
    );
    assert_eq!(tool_finished_count(&job), 2);
}

#[tokio::test]
async fn path_escape_stops_without_dispatching_the_rest() {
    use crate::config::RuntimeConfig;
    use crate::providers::{
        ChatBackend, ChatTurn, CompletionReason, ModelEvent, ProviderConnection, ProviderKind,
    };
    let mut state = crate::tests::test_state(RuntimeConfig::development());
    let backend = crate::tests::ScriptedBackend::events(vec![
        Ok(ModelEvent::ToolCall {
            id: "call-1".to_owned(),
            name: "read".to_owned(),
            arguments: serde_json::json!({"path": ".."}),
        }),
        Ok(ModelEvent::ToolCall {
            id: "call-2".to_owned(),
            name: "read".to_owned(),
            arguments: serde_json::json!({"path": "src/main.rs"}),
        }),
        Ok(ModelEvent::Complete {
            reason: CompletionReason::ToolCalls,
        }),
    ]);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend));
    let connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    state.vault.put(connection.clone()).expect("provider");
    let (record, job) = conversation_job(&state);
    let ended = super::run_agent_action(
        &state,
        granted_read_spec(connection, record.id, record.revision),
        vec![ChatTurn::user("Read the file".to_owned())],
        job.clone(),
    )
    .await;
    assert_eq!(ended.outcome, super::AgentOutcome::AuthorityFailure);
    assert_eq!(ended.reply.tools.len(), 2);
    assert!(
        ended.reply.tools[0]
            .output
            .contains("Stay inside a granted directory.")
    );
    assert_eq!(ended.reply.tools[1].output, "This tool did not run.");
    assert_eq!(
        ended.reply.tools[1]
            .command
            .as_ref()
            .map(|command| command.termination),
        Some(crate::execution::CommandTermination::NotDispatched)
    );
}

#[tokio::test]
async fn not_dispatched_command_returns_as_an_ordinary_result() {
    use crate::config::RuntimeConfig;
    use crate::providers::{
        ChatBackend, ChatTurn, CompletionReason, ModelEvent, ProviderConnection, ProviderKind,
    };
    let mut state = crate::tests::test_state(RuntimeConfig::development());
    let backend = crate::tests::ScriptedBackend::rounds(vec![
        vec![
            Ok(ModelEvent::ToolCall {
                id: "call-1".to_owned(),
                name: "run".to_owned(),
                arguments: serde_json::json!({"command": "true"}),
            }),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::ToolCalls,
            }),
        ],
        vec![
            Ok(ModelEvent::Text("No guest ran.".to_owned())),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::Stop,
            }),
        ],
    ]);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend));
    let connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    state.vault.put(connection.clone()).expect("provider");
    let (record, job) = conversation_job(&state);
    let mut spec = granted_read_spec(connection, record.id, record.revision);
    spec.tool_ids = vec![crate::agents::ToolId::Run];
    spec.tools = crate::tools::definitions_for(
        &[crate::agents::ToolId::Run],
        crate::execution::ToolLocation::Sandbox,
    );
    let ended = super::run_agent_action(
        &state,
        spec,
        vec![ChatTurn::user("Run a command".to_owned())],
        job.clone(),
    )
    .await;
    assert_eq!(ended.outcome, super::AgentOutcome::Completed);
    assert_eq!(ended.reply.tools.len(), 1);
    assert_eq!(
        ended.reply.tools[0]
            .command
            .as_ref()
            .map(|command| command.termination),
        Some(crate::execution::CommandTermination::NotDispatched)
    );
    assert_eq!(ended.reply.text, "No guest ran.");
}

#[tokio::test]
async fn rate_limit_before_and_after_a_tool_runs_the_call_once() {
    use crate::config::RuntimeConfig;
    use crate::providers::{
        ChatBackend, ChatTurn, CompletionReason, ModelEvent, ProviderConnection, ProviderError,
        ProviderKind,
    };
    let mut state = crate::tests::test_state(RuntimeConfig::development());
    let backend = crate::tests::ScriptedBackend::turn_results(vec![
        Err(ProviderError::RateLimited { retry_after: None }),
        Ok(vec![
            Ok(ModelEvent::ToolCall {
                id: "call-1".to_owned(),
                name: "run".to_owned(),
                arguments: serde_json::json!({"command": "printf x >> effects", "explanation": "Record one effect"}),
            }),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::ToolCalls,
            }),
        ]),
        Ok(vec![
            Ok(ModelEvent::Text("Discard this partial reply.".to_owned())),
            Err(ProviderError::RateLimited { retry_after: None }),
        ]),
        Ok(vec![
            Ok(ModelEvent::Text("Done.".to_owned())),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::Stop,
            }),
        ]),
    ]);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend.clone()));
    let connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    state.vault.put(connection.clone()).expect("provider");
    let directory = tempfile::tempdir().expect("directory");
    let records = tempfile::tempdir().expect("records");
    state.conversations = std::sync::Arc::new(
        crate::conversations::ConversationStore::open(records.path().to_owned()).unwrap(),
    );
    let token = crate::sessions::generate_session_token().expect("session");
    let session = token.id();
    state.sessions.insert(session);
    let record = state.conversations.create("Host".to_owned()).unwrap();
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None)
            .unwrap(),
        String::new(),
        vec![crate::agents::ToolId::Run],
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_location(crate::execution::ToolLocation::Host)
    .with_host_approval(crate::execution::HostApprovalPolicy::Automatic);
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
        .begin_message_with_model(
            &record.id,
            record.revision,
            None,
            job.id(),
            "Run once".to_owned(),
        )
        .unwrap();
    let mut spec = read_spec(connection, record.id, record.revision);
    spec.location = crate::execution::ToolLocation::Host;
    spec.tool_ids = vec![crate::agents::ToolId::Run];
    spec.tools = crate::tools::definitions_for(&spec.tool_ids, spec.location);
    spec.host = Some(crate::tools::HostRunSpec {
        session,
        conversation: record.id,
        execution_revision: record.revision,
        directory: directory.path().to_owned(),
        settings,
        run: None,
        step: None,
        attempt: None,
    });
    let ended = super::run_agent_action(
        &state,
        spec,
        vec![ChatTurn::user("Run once".to_owned())],
        job.clone(),
    )
    .await;
    assert_eq!(
        ended.outcome,
        super::AgentOutcome::Completed,
        "{:?}",
        ended.error
    );
    assert_eq!(
        std::fs::read(directory.path().join("effects")).unwrap(),
        b"x"
    );
    state
        .conversations
        .settle_message(
            &record.id,
            job.id(),
            ended.reply.clone(),
            crate::conversations::MessageStatus::Complete,
            None,
        )
        .unwrap();
    let reopened =
        crate::conversations::ConversationStore::open(records.path().to_owned()).unwrap();
    let stored = reopened.get(&record.id).unwrap();
    assert!(stored.messages.iter().any(|message| message.status
        == crate::conversations::MessageStatus::Failed
        && message.text == "Discard this partial reply."));
    let history = crate::conversations::history::project(&stored.messages, None).unwrap();
    assert!(!format!("{history:?}").contains("Discard this partial reply."));
    assert_eq!(history.iter().flat_map(|turn| &turn.calls).count(), 1);
    assert!(backend.last_extra_len() >= 2);
    assert_eq!(backend.turn_count(), 4);
    assert_eq!(ended.reply.text, "Done.");
}

#[tokio::test]
async fn cancellation_interrupts_a_retry_wait() {
    use crate::config::RuntimeConfig;
    use crate::providers::{
        ChatBackend, ChatTurn, ProviderConnection, ProviderError, ProviderKind,
    };
    let mut state = crate::tests::test_state(RuntimeConfig::development());
    let backend =
        crate::tests::ScriptedBackend::turn_results(vec![Err(ProviderError::RateLimited {
            retry_after: hypergraft::RetryAfter::seconds(15),
        })]);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend.clone()));
    let connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    state.vault.put(connection.clone()).expect("provider");
    let (record, job) = conversation_job(&state);
    let cancel = job.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        cancel.request_cancel();
    });
    let ended = super::run_agent_action(
        &state,
        granted_read_spec(connection, record.id, record.revision),
        vec![ChatTurn::user("Hello".to_owned())],
        job.clone(),
    )
    .await;
    assert_eq!(ended.outcome, super::AgentOutcome::Cancelled);
    assert_eq!(backend.turn_count(), 1);
}

#[tokio::test]
async fn steering_arrives_after_the_current_batch_and_continues_the_loop() {
    use crate::config::RuntimeConfig;
    use crate::conversations::QueueDelivery;
    use crate::providers::{
        ChatBackend, ChatTurn, CompletionReason, ModelEvent, ProviderConnection, ProviderKind,
    };
    let mut state = crate::tests::test_state(RuntimeConfig::development());
    let backend = crate::tests::ScriptedBackend::rounds(vec![
        vec![
            Ok(ModelEvent::ToolCall {
                id: "call-1".to_owned(),
                name: "read".to_owned(),
                arguments: serde_json::json!({"path": "missing-a.rs"}),
            }),
            Ok(ModelEvent::ToolCall {
                id: "call-2".to_owned(),
                name: "read".to_owned(),
                arguments: serde_json::json!({"path": "missing-b.rs"}),
            }),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::ToolCalls,
            }),
        ],
        {
            let mut items: Vec<_> = ["Adjusted after both tools"]
                .into_iter()
                .map(|text| Ok(ModelEvent::Text(text.to_owned())))
                .collect();
            items.push(Ok(ModelEvent::Complete {
                reason: CompletionReason::Stop,
            }));
            items
        },
    ]);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend.clone()));
    let connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    state.vault.put(connection.clone()).expect("provider");
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None)
            .unwrap(),
        String::new(),
        vec![crate::agents::ToolId::Read],
        crate::tests::test_environment_id(),
    )
    .unwrap();
    let token = crate::sessions::generate_session_token().expect("session");
    state.sessions.insert(token.id());
    let record = state
        .conversations
        .create("Discussion".to_owned())
        .expect("conversation");
    let record = state
        .conversations
        .update_execution_settings(&record.id, record.revision, settings)
        .expect("settings");
    let job = state
        .sessions
        .begin_conversation_job(&token.id(), record.id)
        .expect("job");
    let record = state
        .conversations
        .begin_message_with_model(
            &record.id,
            record.revision,
            record.model.clone(),
            job.id(),
            "Read the file".to_owned(),
        )
        .expect("message");
    state
        .conversations
        .enqueue(
            &record.id,
            record.queue.revision,
            "Read the other file".to_owned(),
            QueueDelivery::Steering,
            Some(job.id()),
        )
        .expect("enqueue");
    let mut spec = granted_read_spec(connection, record.id, record.revision);
    spec.steering_session = Some(token.id());
    let ended = super::run_agent_action(
        &state,
        spec,
        vec![ChatTurn::user("Read the file".to_owned())],
        job.clone(),
    )
    .await;
    assert_eq!(ended.outcome, super::AgentOutcome::Completed);
    assert_eq!(backend.turn_count(), 2);
    assert_eq!(backend.last_extra_len(), 4);
    assert_eq!(job.snapshot().output.text, "Adjusted after both tools");
    let stored = state.conversations.get(&record.id).expect("stored");
    assert!(stored.queue.items.is_empty());
    assert!(stored.messages.iter().any(|message| {
        message.role == crate::conversations::MessageRole::User
            && message.text == "Read the other file"
    }));
    let tools = stored
        .messages
        .iter()
        .filter(|message| message.role == crate::conversations::MessageRole::Assistant)
        .flat_map(|message| message.activity.iter())
        .filter(|activity| {
            matches!(
                activity,
                crate::providers::AssistantActivity::ToolCall {
                    result: Some(_),
                    ..
                }
            )
        })
        .count();
    assert_eq!(tools, 2);
}

#[tokio::test]
async fn steering_requires_a_complete_response_and_live_owner() {
    use crate::conversations::QueueDelivery;
    use crate::providers::{
        ChatBackend, ChatTurn, CompletionReason, ModelEvent, ProviderConnection, ProviderKind,
    };
    for (reason, live) in [
        (CompletionReason::Stop, true),
        (CompletionReason::Length, true),
        (CompletionReason::Refusal, true),
        (CompletionReason::Unknown, true),
        (CompletionReason::Stop, false),
    ] {
        let mut state = crate::tests::test_state(crate::config::RuntimeConfig::development());
        let backend = crate::tests::ScriptedBackend::rounds(vec![
            vec![
                Ok(ModelEvent::Text("First answer".to_owned())),
                Ok(ModelEvent::Complete { reason }),
            ],
            vec![
                Ok(ModelEvent::Text("After correction".to_owned())),
                Ok(ModelEvent::Complete {
                    reason: CompletionReason::Stop,
                }),
            ],
        ]);
        state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend.clone()));
        let connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
        let token = crate::sessions::generate_session_token().unwrap();
        state.sessions.insert(token.id());
        let record = state.conversations.create("Steering".to_owned()).unwrap();
        let settings = crate::execution::ExecutionSettings::new(
            crate::providers::ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None)
                .unwrap(),
            String::new(),
            Vec::new(),
            crate::tests::test_environment_id(),
        )
        .unwrap();
        let record = state
            .conversations
            .update_execution_settings(&record.id, record.revision, settings)
            .unwrap();
        let job = state
            .sessions
            .begin_conversation_job(&token.id(), record.id)
            .unwrap();
        let record = state
            .conversations
            .begin_message_with_model(
                &record.id,
                record.revision,
                record.model.clone(),
                job.id(),
                "Hello".to_owned(),
            )
            .unwrap();
        state
            .conversations
            .enqueue(
                &record.id,
                record.queue.revision,
                "Correction".to_owned(),
                QueueDelivery::Steering,
                Some(job.id()),
            )
            .unwrap();
        let mut spec = granted_read_spec(connection, record.id, record.revision);
        spec.steering_session = Some(if live {
            token.id()
        } else {
            crate::sessions::generate_session_token().unwrap().id()
        });
        let ended = super::run_agent_action(
            &state,
            spec,
            vec![ChatTurn::user("Hello".to_owned())],
            job.clone(),
        )
        .await;
        let delivered = reason == CompletionReason::Stop && live;
        let stored = state.conversations.get(&record.id).unwrap();
        assert_eq!(stored.queue.items.is_empty(), delivered);
        assert_eq!(backend.turn_count(), if delivered { 2 } else { 1 });
        if delivered {
            assert_eq!(backend.last_extra_len(), 2);
            assert_eq!(stored.messages[1].text, "First answer");
            assert_eq!(job.snapshot().output.text, "After correction");
            assert_eq!(ended.reply.text, "After correction");
        } else if reason != CompletionReason::Stop {
            assert_eq!(ended.outcome, super::AgentOutcome::ProviderFailure);
        }
    }
}
