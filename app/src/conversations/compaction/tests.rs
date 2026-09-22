use crate::providers::{AssistantActivity, AuthMethod, ModelUsage, ProviderKind, ToolOutput};

use super::{CompactionError, CompactionRecord, project, select_boundary, validate_summary};
use crate::conversations::history::{
    CommandEntry, ConversationMessage, MessageRole, MessageStatus, RequestUsage,
};
use crate::conversations::{MessageId, RequestId};
use crate::execution::command::CommandChunk;
use crate::execution::{CommandResult, CommandStream, CommandTermination};

fn identifier() -> MessageId {
    MessageId::generate().expect("message id")
}

fn selection() -> crate::providers::ModelSelection {
    crate::providers::ModelSelection {
        provider: ProviderKind::Xai,
        model: "grok-4.6".to_owned(),
        thinking: None,
    }
}

fn cost(messages: &[ConversationMessage]) -> u64 {
    let turns =
        crate::conversations::history::project(messages, Some(&selection())).expect("turns");
    crate::execution::context::count_turns(ProviderKind::Xai, "grok-4.6", &turns).expect("tokens")
}

fn usage() -> RequestUsage {
    RequestUsage {
        id: RequestId::generate().expect("request"),
        usage: ModelUsage::new(ProviderKind::Xai, "grok-4.6"),
        auth: AuthMethod::ApiKey,
        prices: None,
        sources: Vec::new(),
        advertised: Vec::new(),
    }
}

fn user(text: &str) -> ConversationMessage {
    ConversationMessage {
        parent: None,
        id: identifier(),
        response: None,
        final_phase: false,
        role: MessageRole::User,
        text: text.to_owned(),
        input: None,
        command: None,
        attachments: Vec::new(),
        activity: Vec::new(),
        continuation: Vec::new(),
        status: MessageStatus::Complete,
        error: None,
        request: None,
        completion: None,
        requests: Vec::new(),
    }
}

fn command(included: bool) -> ConversationMessage {
    ConversationMessage {
        parent: None,
        id: identifier(),
        response: None,
        final_phase: false,
        role: MessageRole::Command,
        text: "echo boundary".to_owned(),
        input: None,
        command: Some(CommandEntry {
            included,
            directory: "/workspace".to_owned(),
            output: Some(CommandResult::new(
                vec![CommandChunk {
                    stream: CommandStream::Stdout,
                    text: "boundary output".to_owned(),
                }],
                CommandTermination::Exited(0),
            )),
            before: None,
            after: None,
        }),
        attachments: Vec::new(),
        activity: Vec::new(),
        continuation: Vec::new(),
        status: MessageStatus::Complete,
        error: None,
        request: None,
        completion: None,
        requests: Vec::new(),
    }
}

#[test]
fn a_settled_included_command_is_a_compaction_boundary() {
    let messages = vec![
        user("First"),
        command(true),
        user("Second"),
        assistant("Done"),
    ];
    let budget = cost(&messages[2..=3]);
    let (covered, retained) =
        select_boundary(&messages, None, Some(&selection()), budget).expect("boundary");
    assert_eq!(covered, messages[1].id);
    assert_eq!(retained, messages[2].id);
}

fn assistant(text: &str) -> ConversationMessage {
    let id = identifier();
    ConversationMessage {
        parent: None,
        id,
        response: Some(id),
        final_phase: true,
        role: MessageRole::Assistant,
        text: text.to_owned(),
        input: None,
        command: None,
        attachments: Vec::new(),
        activity: Vec::new(),
        continuation: Vec::new(),
        status: MessageStatus::Complete,
        error: None,
        request: None,
        completion: None,
        requests: Vec::new(),
    }
}

