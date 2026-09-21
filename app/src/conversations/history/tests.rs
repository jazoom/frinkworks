use crate::providers::{
    AssistantActivity, AuthMethod, ModelSelection, ModelUsage, ProviderKind, ToolOutput,
};

use super::{
    ContinuationBlock, ContinuationMetadata, ConversationMessage, HistoryError, MessageRole,
    MessageStatus, PriceProvenance, RequestUsage, project, request_cost, total_cost,
    validate_exchange,
};
use crate::conversations::{MessageId, RequestId};

fn identifier() -> MessageId {
    MessageId::generate().expect("message id")
}

fn tool_call(id: &str, result: Option<ToolOutput>) -> AssistantActivity {
    AssistantActivity::ToolCall {
        id: id.to_owned(),
        name: "read".to_owned(),
        arguments: serde_json::json!({"path": "main.rs"}),
        result,
    }
}

fn output() -> ToolOutput {
    ToolOutput {
        resource: None,
        label: "read main.rs".to_owned(),
        output: "fn main() {}".to_owned(),
        command: None,
    }
}

fn assistant(activity: Vec<AssistantActivity>) -> ConversationMessage {
    ConversationMessage {
        parent: None,
        id: identifier(),
        role: MessageRole::Assistant,
        text: String::new(),
        activity,
        continuation: Vec::new(),
        status: MessageStatus::Complete,
        error: None,
        request: None,
        completion: None,
        requests: Vec::new(),
    }
}

#[test]
fn projection_keeps_tool_exchanges_and_matches_results() {
    let messages = vec![
        ConversationMessage {
            parent: None,
            id: identifier(),
            role: MessageRole::User,
            text: "Read the file".to_owned(),
            activity: Vec::new(),
            continuation: Vec::new(),
            status: MessageStatus::Complete,
            error: None,
            request: None,
            completion: None,
            requests: Vec::new(),
        },
        assistant(vec![tool_call("call-1", Some(output()))]),
    ];
    let history = project(&messages, None).expect("valid history");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].text, "Read the file");
    assert_eq!(history[1].calls.len(), 1);
    assert_eq!(history[1].calls[0].id, "call-1");
    assert!(history[1].calls[0].result.is_some());
    assert_eq!(history[1].tools.len(), 1);
}

#[test]
fn malformed_call_is_rejected() {
    let mut call = tool_call("call-1", None);
    if let AssistantActivity::ToolCall { id, .. } = &mut call {
        id.clear();
    }
    assert_eq!(
        project(&[assistant(vec![call])], None),
        Err(HistoryError::MalformedCall)
    );
}

#[test]
fn duplicate_identifier_is_rejected_across_messages() {
    let messages = vec![
        assistant(vec![tool_call("call-1", None)]),
        assistant(vec![tool_call("call-1", None)]),
    ];
    assert_eq!(
        validate_exchange(&messages),
        Err(HistoryError::DuplicateIdentifier)
    );
}

#[test]
fn orphan_result_is_rejected() {
    let standalone = AssistantActivity::Tool(ToolOutput {
        resource: None,
        label: "read main.rs".to_owned(),
        output: "orphaned".to_owned(),
        command: None,
    });
    assert_eq!(
        project(&[assistant(vec![standalone])], None),
        Err(HistoryError::OrphanResult)
    );
}

#[test]
fn incomplete_tool_outcomes_block_continuation() {
    for status in [
        MessageStatus::Complete,
        MessageStatus::Failed,
        MessageStatus::Interrupted,
    ] {
        let mut message = assistant(vec![tool_call("call-1", None)]);
        message.status = status;
        assert_eq!(project(&[message], None), Err(HistoryError::Unsettled));
    }
}

#[test]
fn interrupted_questions_settle_without_empty_success() {
    let call = AssistantActivity::ToolCall {
        id: "call-1".to_owned(),
        name: crate::conversations::questions::ASK_USER.to_owned(),
        arguments: serde_json::json!({"question": "Which path?"}),
        result: None,
    };
    let mut message = assistant(vec![call]);
    message.status = MessageStatus::Interrupted;
    super::settle_interrupted_questions(&mut message);
    let history = project(&[message], None).expect("settled question");
    let result = history[0].calls[0].result.as_ref().expect("result");
    assert_eq!(
        result.output,
        crate::conversations::questions::INTERRUPTED_RESULT
    );
    assert_ne!(result.output, "");
}

