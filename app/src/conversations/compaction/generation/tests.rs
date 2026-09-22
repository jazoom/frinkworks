use std::cell::RefCell;
use std::sync::Arc;

use crate::config::RuntimeConfig;
use crate::conversations::ConversationId;
use crate::providers::{
    AssistantReply, ChatBackend, ChatTurn, CompletionReason, ModelEvent, ProviderConnection,
    ProviderKind,
};
use crate::sessions::Job;

use super::generate;
use crate::conversations::compaction::{CompactionError, SUMMARY_OUTPUT_TOKENS};

fn state(backend: crate::tests::ScriptedBackend) -> crate::state::AppState {
    let mut state = crate::tests::test_state(RuntimeConfig::development());
    state.chat = Arc::new(ChatBackend::Scripted(backend));
    state
}

fn connection(model: &str) -> ProviderConnection {
    ProviderConnection::with_key(ProviderKind::Xai, "test-key", model)
}

fn chunk_connection() -> ProviderConnection {
    connection_for(ProviderKind::Openrouter, "qwen/qwen-2.5-7b-instruct")
}

fn connection_for(kind: ProviderKind, model: &str) -> ProviderConnection {
    ProviderConnection::with_key(kind, "test-key", model)
}

fn job() -> Arc<Job> {
    Job::for_conversation(
        crate::sessions::JobId::generate().expect("job"),
        ConversationId::generate().expect("conversation"),
    )
}

fn exchange(index: usize, size: usize) -> [ChatTurn; 2] {
    [
        ChatTurn::user(format!("request {index} {}", "x".repeat(size))),
        ChatTurn::assistant(AssistantReply::from(format!("response {index}"))),
    ]
}

fn rounds(replies: impl IntoIterator<Item = &'static str>) -> crate::tests::ScriptedBackend {
    crate::tests::ScriptedBackend::rounds(
        replies
            .into_iter()
            .map(|reply| {
                vec![
                    Ok(ModelEvent::Text(reply.to_owned())),
                    Ok(ModelEvent::Complete {
                        reason: CompletionReason::Stop,
                    }),
                ]
            })
            .collect(),
    )
}

#[tokio::test]
async fn a_small_covered_prefix_uses_one_request() {
    let backend = rounds(["The earlier work is complete."]);
    let state = state(backend.clone());
    let turns = [exchange(0, 200).as_slice(), exchange(1, 200).as_slice()].concat();
    let persisted = RefCell::new(Vec::new());
    let outcome = generate(
        &state,
        &connection("grok-4.6"),
        &turns,
        None,
        &job(),
        |request| {
            persisted.borrow_mut().push(request.clone());
            Ok(())
        },
    )
    .await
    .expect("summary");
    assert_eq!(outcome.text, "The earlier work is complete.");
    assert_eq!(outcome.requests.len(), 1);
    assert_eq!(backend.captured().len(), 1);
    assert_eq!(
        backend.captured()[0].max_tokens,
        Some(SUMMARY_OUTPUT_TOKENS)
    );
}

#[tokio::test]
async fn a_large_prefix_folds_chronological_exchanges_into_a_cumulative_summary() {
    let backend = rounds(["First summary", "Second summary", "Third summary"]);
    let state = state(backend.clone());
    let turns = [
        exchange(0, 60_000).as_slice(),
        exchange(1, 60_000).as_slice(),
        exchange(2, 60_000).as_slice(),
    ]
    .concat();
    let persisted = RefCell::new(Vec::new());
    let outcome = generate(
        &state,
        &chunk_connection(),
        &turns,
        None,
        &job(),
        |request| {
            persisted.borrow_mut().push(request.clone());
            Ok(())
        },
    )
    .await
    .expect("summary");
    assert_eq!(outcome.requests.len(), 3);
    let captured = backend.captured();
    assert_eq!(captured.len(), 3);
    // Each request carries the previous summary and its next chunk exactly once.
    assert!(!captured[0].history[0].text.contains("request 1"));
    assert!(captured[1].history[0].text.contains("First summary"));
    assert!(captured[1].history[0].text.contains("request 1"));
    assert!(captured[2].history[0].text.contains("Second summary"));
    assert!(captured[2].history[0].text.contains("request 2"));
    assert!(!captured[2].history[0].text.contains("request 0"));
    assert_eq!(outcome.text, "Third summary");
    let unique: std::collections::BTreeSet<_> = persisted
        .borrow()
        .iter()
        .map(|request| request.id)
        .collect();
    assert_eq!(unique.len(), 3);
}

#[tokio::test]
async fn a_failed_final_chunk_retains_completed_requests() {
    let backend = crate::tests::ScriptedBackend::rounds(vec![
        vec![
            Ok(ModelEvent::Text("First summary".to_owned())),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::Stop,
            }),
        ],
        vec![
            Ok(ModelEvent::Text("Second summary".to_owned())),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::Stop,
            }),
        ],
        // A length-terminated reply must not become a successful checkpoint.
        vec![
            Ok(ModelEvent::Text("Truncated final summary".to_owned())),
            Ok(ModelEvent::Complete {
                reason: CompletionReason::Length,
            }),
            Ok(ModelEvent::Usage {
                input_tokens: Some(123),
                output_tokens: Some(10_000),
                cache_read_tokens: None,
                cache_creation_tokens: None,
            }),
        ],
    ]);
    let state = state(backend.clone());
    let turns = [
        exchange(0, 60_000).as_slice(),
        exchange(1, 60_000).as_slice(),
        exchange(2, 60_000).as_slice(),
    ]
    .concat();
    let persisted = RefCell::new(Vec::new());
    let outcome = generate(
        &state,
        &chunk_connection(),
        &turns,
        None,
        &job(),
        |request| {
            persisted.borrow_mut().push(request.clone());
            Ok(())
        },
    )
    .await;
    assert!(outcome.is_err());
    // Completed and failed requests are both retained without duplicate ids.
    let unique: std::collections::BTreeSet<_> = persisted
        .borrow()
        .iter()
        .map(|request| request.id)
        .collect();
    assert_eq!(unique.len(), 3);
    let persisted = persisted.borrow();
    let failed = persisted.last().expect("failed request usage");
    assert_eq!(failed.usage.input_tokens, Some(123));
    assert_eq!(failed.usage.output_tokens, Some(10_000));
}