fn assistant_tool(id: &str, result: Option<ToolOutput>) -> ConversationMessage {
    let message = identifier();
    ConversationMessage {
        parent: None,
        id: message,
        response: Some(message),
        final_phase: true,
        role: MessageRole::Assistant,
        text: String::new(),
        input: None,
        command: None,
        attachments: Vec::new(),
        activity: vec![AssistantActivity::ToolCall {
            id: id.to_owned(),
            name: "read".to_owned(),
            arguments: serde_json::json!({"path": "main.rs"}),
            result,
        }],
        continuation: Vec::new(),
        status: MessageStatus::Complete,
        error: None,
        request: None,
        completion: None,
        requests: Vec::new(),
    }
}

fn two_exchanges() -> Vec<ConversationMessage> {
    vec![
        user("First"),
        assistant("First reply"),
        user("Second"),
        assistant("Second reply"),
    ]
}

#[test]
fn compaction_preserves_consumed_resource_provenance() {
    let source = crate::execution::ResourceSource::new(
        crate::execution::ResourceKind::Skill,
        "project",
        "/project/.agents/skills/review/SKILL.md",
        b"review instructions",
    );
    let messages = vec![
        user("Read the skill"),
        assistant_tool(
            "skill-read",
            Some(ToolOutput {
                resource: Some(source.clone()),
                label: "read".to_owned(),
                output: "review instructions".to_owned(),
                command: None,
            }),
        ),
        assistant("Skill read"),
        user("Continue"),
        assistant("Continued"),
    ];
    let turns = crate::conversations::history::project(&messages, None).expect("history");
    let sources = crate::execution::resources::consumed_sources(&[], &turns);
    assert_eq!(sources, vec![source.clone()]);
    let mut request = usage();
    request.sources = sources;
    let budget = cost(&messages[3..=4]);
    let (covered_through, retained_from) =
        select_boundary(&messages, None, Some(&selection()), budget).expect("boundary");
    let record = CompactionRecord {
        covered_through,
        retained_from,
        text: "The review skill supplied instructions.".to_owned(),
        requests: vec![
            serde_json::from_slice(&serde_json::to_vec(&request).expect("persist request"))
                .expect("reload request"),
        ],
        preserve: None,
        created_at_ms: 1,
    };
    let compacted = project(&messages, None, Some(&record)).expect("compacted history");
    assert_eq!(
        crate::execution::resources::consumed_sources(&[], &compacted),
        vec![source]
    );
}

#[test]
fn a_recent_context_budget_keeps_the_latest_complete_exchange() {
    let messages = two_exchanges();
    let budget = cost(&messages[2..=3]);
    let (covered, retained) =
        select_boundary(&messages, None, Some(&selection()), budget).expect("boundary");
    assert_eq!(covered, messages[1].id);
    assert_eq!(retained, messages[2].id);
    // When the whole history fits, the earliest exchange is still covered so a
    // requested compaction always reduces context.
    let (covered, retained) = select_boundary(
        &messages,
        None,
        Some(&selection()),
        super::RECENT_CONTEXT_TOKENS,
    )
    .expect("boundary");
    assert_eq!(covered, messages[1].id);
    assert_eq!(retained, messages[2].id);
}

#[test]
fn retention_budget_clamps_against_overhead_and_headroom() {
    assert_eq!(
        super::retention_budget(200_000, 1_000, 4_000, 500),
        super::RECENT_CONTEXT_TOKENS
    );
    assert_eq!(super::retention_budget(30_000, 5_000, 4_000, 1_000), 11_000);
    assert_eq!(super::retention_budget(20_000, 5_000, 4_000, 1_000), 1_000);
    assert_eq!(super::retention_budget(8_000, 5_000, 4_000, 0), 0);
}

#[test]
fn no_complete_exchange_in_the_budget_is_reported() {
    let messages = vec![
        user(&"a".repeat(4_000)),
        assistant(&"b".repeat(4_000)),
        user("Later"),
        assistant("Done"),
    ];
    assert_eq!(
        select_boundary(&messages, None, Some(&selection()), 0),
        Err(CompactionError::Retention)
    );
}

