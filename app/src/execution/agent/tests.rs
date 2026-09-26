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
        resource: None,
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

#[tokio::test]
async fn automatic_compaction_fits_a_smaller_model_without_deleting_local_tool_results() {
    use crate::preferences::CompactionPreference;
    use crate::providers::{
        AssistantReply, ChatBackend, CompletionReason, ModelEvent, ProviderConnection,
        ProviderKind, ToolOutput,
    };
    let mut state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let backend = crate::tests::ScriptedBackend::events(vec![
        Ok(ModelEvent::Text("Earlier file read completed.".to_owned())),
        Ok(ModelEvent::Complete {
            reason: CompletionReason::Stop,
        }),
    ]);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend.clone()));
    // This fixture sits below the default 95 percent but above 90 percent.
    // Set the policy explicitly so the test pins compaction mechanics.
    state
        .preferences
        .set_compaction(CompactionPreference::new(true, 90).unwrap())
        .unwrap();
    let (record, job) = conversation_job(&state);
    // Each exchange fits alone, but both results exceed this model's context window.
    for (id, output) in [
        ("first", "x".repeat(65_000)),
        ("latest", "Retained file result".repeat(2_750)),
    ] {
        let mut reply = AssistantReply::default();
        reply.start_tool(
            id.to_owned(),
            "read".to_owned(),
            serde_json::json!({"path": "file.txt"}),
        );
        reply.finish_tool(
            id,
            ToolOutput {
                resource: None,
                label: "read".to_owned(),
                output,
                command: None,
            },
        );
        reply.completion = Some(CompletionReason::ToolCalls);
        state
            .conversations
            .settle_tool_batch(&record.id, job.id(), &reply)
            .unwrap();
    }
    let mut spec = read_spec(
        ProviderConnection::with_key(
            ProviderKind::Openrouter,
            "test-key",
            "qwen/qwen-2.5-7b-instruct",
        ),
        record.id,
        record.revision,
    );
    spec.steering_session = Some(crate::sessions::generate_session_token().unwrap().id());
    let mut turns = super::current_conversation_turns(&state, &spec).unwrap();
    let before = turns.clone();
    let mut guard = super::CompactionGuard::default();
    let result = super::fit_context(&state, &spec, &job, &mut turns, &[], &[], &mut guard).await;
    assert!(matches!(result, Ok(estimate) if estimate.fits() && !estimate.unknown_capacity()));
    assert_eq!(backend.turn_count(), 1);
    assert_eq!(
        turns
            .iter()
            .flat_map(|turn| &turn.calls)
            .map(|call| call.id.as_str())
            .collect::<Vec<_>>(),
        vec!["latest"]
    );
    let stored = state.conversations.get(&record.id).unwrap();
    assert_eq!(
        crate::conversations::history::project(&stored.messages, None).unwrap(),
        before
    );
    assert_eq!(stored.summary_requests.len(), 1);
}

#[tokio::test]
async fn disabled_automatic_compaction_blocks_provider_overflow_without_a_summary_request() {
    overflow_without_summary(false, 1).await;
}

#[tokio::test]
async fn below_threshold_provider_overflow_does_not_dispatch_a_summary() {
    overflow_without_summary(true, 95).await;
}

async fn overflow_without_summary(enabled: bool, threshold: u8) {
    use crate::config::RuntimeConfig;
    use crate::preferences::CompactionPreference;
    use crate::providers::{ChatBackend, ProviderConnection, ProviderError, ProviderKind};
    let mut state = crate::tests::test_state(RuntimeConfig::development());
    let backend =
        crate::tests::ScriptedBackend::turn_results(vec![Err(ProviderError::ContextOverflow)]);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend.clone()));
    state
        .preferences
        .set_compaction(CompactionPreference { enabled, threshold })
        .unwrap();
    let connection = ProviderConnection::with_key(
        ProviderKind::Openrouter,
        "test-key",
        "qwen/qwen-2.5-7b-instruct",
    );
    state.vault.put(connection.clone()).expect("provider");
    let (record, job) = conversation_job(&state);
    // A real summary boundary ensures that an unintended recovery can dispatch.
    for id in ["first", "latest"] {
        let mut reply = crate::providers::AssistantReply::default();
        reply.start_tool(
            id.to_owned(),
            "read".to_owned(),
            serde_json::json!({"path": "file.txt"}),
        );
        reply.finish_tool(
            id,
            crate::providers::ToolOutput {
                resource: None,
                label: "read".to_owned(),
                output: "file content ".repeat(500),
                command: None,
            },
        );
        reply.completion = Some(crate::providers::CompletionReason::ToolCalls);
        state
            .conversations
            .settle_tool_batch(&record.id, job.id(), &reply)
            .unwrap();
    }
    let mut spec = read_spec(connection, record.id, record.revision);
    spec.steering_session = Some(crate::sessions::generate_session_token().unwrap().id());
    let turns = super::current_conversation_turns(&state, &spec).unwrap();
    let estimate = super::current_estimate(&state, &spec, &turns, &[], &spec.tools).unwrap();
    assert_eq!(
        estimate.automatic_trigger(state.preferences.compaction()),
        !enabled
    );
    let ended = super::run_agent_action(&state, spec, turns, job).await;
    assert_eq!(ended.outcome, super::AgentOutcome::ContextBlocked);
    assert_eq!(backend.turn_count(), 1);
    let stored = state.conversations.get(&record.id).unwrap();
    assert!(stored.compaction.is_none());
    assert!(stored.summary_requests.is_empty());
}

