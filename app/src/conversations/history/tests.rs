use crate::providers::{AssistantActivity, ModelSelection, ProviderKind, ToolOutput};

use super::{
    ContinuationBlock, ContinuationMetadata, ConversationMessage, HistoryError, MessageRole,
    MessageStatus, project, validate_exchange,
};
use crate::conversations::MessageId;

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
        label: "read main.rs".to_owned(),
        output: "fn main() {}".to_owned(),
        command: None,
    }
}

fn assistant(activity: Vec<AssistantActivity>) -> ConversationMessage {
    ConversationMessage {
        id: identifier(),
        role: MessageRole::Assistant,
        text: String::new(),
        activity,
        continuation: Vec::new(),
        status: MessageStatus::Complete,
        error: None,
        request: None,
    }
}

#[test]
fn projection_keeps_tool_exchanges_and_matches_results() {
    let messages = vec![
        ConversationMessage {
            id: identifier(),
            role: MessageRole::User,
            text: "Read the file".to_owned(),
            activity: Vec::new(),
            continuation: Vec::new(),
            status: MessageStatus::Complete,
            error: None,
            request: None,
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