#[test]
fn a_retained_exchange_is_not_split_from_its_tool_result() {
    let messages = vec![
        user(&"a".repeat(4_000)),
        assistant(&"b".repeat(4_000)),
        user("Run the tool"),
        assistant_tool(
            "call-latest",
            Some(ToolOutput {
                resource: None,
                label: "read".to_owned(),
                output: "latest output".to_owned(),
                command: None,
            }),
        ),
        user("Continue"),
        assistant("Done"),
    ];
    let budget = cost(&messages[4..=5]);
    let (covered, retained) =
        select_boundary(&messages, None, Some(&selection()), budget).expect("boundary");
    assert_eq!(covered, messages[3].id);
    assert_eq!(retained, messages[4].id);
}

#[test]
fn repeated_coverage_extends_one_previous_summary() {
    let messages = vec![
        user(&"a".repeat(4_000)),
        assistant(&"b".repeat(4_000)),
        user(&"c".repeat(4_000)),
        assistant(&"d".repeat(4_000)),
        user("Recent"),
        assistant("Recent reply"),
    ];
    let first = CompactionRecord {
        covered_through: messages[1].id,
        retained_from: messages[2].id,
        text: "First summary".to_owned(),
        requests: vec![usage()],
        preserve: None,
        created_at_ms: 1,
    };
    let budget = cost(&messages[4..=5]);
    let (covered, retained) =
        select_boundary(&messages, Some(&first), Some(&selection()), budget).expect("boundary");
    assert_eq!(covered, messages[3].id);
    assert_eq!(retained, messages[4].id);
    let covered = super::covered_turns(&messages, Some(&selection()), covered, Some(&first))
        .expect("covered turns");
    assert!(covered[0].text.contains("First summary"));
    assert_eq!(covered.len(), 3);
    assert_eq!(covered[1].text, "c".repeat(4_000));
}

#[test]
fn a_tool_call_cannot_split_from_its_result() {
    let messages = vec![
        user("Read"),
        assistant_tool("call-1", None),
        user("Later"),
        assistant("Done"),
    ];
    assert_eq!(
        select_boundary(
            &messages,
            None,
            Some(&selection()),
            super::RECENT_CONTEXT_TOKENS
        ),
        Err(CompactionError::Unsettled)
    );
}

#[test]
fn one_exchange_cannot_compact() {
    let messages = vec![user("Hello"), assistant("Hi")];
    assert_eq!(
        select_boundary(
            &messages,
            None,
            Some(&selection()),
            super::RECENT_CONTEXT_TOKENS
        ),
        Err(CompactionError::NothingToCompact)
    );
}

#[test]
fn a_malformed_summary_does_not_replace_context() {
    assert_eq!(validate_summary("", None), Err(CompactionError::Malformed));
    assert_eq!(
        validate_summary("bad\0summary", None),
        Err(CompactionError::Malformed)
    );
    assert_eq!(
        validate_summary("leak", Some("leak")),
        Err(CompactionError::Malformed)
    );
    let messages = two_exchanges();
    let record = CompactionRecord {
        covered_through: messages[1].id,
        retained_from: messages[2].id,
        text: String::new(),
        requests: vec![usage()],
        preserve: None,
        created_at_ms: 1,
    };
    assert!(!record.valid(&messages));
    assert!(project(&messages, None, Some(&record)).is_err());
}

#[test]
fn a_valid_summary_replaces_only_the_model_projection() {
    let messages = two_exchanges();
    let record = CompactionRecord {
        covered_through: messages[1].id,
        retained_from: messages[2].id,
        text: "Earlier the user asked first.".to_owned(),
        requests: vec![usage()],
        preserve: None,
        created_at_ms: 1,
    };
    assert!(record.valid(&messages));
    let history = project(&messages, None, Some(&record)).expect("projection");
    assert_eq!(history.len(), 3);
    assert!(history[0].text.ends_with("Earlier the user asked first."));
    assert_eq!(history[1].text, "Second");
    assert_eq!(history[2].text, "Second reply");
}