#[tokio::test]
async fn workflow_compaction_preserves_scope_and_complete_tool_exchanges() {
    use crate::providers::{
        ChatBackend, ChatTurn, CompletionReason, ModelEvent, ProviderConnection, ProviderKind,
    };
    let mut state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let backend = crate::tests::ScriptedBackend::events(vec![
        Ok(ModelEvent::Text("Phase summary".to_owned())),
        Ok(ModelEvent::Complete {
            reason: CompletionReason::Stop,
        }),
    ]);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend.clone()));
    let (record, job) = conversation_job(&state);
    let mut spec = read_spec(
        ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6"),
        record.id,
        record.revision,
    );
    let run = crate::workflows::RunId::generate().unwrap();
    let attempt = crate::workflows::AttemptId::generate().unwrap();
    spec.evidence = Some(crate::workflows::AttemptEvidenceContext::new(
        state.workflow_evidence.clone(),
        run,
        attempt,
        "phase",
    ));
    let mut first = ChatTurn::user("First phase response".to_owned());
    first.role = crate::providers::Role::Assistant;
    first.calls.push(crate::providers::ChatToolCall {
        id: "call-one".to_owned(),
        name: "read".to_owned(),
        arguments: serde_json::json!({"path": "phase.txt"}),
        result: Some(crate::providers::ToolOutput {
            resource: None,
            label: "read".to_owned(),
            // The first phase exceeds the recent-context budget, so it is
            // summarised while the small latest phase is retained whole.
            output: "Phase evidence".repeat(10_000),
            command: None,
        }),
    });
    let mut last = first.clone();
    last.calls[0].id = "call-two".to_owned();
    last.calls[0].result.as_mut().unwrap().output = "Phase evidence".to_owned();
    spec.context_prefix_len = 1;
    let mut turns = vec![
        ChatTurn::user("Pinned phase input".to_owned()),
        first,
        last.clone(),
    ];
    for turn in &turns[1..] {
        spec.evidence.as_ref().unwrap().turn(turn).unwrap();
    }
    let result = super::compact_history(&state, &spec, &job, &mut turns, &[], &spec.tools).await;
    assert!(result.is_ok());
    assert_eq!(turns.len(), 3);
    assert_eq!(turns[0].text, "Pinned phase input");
    assert_eq!(turns[2], last);
    assert!(backend.last_tools().is_empty());
    assert_eq!(backend.last_extra_len(), 0);
    let prompt = &backend.last_history()[0].text;
    assert!(prompt.contains("phase.txt"));
    assert!(prompt.contains("Phase evidence"));
    assert!(!prompt.contains("Read the file"));
    assert!(!prompt.contains("Pinned phase input"));
    assert!(
        state
            .conversations
            .get(&record.id)
            .unwrap()
            .compaction
            .is_none()
    );
    let evidence = state.workflow_evidence.get(&run, &attempt).unwrap();
    assert_eq!(evidence.compaction.unwrap().covered_through, 0);
    assert_eq!(
        evidence
            .events
            .iter()
            .filter(|event| event.request.is_some())
            .count(),
        1
    );
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
        context_prefix_len: 0,
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
        budget: crate::execution::BudgetPolicy::ordinary(),
        sources: Vec::new(),
        advertised: Vec::new(),
    }
}