#[test]
fn failed_command_results_stay_on_the_call() {
    use crate::execution::command::{CommandResult, CommandTermination};
    let command = CommandResult::new(
        vec![crate::execution::command::CommandChunk {
            stream: crate::execution::CommandStream::Stderr,
            text: "partial failure output".to_owned(),
        }],
        CommandTermination::Exited(1),
    );
    let result = ToolOutput {
        resource: None,
        label: "run".to_owned(),
        output: "exit 1".to_owned(),
        command: Some(command),
    };
    let mut message = assistant(vec![tool_call("call-1", Some(result.clone()))]);
    message.status = MessageStatus::Failed;
    let history = project(&[message], None).expect("partial command remains");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].calls[0].result.as_ref(), Some(&result));
    assert!(history[0].text.is_empty());
}

#[test]
fn unknown_command_results_block_continuation() {
    use crate::execution::command::{CommandResult, CommandTermination};
    let command = CommandResult::new(Vec::new(), CommandTermination::Unknown);
    let result = ToolOutput {
        resource: None,
        label: "run".to_owned(),
        output: "unknown".to_owned(),
        command: Some(command),
    };
    let mut message = assistant(vec![tool_call("call-1", Some(result))]);
    message.status = MessageStatus::Failed;
    assert_eq!(project(&[message], None), Err(HistoryError::Unsettled));
}

#[test]
fn failed_prose_stays_local() {
    let mut message = assistant(Vec::new());
    message.text = "Incomplete answer".to_owned();
    message.status = MessageStatus::Failed;
    assert!(project(&[message], None).unwrap().is_empty());
}

#[test]
fn argument_bytes_count_towards_the_aggregate_bound() {
    let calls = (0..8)
        .map(|index| AssistantActivity::ToolCall {
            id: format!("call-{index}"),
            name: "write".to_owned(),
            arguments: serde_json::json!({"content": "x".repeat(40 * 1024)}),
            result: Some(output()),
        })
        .collect();
    assert_eq!(project(&[assistant(calls)], None), Err(HistoryError::Bound));
}

#[test]
fn size_bound_is_enforced() {
    let oversized = "x".repeat(super::MAXIMUM_ACTIVITY_BYTES + 1);
    assert_eq!(
        project(
            &[assistant(vec![AssistantActivity::Thinking(oversized)])],
            None
        ),
        Err(HistoryError::Bound)
    );
}

#[test]
fn continuation_is_dropped_across_providers() {
    let metadata = ContinuationMetadata {
        provider: ProviderKind::Deepseek,
        model: "deepseek-v4-flash".to_owned(),
        reasoning_id: None,
        blocks: vec![ContinuationBlock::Encrypted {
            data: "opaque".to_owned(),
        }],
    };
    let mut message = assistant(vec![tool_call("call-1", Some(output()))]);
    message.continuation = vec![metadata.clone()];
    let same = ModelSelection::new(ProviderKind::Deepseek, "deepseek-v4-flash".to_owned(), None)
        .expect("selection");
    let smaller =
        ModelSelection::new(ProviderKind::Deepseek, "different-model".to_owned(), None).unwrap();
    assert_eq!(
        project(std::slice::from_ref(&message), Some(&smaller)),
        Err(HistoryError::Continuation)
    );
    let changed = ModelSelection::new(
        ProviderKind::Openrouter,
        "openai/gpt-4o-mini".to_owned(),
        None,
    )
    .expect("selection");
    assert_eq!(
        project(std::slice::from_ref(&message), Some(&same)).expect("same provider")[0]
            .continuation,
        vec![metadata]
    );
    assert!(
        project(&[message], Some(&changed)).expect("portable fields survive")[0]
            .continuation
            .is_empty()
    );
}

#[test]
fn invalid_continuation_metadata_is_rejected() {
    let mut message = assistant(vec![tool_call("call-1", Some(output()))]);
    message.continuation = vec![ContinuationMetadata {
        provider: ProviderKind::Deepseek,
        model: "deepseek-v4-flash".to_owned(),
        reasoning_id: None,
        blocks: vec![ContinuationBlock::Encrypted {
            data: "x".repeat(super::MAXIMUM_CONTINUATION_BYTES + 1),
        }],
    }];
    let selection =
        ModelSelection::new(ProviderKind::Deepseek, "deepseek-v4-flash".to_owned(), None)
            .expect("selection");
    assert_eq!(
        project(&[message], Some(&selection)),
        Err(HistoryError::Continuation)
    );
}

