use rig_core::completion::ToolDefinition;

use crate::providers::{ChatToolCall, ChatTurn, ProviderKind, ToolOutput};

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
        provider: ProviderKind::Xai,
        model: "uncatalogued-model",
        output_limit: None,
    }
}

fn user(text: impl Into<String>) -> ChatTurn {
    ChatTurn::user(text.into())
}

fn assistant_with_call(text: &str, id: &str, output: &str) -> ChatTurn {
    let result = ToolOutput {
        resource: None,
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
    assert!(estimate.unknown_capacity());
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
fn supported_models_use_a_tokenizer_and_other_models_use_an_approximation() {
    let turns = [user("Hello, world. こんにちは世界。")];
    let supported = ContextRequest {
        model: "gpt-4o",
        ..request("Help.", &[], &turns)
    };
    let estimate = inspect(supported, Some(128_000)).expect("inspect");
    assert!(!estimate.approximate());

    let vendor = ContextRequest {
        model: "openai/gpt-4o",
        ..request("Help.", &[], &turns)
    };
    let estimate = inspect(vendor, Some(128_000)).expect("inspect");
    assert!(!estimate.approximate());

    let fallback = inspect(request("Help.", &[], &turns), Some(128_000)).expect("inspect");
    assert!(fallback.approximate());
}

#[test]
fn token_counts_are_not_serialised_byte_lengths() {
    let turns = [user("alpha beta gamma delta epsilon")];
    let estimate = measure(request("Help.", &[], &turns), Some(8_000)).expect("fits");
    let text = "alpha beta gamma delta epsilon";
    assert!(estimate.input_tokens < text.len() as u64);
}

#[test]
fn a_known_output_limit_caps_the_allowance_and_unknown_stays_operational() {
    let turns = [user("Hello")];
    let known = ContextRequest {
        output_limit: Some(512),
        ..request("Help.", &[], &turns)
    };
    let estimate = inspect(known, Some(128_000)).expect("inspect");
    assert_eq!(estimate.output_allowance, 512);

    let unknown = inspect(request("Help.", &[], &turns), Some(128_000)).expect("inspect");
    assert!(unknown.output_allowance > 0);
    assert!(unknown.output_allowance < 128_000);
}

#[test]
fn usage_never_carries_over_to_a_new_request() {
    use crate::conversations::RequestUsage;
    use crate::providers::ModelUsage;
    let mut turns = [user("x".repeat(400))];
    let original = inspect(request("Help.", &[], &turns), Some(128_000)).unwrap();
    let maximum_capacity = inspect(request("Help.", &[], &turns), Some(u64::MAX)).unwrap();
    let measured = maximum_capacity.with_measured_input(u64::MAX);
    assert!(!measured.fits());
    assert!(measured.needs_compaction(true));
    let mut usage = ModelUsage::new(ProviderKind::Xai, "another-model");
    usage.input_tokens = Some(u64::MAX);
    turns[0].usage.push(RequestUsage {
        usage,
        id: crate::conversations::RequestId::generate().unwrap(),
        auth: crate::providers::AuthMethod::ApiKey,
        prices: None,
        sources: Vec::new(),
        advertised: Vec::new(),
    });
    let next = inspect(request("Help.", &[], &turns), Some(128_000)).unwrap();
    assert_eq!(next, original);
    assert!(!next.measured);
}

#[test]
fn unknown_capacity_never_uses_a_percentage_trigger() {
    let turns = [user("x".repeat(100_000))];
    for capacity in [None, Some(0)] {
        let estimate = inspect(request("Help.", &[], &turns), capacity).unwrap();
        assert!(estimate.unknown_capacity());
        assert!(estimate.fits());
        assert!(!estimate.needs_compaction(true));
    }
}

#[test]
fn small_output_limits_do_not_reserve_impossible_headroom() {
    let turns = [user("x".repeat(3_400))];
    let request = ContextRequest {
        output_limit: Some(64),
        ..request("Help.", &[], &turns)
    };
    let estimate = measure(request, Some(1_000)).unwrap();
    assert_eq!(estimate.reserved_output_tokens, 64);
    assert_eq!(estimate.output_allowance, 64);
}

#[test]
fn stale_continuation_from_another_model_is_rejected() {
    use crate::conversations::{ContinuationBlock, ContinuationMetadata};
    use crate::providers::Role;
    let mut turn = user("Hello");
    turn.role = Role::Assistant;
    turn.continuation.push(ContinuationMetadata {
        provider: ProviderKind::Xai,
        model: "grok-3".to_owned(),
        reasoning_id: None,
        blocks: vec![ContinuationBlock::Encrypted {
            data: "opaque".to_owned(),
        }],
    });
    let turns = [turn];
    let stale = ContextRequest {
        model: "grok-4.6",
        ..request("Help.", &[], &turns)
    };
    assert_eq!(
        measure(stale, Some(128_000)),
        Err(ContextError::Continuation)
    );
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
    larger[1].calls[0].result.as_mut().unwrap().output = "x".repeat(200_000);
    assert_eq!(
        measure(request("Help.", &[], &larger), Some(32_768)),
        Err(ContextError::Overflow)
    );
}

#[test]
fn tool_declarations_contribute_to_input_tokens_once() {
    let turns = [user("Hello")];
    let without = inspect(request("Help.", &[], &turns), Some(128_000)).expect("inspect");
    let tools = [ToolDefinition {
        name: "read".to_owned(),
        description: "Read a file from an approved root.".to_owned(),
        parameters: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
    }];
    let with = inspect(request("Help.", &tools, &turns), Some(128_000)).expect("inspect");
    assert!(with.input_tokens > without.input_tokens);
}

#[test]
fn compaction_triggers_before_the_request_fills_the_limit() {
    let turns = [user("x".repeat(1_200))];
    let estimate = inspect(request("Help.", &[], &turns), Some(400)).expect("inspect");
    assert!(estimate.needs_compaction(true));
}

#[test]
fn resource_text_stays_below_explicit_and_server_instructions() {
    use crate::execution::resources::{
        InstructionSource, ResourceKind, ResourceSource, SkillAdvertisement,
    };
    let instructions = [
        InstructionSource::new("project", "/project/AGENTS.md", "project rule".to_owned()),
        InstructionSource::new(
            "second",
            "/access/second/AGENTS.md",
            "second rule".to_owned(),
        ),
    ];
    let skills = [SkillAdvertisement {
        source: ResourceSource::new(
            ResourceKind::Skill,
            "project",
            "/project/.agents/skills/alpha/SKILL.md",
            b"meta",
        ),
        name: "alpha".to_owned(),
        description: "alpha work".to_owned(),
        read_path: "/project/.agents/skills/alpha/SKILL.md".to_owned(),
    }];
    let composed = super::compose_resources("Server boundary.", &instructions, &skills);
    let server = composed.text.find("Server boundary.").expect("server");
    let project = composed.text.find("project rule").expect("project");
    let second = composed.text.find("second rule").expect("second");
    let skill = composed.text.find("alpha work").expect("skill");
    assert!(server < project && project < second && second < skill);
    assert_eq!(composed.sources.len(), 2);
    assert_eq!(composed.advertised.len(), 1);
}