#[tokio::test]
async fn stream_fragmentation_does_not_limit_reply_length() {
    use crate::providers::{
        ChatBackend, ChatTurn, CompletionReason, ModelEvent, ProviderConnection, ProviderKind,
    };

    for fragmented in [false, true] {
        let mut state = crate::tests::test_state(crate::config::RuntimeConfig::development());
        let mut events = Vec::new();
        let pieces = if fragmented { 4097 } else { 1 };
        let piece = if fragmented {
            "x".to_owned()
        } else {
            "x".repeat(4097)
        };
        for _ in 0..pieces {
            events.push(Ok(ModelEvent::Thinking(piece.clone())));
        }
        for _ in 0..pieces {
            events.push(Ok(ModelEvent::Text(piece.clone())));
        }
        events.push(Ok(ModelEvent::Complete {
            reason: CompletionReason::Stop,
        }));
        state.chat = std::sync::Arc::new(ChatBackend::Scripted(
            crate::tests::ScriptedBackend::events(events),
        ));
        let connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
        let (record, job) = conversation_job(&state);
        let mut spec = read_spec(connection, record.id, record.revision);
        spec.conversation = None;
        let ended = super::run_agent_action(
            &state,
            spec,
            vec![ChatTurn::user("Explain the result".to_owned())],
            job,
        )
        .await;
        assert_eq!(ended.outcome, super::AgentOutcome::Completed);
        assert!(ended.error.is_none());
        assert_eq!(ended.reply.text, "x".repeat(4097));
        assert_eq!(ended.reply.thinking, "x".repeat(4097));
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
async fn sandbox_run_without_approval_context_fails_closed() {
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
    assert_eq!(ended.outcome, super::AgentOutcome::AuthorityFailure);
    assert_eq!(ended.reply.tools.len(), 1);
    assert!(
        ended.reply.tools[0]
            .output
            .contains("Command approval is unavailable.")
    );
    assert!(ended.reply.text.is_empty());
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
    assert_eq!(
        backend
            .last_history()
            .iter()
            .flat_map(|turn| &turn.calls)
            .filter(|call| call.result.is_some())
            .count(),
        1
    );
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
    let sent = backend.last_history();
    assert_eq!(sent.last().unwrap().text, "Read the other file");
    assert_eq!(
        sent.iter()
            .flat_map(|turn| &turn.calls)
            .filter(|call| call.result.is_some())
            .count(),
        2
    );
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
            let sent = backend.last_history();
            assert_eq!(
                sent.iter()
                    .filter(|turn| turn.text == "First answer")
                    .count(),
                1
            );
            assert_eq!(sent.len(), 3);
            assert_eq!(stored.messages[1].text, "First answer");
            assert_eq!(job.snapshot().output.text, "After correction");
            assert_eq!(ended.reply.text, "After correction");
        } else if reason != CompletionReason::Stop {
            assert_eq!(ended.outcome, super::AgentOutcome::ProviderFailure);
        }
    }
}

#[tokio::test]
async fn budget_exhaustion_pauses_after_a_settled_tool_batch() {
    use crate::config::RuntimeConfig;
    use crate::execution::BudgetPolicy;
    use crate::providers::{
        ChatBackend, ChatTurn, CompletionReason, ModelEvent, ProviderConnection, ProviderKind,
    };
    use std::time::Duration;
    let mut state = crate::tests::test_state(RuntimeConfig::development());
    let backend = crate::tests::ScriptedBackend::rounds(vec![
        vec![
            Ok(ModelEvent::ToolCall {
                id: "call-1".to_owned(),
                name: "read".to_owned(),
                arguments: serde_json::json!({"path": "src/main.rs"}),
            }),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::ToolCalls,
            }),
        ],
        vec![
            Ok(ModelEvent::Text("should not run".to_owned())),
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
    spec.budget = BudgetPolicy {
        model_requests: 1,
        tool_dispatches: 20,
        elapsed: Duration::from_secs(60),
    };
    let ended = super::run_agent_action(
        &state,
        spec,
        vec![ChatTurn::user("Read the file".to_owned())],
        job.clone(),
    )
    .await;
    assert_eq!(ended.outcome, super::AgentOutcome::BudgetExhausted);
    assert_eq!(ended.reply.tools.len(), 1);
    assert!(ended.budget.is_some());
    assert_eq!(tool_finished_count(&job), 1);
}

#[tokio::test]
async fn ordinary_budget_allows_more_than_twelve_rounds() {
    use crate::config::RuntimeConfig;
    use crate::providers::{
        ChatBackend, ChatTurn, CompletionReason, ModelEvent, ProviderConnection, ProviderKind,
    };
    let mut state = crate::tests::test_state(RuntimeConfig::development());
    let mut rounds = Vec::new();
    for index in 0..13 {
        rounds.push(vec![
            Ok(ModelEvent::ToolCall {
                id: format!("call-{index}"),
                name: "read".to_owned(),
                arguments: serde_json::json!({"path": "src/main.rs"}),
            }),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::ToolCalls,
            }),
        ]);
    }
    rounds.push(vec![
        Ok(ModelEvent::Text("Done after thirteen rounds".to_owned())),
        Ok(ModelEvent::Complete {
            reason: CompletionReason::Stop,
        }),
    ]);
    let backend = crate::tests::ScriptedBackend::rounds(rounds);
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
    assert_eq!(ended.reply.text, "Done after thirteen rounds");
    assert_eq!(tool_finished_count(&job), 13);
}