fn request(id: RequestId, usage: ModelUsage) -> RequestUsage {
    RequestUsage {
        id,
        usage,
        auth: AuthMethod::ApiKey,
        prices: Some(PriceProvenance {
            source: "models.dev".to_owned(),
            input: Some(1_000_000),
            output: Some(2_000_000),
            cache_read: Some(100_000),
            cache_write: Some(1_250_000),
        }),
        sources: Vec::new(),
        advertised: Vec::new(),
    }
}

#[test]
fn duplicate_request_identities_are_rejected() {
    let id = RequestId::generate().expect("request");
    let mut usage = ModelUsage::new(ProviderKind::Xai, "grok-4");
    usage.input_tokens = Some(10);
    let mut message = assistant(Vec::new());
    message.requests = vec![request(id, usage.clone()), request(id, usage)];
    assert_eq!(validate_exchange(&[message]), Err(HistoryError::Usage));
}

#[test]
fn request_identity_cannot_appear_in_two_messages() {
    let recorded = request(
        RequestId::generate().expect("request"),
        ModelUsage::new(ProviderKind::Xai, "grok-4"),
    );
    let mut first = assistant(Vec::new());
    first.requests.push(recorded.clone());
    let mut second = assistant(Vec::new());
    second.requests.push(recorded);
    assert_eq!(
        validate_exchange(&[first, second]),
        Err(HistoryError::Usage)
    );
}

#[test]
fn partial_usage_never_produces_a_complete_cost() {
    let mut usage = ModelUsage::new(ProviderKind::Xai, "grok-4");
    usage.input_tokens = Some(100);
    let recorded = request(RequestId::generate().expect("request"), usage);
    let cost = request_cost(&recorded);
    assert_eq!(cost.known_micros, Some(100));
    assert!(cost.incomplete);
}

#[test]
fn malformed_cache_overflow_is_rejected() {
    let mut usage = ModelUsage::new(ProviderKind::Xai, "grok-4");
    usage.cache_read_tokens = Some(u64::MAX);
    usage.cache_creation_tokens = Some(3);
    let mut message = assistant(Vec::new());
    message.requests = vec![request(RequestId::generate().expect("request"), usage)];
    assert_eq!(validate_exchange(&[message]), Err(HistoryError::Usage));
}

#[test]
fn cost_does_not_double_count_cache_tokens_included_in_input() {
    let mut usage = ModelUsage::new(ProviderKind::Xai, "grok-4");
    usage.input_tokens = Some(100);
    usage.output_tokens = Some(10);
    usage.cache_read_tokens = Some(40);
    usage.cache_creation_tokens = Some(10);
    let recorded = request(RequestId::generate().expect("request"), usage);
    let cost = request_cost(&recorded);
    assert!(!cost.incomplete);
    // Uncached 50 * $1 + cache read 40 * $0.10 + cache write 10 * $1.25 + output 10 * $2
    // Prices are micros per million tokens: 1_000_000 means $1 / MTok.
    assert_eq!(cost.known_micros, Some(50 + 4 + 13 + 20));
}

#[test]
fn plan_authentication_does_not_use_api_prices() {
    let mut usage = ModelUsage::new(ProviderKind::Xai, "grok-4");
    usage.input_tokens = Some(100);
    usage.output_tokens = Some(10);
    let mut recorded = request(RequestId::generate().expect("request"), usage);
    recorded.auth = AuthMethod::Plan;
    let cost = request_cost(&recorded);
    assert!(cost.incomplete);
    assert_eq!(cost.known_micros, None);
}

#[test]
fn totals_stay_incomplete_when_a_request_lacks_usage() {
    let mut known = ModelUsage::new(ProviderKind::Xai, "grok-4");
    known.input_tokens = Some(1_000_000);
    let unknown = ModelUsage::new(ProviderKind::OpenaiCodex, "gpt-5");
    let total = total_cost(&[
        request(RequestId::generate().expect("request"), known),
        request(RequestId::generate().expect("request"), unknown),
    ]);
    assert!(total.incomplete);
    assert_eq!(total.known_micros, Some(1_000_000));
}

#[test]
fn cost_overflow_does_not_wrap_to_zero() {
    let mut usage = ModelUsage::new(ProviderKind::Xai, "grok-4");
    usage.input_tokens = Some(u64::MAX);
    let mut recorded = request(RequestId::generate().expect("request"), usage);
    recorded.prices = Some(PriceProvenance {
        source: "models.dev".to_owned(),
        input: Some(u64::MAX),
        output: None,
        cache_read: None,
        cache_write: None,
    });
    let cost = request_cost(&recorded);
    assert!(cost.incomplete);
    assert_eq!(cost.known_micros, None);
}