#[tokio::test]
async fn preserve_sits_below_the_fixed_instructions() {
    let backend = rounds(["Summary"]);
    let state = state(backend.clone());
    let turns = exchange(0, 200);
    let outcome = generate(
        &state,
        &connection("grok-4.6"),
        &turns,
        Some("Keep the migration plan and the open question."),
        &job(),
        |_| Ok(()),
    )
    .await
    .expect("summary");
    assert_eq!(outcome.requests.len(), 1);
    let captured = &backend.captured()[0];
    let preserve_at = captured
        .preamble
        .find("Keep the migration plan")
        .expect("preserve");
    assert!(
        captured
            .preamble
            .starts_with("You summarise a coding-agent conversation for later model requests")
    );
    assert!(preserve_at > 0);
    assert!(captured.preamble[..preserve_at].contains("What to preserve:"));
    // The instruction travels in the preamble, not as source content.
    assert!(!captured.history[0].text.contains("Keep the migration plan"));
}

#[tokio::test]
async fn an_indivisible_oversized_exchange_is_reported() {
    let backend = rounds(["unused"]);
    let state = state(backend);
    let turns = exchange(0, 400_000);
    let outcome = generate(
        &state,
        &chunk_connection(),
        &turns,
        None,
        &job(),
        |_| Ok(()),
    )
    .await;
    assert_eq!(outcome.err(), Some(CompactionError::Oversized.message()));
}

#[tokio::test]
async fn cancellation_between_chunks_stops_before_the_next_request() {
    let backend = rounds(["First summary", "unused"]);
    let state = state(backend.clone());
    let turns = [
        exchange(0, 60_000).as_slice(),
        exchange(1, 60_000).as_slice(),
    ]
    .concat();
    let job = job();
    let mut persisted = Vec::new();
    let result = generate(&state, &chunk_connection(), &turns, None, &job, |request| {
        persisted.push(request.clone());
        if persisted.len() == 2 {
            job.request_cancel();
        }
        Ok(())
    })
    .await;
    assert!(result.is_err());
    assert_eq!(backend.captured().len(), 1);
    assert_eq!(persisted.len(), 2);
    assert_eq!(persisted[0].id, persisted[1].id);
}

#[tokio::test]
async fn fragmented_text_within_the_allowance_is_not_rejected() {
    let mut events = vec![Ok(ModelEvent::Text("x".to_owned())); 10_000];
    events.push(Ok(ModelEvent::Complete {
        reason: CompletionReason::Stop,
    }));
    let backend = crate::tests::ScriptedBackend::events(events);
    let state = state(backend);
    let result = generate(
        &state,
        &chunk_connection(),
        &exchange(0, 20),
        None,
        &job(),
        |_| Ok(()),
    )
    .await
    .expect("valid fragmented response");
    assert_eq!(result.text, "x".repeat(10_000));
}

#[tokio::test]
async fn unknown_capacity_never_dispatches_a_summary() {
    let backend = rounds(["unused"]);
    let state = state(backend.clone());
    let outcome = generate(
        &state,
        &connection("uncatalogued-model"),
        &exchange(0, 200),
        None,
        &job(),
        |_| panic!("No request can start without known capacity"),
    )
    .await;
    assert_eq!(
        outcome.err(),
        Some(CompactionError::UnknownCapacity.message())
    );
    assert!(backend.captured().is_empty());
}

#[tokio::test]
async fn request_byte_bounds_split_the_prefix_instead_of_rejecting_it() {
    let backend = rounds(["First summary", "Final summary"]);
    let state = state(backend.clone());
    let turns = [
        exchange(0, 600_000).as_slice(),
        exchange(1, 600_000).as_slice(),
    ]
    .concat();
    let outcome = generate(
        &state,
        &connection("grok-4.6"),
        &turns,
        None,
        &job(),
        |_| Ok(()),
    )
    .await
    .expect("bounded chunks");
    assert_eq!(outcome.text, "Final summary");
    let captured = backend.captured();
    assert_eq!(captured.len(), 2);
    assert!(!captured[0].history[0].text.contains("request 1"));
    assert!(!captured[1].history[0].text.contains("request 0"));
    assert!(captured[1].history[0].text.contains("First summary"));
}

#[tokio::test]
async fn a_known_output_shortfall_is_reported_before_any_request() {
    let backend = rounds(["unused"]);
    let state = state(backend.clone());
    let turns = exchange(0, 200);
    // This model publishes a 4,096-token output limit, below the allowance.
    let outcome = generate(
        &state,
        &connection_for(ProviderKind::Openrouter, "openai/gpt-4"),
        &turns,
        None,
        &job(),
        |_| Ok(()),
    )
    .await;
    assert_eq!(outcome.err(), Some(CompactionError::Output.message()));
    assert!(backend.captured().is_empty());
}