#[tokio::test]
async fn retries_consume_the_model_request_budget_without_replaying_tools() {
    use crate::providers::{
        ChatBackend, ChatTurn, CompletionReason, ModelEvent, ProviderConnection, ProviderError,
        ProviderKind,
    };
    let mut state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let backend = crate::tests::ScriptedBackend::turn_results(vec![
        Ok(vec![
            Ok(ModelEvent::ToolCall {
                id: "completed".to_owned(),
                name: "read".to_owned(),
                arguments: serde_json::json!({"path": "src/main.rs"}),
            }),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::ToolCalls,
            }),
        ]),
        Err(ProviderError::RateLimited { retry_after: None }),
        Ok(vec![
            Ok(ModelEvent::Text("Over budget".to_owned())),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::Stop,
            }),
        ]),
    ]);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend));
    let connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    let (record, job) = conversation_job(&state);
    let mut spec = granted_read_spec(connection, record.id, record.revision);
    spec.budget.model_requests = 2;
    let ended = super::run_agent_action(
        &state,
        spec,
        vec![ChatTurn::user("Read".to_owned())],
        job.clone(),
    )
    .await;
    assert_eq!(ended.outcome, super::AgentOutcome::BudgetExhausted);
    assert_eq!(ended.budget.unwrap().model_requests, 2);
    assert_eq!(ended.reply.tools.len(), 1);
    assert!(!ended.reply.text.contains("Over budget"));
    assert_eq!(ended.budget.unwrap().tool_dispatches, 1);
}

#[tokio::test]
async fn a_tool_batch_cannot_exceed_the_dispatch_budget() {
    use crate::providers::{
        ChatBackend, ChatTurn, CompletionReason, ModelEvent, ProviderConnection, ProviderKind,
    };
    let mut state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let backend = crate::tests::ScriptedBackend::rounds(vec![vec![
        Ok(ModelEvent::ToolCall {
            id: "first".to_owned(),
            name: "read".to_owned(),
            arguments: serde_json::json!({"path": "src/main.rs"}),
        }),
        Ok(ModelEvent::ToolCall {
            id: "second".to_owned(),
            name: "read".to_owned(),
            arguments: serde_json::json!({"path": "src/main.rs"}),
        }),
        Ok(ModelEvent::Complete {
            reason: CompletionReason::ToolCalls,
        }),
    ]]);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend));
    let connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    let (record, job) = conversation_job(&state);
    let mut spec = granted_read_spec(connection, record.id, record.revision);
    spec.budget.tool_dispatches = 1;
    let ended =
        super::run_agent_action(&state, spec, vec![ChatTurn::user("Read".to_owned())], job).await;
    assert_eq!(ended.outcome, super::AgentOutcome::BudgetExhausted);
    let budget = ended.budget.unwrap();
    assert!(budget.valid());
    assert_eq!(budget.tool_dispatches, 1);
    assert_eq!(ended.reply.tools.len(), 2);
    assert_eq!(
        ended.reply.tools[1].command.as_ref().unwrap().termination,
        crate::execution::CommandTermination::NotDispatched
    );
}