#[test]
fn a_missing_cover_boundary_is_rejected() {
    let messages = two_exchanges();
    let record = CompactionRecord {
        covered_through: identifier(),
        retained_from: messages[2].id,
        text: "Stale".to_owned(),
        requests: vec![usage()],
        preserve: None,
        created_at_ms: 1,
    };
    assert!(!record.valid(&messages));
    assert_eq!(
        select_boundary(
            &messages,
            Some(&record),
            Some(&selection()),
            super::RECENT_CONTEXT_TOKENS
        ),
        Err(CompactionError::Malformed)
    );
}

#[test]
fn a_summary_keeps_its_boundary_when_a_later_request_is_pending() {
    let mut messages = two_exchanges();
    let record = CompactionRecord {
        covered_through: messages[1].id,
        retained_from: messages[2].id,
        text: "Earlier context".to_owned(),
        requests: vec![usage()],
        preserve: None,
        created_at_ms: 1,
    };
    messages.push(user("Next"));
    let mut pending = assistant("");
    pending.status = MessageStatus::Pending;
    messages.push(pending);
    assert!(record.valid(&messages));
    let projected = project(&messages, None, Some(&record)).unwrap();
    assert_eq!(projected.last().unwrap().text, "Next");
    let mut skipped = record.clone();
    skipped.retained_from = messages[4].id;
    assert_eq!(project(&messages, None, Some(&skipped)).unwrap(), projected);
}

#[test]
fn interrupted_completed_tools_do_not_shift_the_retained_suffix() {
    let result = ToolOutput {
        resource: None,
        label: "read".to_owned(),
        output: "file contents".to_owned(),
        command: None,
    };
    let mut interrupted = assistant_tool("first", Some(result));
    interrupted.status = MessageStatus::Interrupted;
    let messages = vec![
        user("Read"),
        interrupted,
        user("Explain"),
        assistant("Explanation"),
        user("Retained"),
        assistant("Latest"),
    ];
    let record = CompactionRecord {
        covered_through: messages[3].id,
        retained_from: messages[4].id,
        text: "Summary".to_owned(),
        requests: vec![usage()],
        preserve: None,
        created_at_ms: 1,
    };
    let projected = project(&messages, None, Some(&record)).unwrap();
    assert_eq!(projected.len(), 3);
    assert_eq!(projected[1].text, "Retained");
    let covered = super::covered_turns(&messages, None, messages[3].id, None).unwrap();
    assert_eq!(covered.len(), 4);
    assert_eq!(
        covered[1].calls[0].result.as_ref().unwrap().output,
        "file contents"
    );
}

#[test]
fn a_summary_checkpoint_is_isolated_to_its_sibling_path() {
    let shared_user = user("Start");
    let shared_assistant = assistant("Shared prefix");
    let shared_id = shared_assistant.id;
    let a_user = user("A question");
    let a_assistant = assistant("A reply");
    let b_user = user("B question");
    let b_assistant = assistant("B reply");
    let path_a = vec![
        shared_user.clone(),
        shared_assistant.clone(),
        a_user.clone(),
        a_assistant,
    ];
    let path_b = vec![shared_user, shared_assistant, b_user, b_assistant];
    let record = CompactionRecord {
        covered_through: shared_id,
        retained_from: a_user.id,
        text: "Shared prefix summary".to_owned(),
        requests: vec![usage()],
        preserve: None,
        created_at_ms: 1,
    };
    assert!(record.valid(&path_a));
    let projected = project(&path_b, None, Some(&record)).unwrap();
    assert_eq!(projected.len(), 3);
    assert!(projected[0].text.contains("Shared prefix summary"));
    assert_eq!(projected[1].text, "B question");
    assert_eq!(projected[2].text, "B reply");
    let sibling_only = CompactionRecord {
        covered_through: path_a[3].id,
        retained_from: identifier(),
        text: "Private A context".to_owned(),
        ..record
    };
    assert!(!sibling_only.valid(&path_b));
    assert!(project(&path_b, None, Some(&sibling_only)).is_err());
}

