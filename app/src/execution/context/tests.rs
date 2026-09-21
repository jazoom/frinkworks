use rig_core::completion::ToolDefinition;

use crate::providers::{ChatToolCall, ChatTurn, ToolOutput};

use super::{ContextError, ContextRequest, FALLBACK_CONTEXT_TOKENS, inspect, measure};

fn request<'a>(
    preamble: &'a str,
    tools: &'a [ToolDefinition],
    turns: &'a [ChatTurn],
) -> ContextRequest<'a> {
    ContextRequest {
        preamble,
        tools,
        turns,
        extra: &[],
    }
}

fn user(text: impl Into<String>) -> ChatTurn {
    ChatTurn::user(text.into())
}

fn assistant_with_call(text: &str, id: &str, output: &str) -> ChatTurn {
    let result = ToolOutput {
        label: "read".to_owned(),
        output: output.to_owned(),
        command: None,
    };
    ChatTurn {
        role: crate::providers::Role::Assistant,
        text: text.to_owned(),
        thinking: String::new(),
        tools: vec![result.clone()],
        activity: Vec::new(),
        usage: Vec::new(),
        calls: vec![ChatToolCall {
            id: id.to_owned(),
            name: "read".to_owned(),
            arguments: serde_json::json!({"path": "src/main.rs"}),
            result: Some(result),
        }],
        continuation: Vec::new(),
    }
}

#[test]
fn unknown_catalogue_capacity_uses_the_conservative_fallback() {
    let turns = [user("Hello")];
    let estimate = measure(request("Help.", &[], &turns), None).expect("fits fallback");
    assert_eq!(estimate.limit, FALLBACK_CONTEXT_TOKENS);
    assert!(estimate.fallback_limit());
    assert!(estimate.fits());
    assert!(estimate.output_allowance > 0);
}

#[test]
fn a_known_model_limit_is_enforced() {
    let turns = [user("x".repeat(8_000))];
    let error = measure(request("Help.", &[], &turns), Some(100)).unwrap_err();
    assert_eq!(error, ContextError::Overflow);
    let estimate = inspect(request("Help.", &[], &turns), Some(100)).expect("inspect overflow");
    assert!(!estimate.fits());
    assert_eq!(estimate.catalogue_limit, Some(100));
    assert_eq!(estimate.limit, 100);
}

#[test]
fn null_bytes_are_untrusted_context() {
    let turns = [user("ok")];
    assert_eq!(
        measure(request("bad\0preamble", &[], &turns), Some(8_000)),
        Err(ContextError::Untrusted)
    );
    let turns = [user("bad\0text")];
    assert_eq!(
        measure(request("Help.", &[], &turns), Some(8_000)),
        Err(ContextError::Untrusted)
    );
}

#[test]
fn the_serialised_request_bound_applies_even_to_unlimited_catalogue_values() {
    let mut turns = Vec::new();
    for index in 0..2_000 {
        turns.push(user(format!("block-{index} {}", "n".repeat(2_000))));
    }
    assert_eq!(
        measure(request("Help.", &[], &turns), Some(u64::MAX)),
        Err(ContextError::Bound)
    );
}

#[test]
fn an_orphan_tool_result_cannot_be_dropped_to_fit() {
    let mut turn = assistant_with_call("used read", "call-1", "fn main() {}");
    turn.calls[0].result = None;
    turn.tools.clear();
    let turns = [user("Read it"), turn];
    assert_eq!(
        measure(request("Help.", &[], &turns), Some(8_000)),
        Err(ContextError::Orphan)
    );
}

#[test]
fn a_complete_tool_exchange_counts_toward_capacity() {
    let turns = [
        user("Read src/main.rs"),
        assistant_with_call("Here", "call-1", "fn main() {}"),
        user("Thanks"),
    ];
    measure(request("Help.", &[], &turns), Some(32_768)).expect("fits");
    let mut larger = turns.clone();
    larger[1].calls[0].result.as_mut().unwrap().output = "x".repeat(32_768);
    assert_eq!(
        measure(request("Help.", &[], &larger), Some(32_768)),
        Err(ContextError::Overflow)
    );
}

#[test]
fn compaction_triggers_before_the_request_fills_the_limit() {
    let turns = [user("x".repeat(1_200))];
    let estimate = inspect(request("Help.", &[], &turns), Some(400)).expect("inspect");
    assert!(estimate.needs_compaction(true));
}