#[tokio::test]
async fn retry_keeps_separate_usage_records_including_unknown_values() {
    use crate::conversations::MessageStatus;
    use crate::providers::{
        ChatBackend, ChatTurn, CompletionReason, ModelEvent, ProviderConnection, ProviderError,
        ProviderKind,
    };
    let mut state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let backend = crate::tests::ScriptedBackend::turn_results(vec![
        Err(ProviderError::RateLimited { retry_after: None }),
        Ok(vec![
            Ok(ModelEvent::Text("Recovered.".to_owned())),
            Ok(ModelEvent::Usage {
                input_tokens: Some(11),
                output_tokens: Some(4),
                cache_read_tokens: None,
                cache_creation_tokens: None,
            }),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::Stop,
            }),
        ]),
    ]);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend));
    let connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    let (record, job) = conversation_job(&state);
    let ended = super::run_agent_action(
        &state,
        granted_read_spec(connection, record.id, record.revision),
        vec![ChatTurn::user("Hello".to_owned())],
        job,
    )
    .await;
    assert_eq!(ended.outcome, super::AgentOutcome::Completed);
    assert_eq!(ended.reply.usage.len(), 1);
    assert_eq!(ended.reply.usage[0].usage.input_tokens, Some(11));
    assert_eq!(ended.reply.usage[0].usage.output_tokens, Some(4));
    assert_eq!(ended.reply.usage[0].usage.cache_read_tokens, None);
    let stored = state.conversations.get(&record.id).expect("stored");
    let failed = stored
        .messages
        .iter()
        .find(|message| message.status == MessageStatus::Failed)
        .expect("failed attempt");
    assert_eq!(failed.requests.len(), 1);
    assert!(!failed.requests[0].usage.has_tokens());
    assert_ne!(failed.requests[0].id, ended.reply.usage[0].id);
    assert_eq!(failed.requests[0].usage.model, "grok-4.6");
    assert_eq!(ended.reply.usage[0].usage.model, "grok-4.6");
}

#[tokio::test]
async fn plan_authentication_does_not_snapshot_api_prices() {
    use crate::providers::{
        AuthMethod, ChatBackend, ChatTurn, CompletionReason, ModelEvent, ProviderConnection,
        ProviderKind,
    };
    let mut state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let backend = crate::tests::ScriptedBackend::events(vec![
        Ok(ModelEvent::Text("Hello".to_owned())),
        Ok(ModelEvent::Usage {
            input_tokens: Some(8),
            output_tokens: Some(2),
            cache_read_tokens: None,
            cache_creation_tokens: None,
        }),
        Ok(ModelEvent::Complete {
            reason: CompletionReason::Stop,
        }),
    ]);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend));
    let mut connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    connection.auth = AuthMethod::Plan;
    let (record, job) = conversation_job(&state);
    let ended = super::run_agent_action(
        &state,
        granted_read_spec(connection, record.id, record.revision),
        vec![ChatTurn::user("Hello".to_owned())],
        job,
    )
    .await;
    assert_eq!(ended.outcome, super::AgentOutcome::Completed);
    assert_eq!(ended.reply.usage.len(), 1);
    assert_eq!(ended.reply.usage[0].auth, AuthMethod::Plan);
    assert!(ended.reply.usage[0].prices.is_none());
    assert_eq!(
        crate::conversations::history::request_cost(&ended.reply.usage[0]).known_micros,
        None
    );
}

#[tokio::test]
async fn provider_requests_honour_the_selected_output_allowance() {
    use crate::providers::{
        ChatBackend, ChatTurn, CompletionReason, ModelEvent, ProviderConnection, ProviderKind,
    };
    let mut state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let backend = crate::tests::ScriptedBackend::events(vec![
        Ok(ModelEvent::Text("Hello".to_owned())),
        Ok(ModelEvent::Complete {
            reason: CompletionReason::Stop,
        }),
    ]);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend.clone()));
    let connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    state.vault.put(connection.clone()).expect("provider");
    let (record, job) = conversation_job(&state);
    let ended = super::run_agent_action(
        &state,
        granted_read_spec(connection, record.id, record.revision),
        vec![ChatTurn::user("Hello".to_owned())],
        job,
    )
    .await;
    assert_eq!(ended.outcome, super::AgentOutcome::Completed);
    assert!(backend.last_max_tokens().is_some());
}