#[test]
fn preserve_instructions_are_bounded_and_normalised() {
    assert_eq!(super::normalise_preserve(None), Ok(None));
    assert_eq!(super::normalise_preserve(Some("   ")), Ok(None));
    assert_eq!(
        super::normalise_preserve(Some("  keep the plan  ")),
        Ok(Some("keep the plan".to_owned()))
    );
    assert_eq!(
        super::normalise_preserve(Some(&"x".repeat(super::MAXIMUM_PRESERVE_BYTES + 1))),
        Err(CompactionError::Preserve)
    );
    assert_eq!(
        super::normalise_preserve(Some("bad\0text")),
        Err(CompactionError::Preserve)
    );
}

#[test]
fn a_checkpoint_records_every_summary_request_once() {
    let messages = two_exchanges();
    let record = CompactionRecord {
        covered_through: messages[1].id,
        retained_from: messages[2].id,
        text: "Cumulative summary".to_owned(),
        requests: vec![usage(), usage()],
        preserve: Some("keep the plan".to_owned()),
        created_at_ms: 1,
    };
    assert!(record.valid(&messages));
    let history = project(&messages, None, Some(&record)).expect("projection");
    assert_eq!(history[0].usage.len(), 2);
    let mut malformed = record.clone();
    malformed.requests.clear();
    assert!(!malformed.valid(&messages));
    let mut unsupported = record.clone();
    unsupported.preserve = Some("bad\0text".to_owned());
    assert!(!unsupported.valid(&messages));
}

#[test]
fn an_over_bound_summary_is_rejected() {
    assert_eq!(
        validate_summary(&"x".repeat(super::MAXIMUM_SUMMARY_BYTES + 1), None),
        Err(CompactionError::Bound)
    );
    assert_eq!(
        validate_summary(&"x".repeat(super::MAXIMUM_SUMMARY_BYTES), None),
        Ok("x".repeat(super::MAXIMUM_SUMMARY_BYTES))
    );
}

#[test]
fn project_turns_carries_every_request_and_the_retained_suffix() {
    let turns = vec![
        crate::providers::ChatTurn::user("First".to_owned()),
        crate::providers::ChatTurn::assistant(crate::providers::AssistantReply::from("Reply")),
        crate::providers::ChatTurn::user("Second".to_owned()),
        crate::providers::ChatTurn::assistant(crate::providers::AssistantReply::from("Latest")),
    ];
    let requests = vec![usage(), usage()];
    let projected = super::project_turns(&turns, 1, "Summary", &requests).unwrap();
    assert_eq!(projected.len(), 3);
    assert_eq!(projected[0].usage.len(), 2);
    assert_eq!(projected[1].text, "Second");
    assert_eq!(projected[2].text, "Latest");
}

#[test]
fn opaque_continuation_blocks_an_otherwise_valid_boundary() {
    let target = |label: &str, output: &str| ToolOutput {
        resource: None,
        label: label.to_owned(),
        output: output.to_owned(),
        command: None,
    };
    let mut first = assistant_tool("call-1", Some(target("read", "first")));
    first.continuation = vec![crate::conversations::ContinuationMetadata {
        provider: ProviderKind::Xai,
        model: "grok-4.6".to_owned(),
        reasoning_id: None,
        blocks: vec![crate::conversations::ContinuationBlock::Encrypted {
            data: "opaque".to_owned(),
        }],
    }];
    let messages = vec![
        user("First"),
        first,
        assistant_tool(
            "call-2",
            Some(target("read", &"large result ".repeat(1_000))),
        ),
        assistant_tool("call-3", Some(target("read", "latest"))),
    ];
    assert_eq!(
        select_boundary(&messages, None, Some(&selection()), 100),
        Err(CompactionError::Continuation)
    );
    let turns = crate::conversations::history::project(&messages, Some(&selection())).unwrap();
    assert_eq!(
        super::workflow_cover_index(&turns, Some(&selection()), 100),
        Err(CompactionError::Continuation)
    );
}