#[tokio::test]
async fn a_compacted_long_turn_settles_once_without_replaying_settled_tools() {
    use crate::providers::{
        AssistantReply, ChatBackend, CompletionReason, ModelEvent, ProviderConnection,
        ProviderKind, ToolOutput,
    };
    let mut state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    // This fixture sits below the default 95 percent but above 90 percent.
    state
        .preferences
        .set_compaction(crate::preferences::CompactionPreference::new(true, 90).unwrap())
        .unwrap();
    let backend = crate::tests::ScriptedBackend::rounds(vec![
        vec![
            Ok(ModelEvent::Text("Retained file result".repeat(2_750))),
            Ok(ModelEvent::ToolCall {
                id: "latest".to_owned(),
                name: "run".to_owned(),
                arguments: serde_json::json!({"command": "printf x >> effects", "explanation": "Record one effect"}),
            }),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::ToolCalls,
            }),
        ],
        vec![
            Ok(ModelEvent::Text("Earlier file read completed.".to_owned())),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::Stop,
            }),
        ],
        vec![
            Ok(ModelEvent::Text("Long turn complete.".to_owned())),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::Stop,
            }),
        ],
    ]);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend.clone()));
    let directory = tempfile::tempdir().unwrap();
    let records = tempfile::tempdir().unwrap();
    state.conversations = std::sync::Arc::new(
        crate::conversations::ConversationStore::open(records.path().to_owned()).unwrap(),
    );
    let session = crate::sessions::generate_session_token().unwrap().id();
    state.sessions.insert(session);
    let record = state.conversations.create("Long turn".to_owned()).unwrap();
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(
            ProviderKind::Openrouter,
            "qwen/qwen-2.5-7b-instruct".to_owned(),
            None,
        )
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
    // The next real tool batch must commit before its compaction decision.
    {
        let id = "first";
        let output = "x".repeat(65_000);
        let mut reply = AssistantReply::default();
        reply.start_tool(
            id.to_owned(),
            "read".to_owned(),
            serde_json::json!({"path": "file.txt"}),
        );
        reply.finish_tool(
            id,
            ToolOutput {
                resource: None,
                label: "read".to_owned(),
                output,
                command: None,
            },
        );
        reply.completion = Some(CompletionReason::ToolCalls);
        state
            .conversations
            .settle_tool_batch(&record.id, job.id(), &reply)
            .unwrap();
    }
    let connection = ProviderConnection::with_key(
        ProviderKind::Openrouter,
        "test-key",
        "qwen/qwen-2.5-7b-instruct",
    );
    state.vault.put(connection.clone()).expect("provider");
    let mut spec = read_spec(connection, record.id, record.revision);
    spec.steering_session = Some(session);
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
    let turns = super::current_conversation_turns(&state, &spec).unwrap();
    let ended = super::run_agent_action(&state, spec, turns, job.clone()).await;
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
    assert_eq!(backend.turn_count(), 3);
    let requests = backend.captured();
    let final_request = requests.last().expect("final request");
    assert!(
        final_request
            .history
            .iter()
            .any(|turn| turn.text.contains("Earlier file read completed.")),
        "the covered context is replaced by its summary"
    );
    let final_calls: Vec<&str> = final_request
        .history
        .iter()
        .flat_map(|turn| &turn.calls)
        .map(|call| call.id.as_str())
        .collect();
    assert_eq!(final_calls.iter().filter(|id| **id == "latest").count(), 1);
    assert_eq!(backend.last_extra_len(), 0);
    assert!(
        !final_calls.contains(&"first"),
        "a covered settled tool is never replayed"
    );
    let stored = state.conversations.get(&record.id).unwrap();
    let mut ids: Vec<String> = stored
        .messages
        .iter()
        .flat_map(|message| &message.activity)
        .filter_map(|activity| match activity {
            crate::providers::AssistantActivity::ToolCall { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    let count = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), count, "no durable tool call is duplicated");
    assert_eq!(count, 2);
    // Reopen before logical settlement to simulate interruption after compaction.
    let reopened =
        crate::conversations::ConversationStore::open(records.path().to_owned()).unwrap();
    let interrupted = reopened.get(&record.id).unwrap();
    assert_eq!(
        std::fs::read(directory.path().join("effects")).unwrap(),
        b"x"
    );
    assert_eq!(
        interrupted
            .messages
            .iter()
            .filter(|message| message.final_phase)
            .count(),
        1
    );
    let mut after: Vec<String> = interrupted
        .messages
        .iter()
        .flat_map(|message| &message.activity)
        .filter_map(|activity| match activity {
            crate::providers::AssistantActivity::ToolCall { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    after.sort();
    after.dedup();
    assert_eq!(after.len(), 2);
}
