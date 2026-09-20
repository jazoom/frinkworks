use std::time::{Duration, Instant};

use super::{
    MAXIMUM_PROVIDER_DETAIL_BYTES, ProviderConnection, ProviderError, ProviderKind, SecretString,
    ThinkingEffort, classify_failure_status, classify_verify_status, provider_detail,
    rig::{VERIFY_TIMEOUT, map_finish_reason, thinking_parameters, verify_at},
    with_json_detail, with_provider_detail,
};

#[test]
fn thinking_levels_map_to_each_provider_request_shape() {
    let mut connection = ProviderConnection::with_key(ProviderKind::OpenaiCodex, "key", "model");
    assert_eq!(thinking_parameters(&connection), None);

    let off = ThinkingEffort::new("none".to_owned()).unwrap();
    assert_eq!(off.label(), "Off");
    connection.thinking = Some(off);
    assert_eq!(
        thinking_parameters(&connection),
        Some(serde_json::json!({"reasoning": {"effort": "none"}}))
    );

    connection.thinking = Some(ThinkingEffort::new("high".to_owned()).unwrap());
    assert_eq!(
        thinking_parameters(&connection),
        Some(serde_json::json!({"reasoning": {"effort": "high"}}))
    );

    connection.kind = ProviderKind::Synthetic;
    assert_eq!(
        thinking_parameters(&connection),
        Some(serde_json::json!({"reasoning_effort": "high"}))
    );

    connection.kind = ProviderKind::Openrouter;
    assert_eq!(
        thinking_parameters(&connection),
        Some(serde_json::json!({"reasoning": {"effort": "high"}}))
    );

    connection.kind = ProviderKind::Deepseek;
    assert_eq!(
        thinking_parameters(&connection),
        Some(serde_json::json!({"thinking": {"type": "enabled"}, "reasoning_effort": "high"}))
    );
}

#[test]
fn finish_reasons_map_from_provider_data_not_output_text() {
    use super::CompletionReason;
    use rig_core::completion::FinishReason;
    assert_eq!(
        map_finish_reason(Some(&FinishReason::Stop)),
        CompletionReason::Stop
    );
    assert_eq!(
        map_finish_reason(Some(&FinishReason::ToolCalls)),
        CompletionReason::ToolCalls
    );
    assert_eq!(
        map_finish_reason(Some(&FinishReason::Length)),
        CompletionReason::Length
    );
    assert_eq!(
        map_finish_reason(Some(&FinishReason::ContentFilter)),
        CompletionReason::Refusal
    );
    assert_eq!(
        map_finish_reason(Some(&FinishReason::Other("max_tokens".to_owned()))),
        CompletionReason::Unknown
    );
    assert_eq!(map_finish_reason(None), CompletionReason::Unknown);
}

#[test]
fn parses_known_providers() {
    assert_eq!(ProviderKind::parse("xai"), Some(ProviderKind::Xai));
    assert_eq!(
        ProviderKind::parse("openai-codex"),
        Some(ProviderKind::OpenaiCodex)
    );
    assert_eq!(
        ProviderKind::parse("synthetic"),
        Some(ProviderKind::Synthetic)
    );
    assert_eq!(
        ProviderKind::parse("openrouter"),
        Some(ProviderKind::Openrouter)
    );
    assert_eq!(
        ProviderKind::parse("deepseek"),
        Some(ProviderKind::Deepseek)
    );
    assert_eq!(ProviderKind::parse("openai"), None);
}

#[test]
fn secret_debug_is_redacted() {
    let secret = SecretString::new("sk-secret-value".to_owned());
    let debug = format!("{secret:?}");
    assert_eq!(debug, "SecretString(<redacted>)");
    assert!(!debug.contains("sk-secret"));
}

#[test]
fn secret_new_trims_the_key() {
    let secret = SecretString::new("  sk-secret-value\n".to_owned());
    assert_eq!(secret.expose(), "sk-secret-value");
}

#[test]
fn verify_status_treats_auth_failures_as_rejection() {
    assert_eq!(
        classify_verify_status(401, None),
        Err(ProviderError::Rejected)
    );
    assert_eq!(
        classify_verify_status(403, None),
        Err(ProviderError::Rejected)
    );
}

#[test]
fn verify_status_treats_rate_limits_as_rate_limited() {
    assert_eq!(
        classify_verify_status(429, None),
        Err(ProviderError::RateLimited { retry_after: None })
    );
    assert_eq!(
        classify_verify_status(429, Some("15")),
        Err(ProviderError::RateLimited {
            retry_after: hypergraft::RetryAfter::seconds(15),
        })
    );
    assert_eq!(
        classify_verify_status(429, Some(" 8 ")),
        Err(ProviderError::RateLimited {
            retry_after: hypergraft::RetryAfter::seconds(8),
        })
    );
    assert_eq!(
        classify_verify_status(429, Some("0")),
        Err(ProviderError::RateLimited { retry_after: None })
    );
    assert_eq!(
        classify_verify_status(429, Some("Mon, 01 Jan 2020 00:00:00 GMT")),
        Err(ProviderError::RateLimited { retry_after: None })
    );
}

#[test]
fn verify_status_treats_transport_and_server_errors_as_unavailability() {
    assert_eq!(
        classify_verify_status(500, None),
        Err(ProviderError::Unreachable)
    );
    assert_eq!(
        classify_verify_status(502, None),
        Err(ProviderError::Unreachable)
    );
    assert_eq!(
        classify_verify_status(503, None),
        Err(ProviderError::Unreachable)
    );
    assert_eq!(
        classify_verify_status(408, None),
        Err(ProviderError::Unreachable)
    );
}

#[test]
fn verify_status_accepts_success_and_authenticated_request_errors() {
    assert_eq!(classify_verify_status(200, None), Ok(()));
    assert_eq!(classify_verify_status(204, None), Ok(()));
    assert_eq!(classify_verify_status(400, None), Ok(()));
    assert_eq!(classify_verify_status(422, None), Ok(()));
}

#[test]
fn completion_failures_use_the_same_status_families() {
    assert_eq!(classify_failure_status(401, None), ProviderError::Rejected);
    assert_eq!(classify_failure_status(403, None), ProviderError::Rejected);
    assert_eq!(
        super::classify_failure_status_for(401, None, super::AuthMethod::Plan),
        ProviderError::Reauthenticate
    );
    assert_eq!(
        classify_failure_status(429, Some("20")),
        ProviderError::RateLimited {
            retry_after: hypergraft::RetryAfter::seconds(20),
        }
    );
    assert_eq!(
        classify_failure_status(402, None),
        ProviderError::AccountInactive
    );
    assert_eq!(classify_failure_status(404, None), ProviderError::Refused);
    assert_eq!(
        classify_failure_status(500, None),
        ProviderError::Unreachable
    );
}

#[test]
fn each_provider_outcome_maps_to_one_patch_status() {
    assert_eq!(
        ProviderError::Rejected.patch_status(),
        hypergraft::PatchStatus::Unauthorized
    );
    assert_eq!(
        ProviderError::Reauthenticate.patch_status(),
        hypergraft::PatchStatus::UnprocessableEntity
    );
    assert_eq!(
        ProviderError::AccountInactive.patch_status(),
        hypergraft::PatchStatus::UnprocessableEntity
    );
    assert_eq!(
        ProviderError::Refused.patch_status(),
        hypergraft::PatchStatus::UnprocessableEntity
    );
    assert_eq!(
        ProviderError::Unreachable.patch_status(),
        hypergraft::PatchStatus::UnprocessableEntity
    );
    assert_eq!(
        ProviderError::EmptyReply.patch_status(),
        hypergraft::PatchStatus::UnprocessableEntity
    );
    assert_eq!(
        ProviderError::ReplyTooLong.patch_status(),
        hypergraft::PatchStatus::UnprocessableEntity
    );
    assert_eq!(
        ProviderError::RateLimited {
            retry_after: hypergraft::RetryAfter::seconds(12),
        }
        .patch_status(),
        hypergraft::PatchStatus::TooManyRequests(hypergraft::RetryAfter::seconds(12).unwrap())
    );
    assert_eq!(
        ProviderError::RateLimited { retry_after: None }.patch_status(),
        hypergraft::PatchStatus::TooManyRequests(hypergraft::RetryAfter::seconds(1).unwrap())
    );
}

#[test]
fn recovery_copy_does_not_blame_a_valid_key_for_an_outage() {
    assert!(
        ProviderError::Unreachable
            .message()
            .contains("could not be reached")
    );
    assert!(!ProviderError::Unreachable.message().contains("key"));
    assert!(
        ProviderError::Rejected
            .message()
            .contains("key was rejected")
    );
    assert!(
        ProviderError::RateLimited { retry_after: None }
            .message()
            .contains("rate-limited")
    );
    assert!(
        ProviderError::AccountInactive
            .message()
            .contains("subscription")
    );
    assert!(!ProviderError::AccountInactive.message().contains("key"));
    assert!(ProviderError::Refused.message().contains("refused"));
    assert!(!ProviderError::Refused.message().contains("key"));
    assert!(
        ProviderError::Reauthenticate
            .message()
            .contains("plan login")
    );
    assert!(!ProviderError::Reauthenticate.message().contains("key"));
}

#[test]
fn a_provider_json_message_replaces_an_outage_label() {
    let body = serde_json::json!({
        "error": { "message": "You have insufficient credits" }
    })
    .to_string();
    assert_eq!(
        provider_detail(body.as_bytes()).as_deref(),
        Some("You have insufficient credits")
    );
    assert_eq!(
        with_provider_detail(ProviderError::Unreachable, Some(body.as_bytes())),
        ProviderError::Detail("You have insufficient credits".to_owned())
    );
    assert_eq!(
        with_provider_detail(ProviderError::Rejected, Some(body.as_bytes())),
        ProviderError::Rejected
    );
    assert_eq!(provider_detail(b"secret-provider-body-do-not-copy"), None);
    assert_eq!(
        with_json_detail(
            ProviderError::Unreachable,
            Some(&serde_json::json!({
                "message": "top-level-only",
                "error": { "metadata": { "raw": "upstream-only" } }
            })),
        ),
        ProviderError::Unreachable
    );
}

#[test]
fn provider_detail_is_bounded_and_strips_controls() {
    let long = "a".repeat(MAXIMUM_PROVIDER_DETAIL_BYTES + 20);
    let body = serde_json::json!({ "error": { "message": long } }).to_string();
    let detail = provider_detail(body.as_bytes()).expect("detail");
    assert_eq!(detail.len(), MAXIMUM_PROVIDER_DETAIL_BYTES);
    assert_eq!(
        provider_detail(br#"{"error":{"message":"bad\u0000credit"}}"#).as_deref(),
        Some("badcredit")
    );
}

#[tokio::test]
async fn stalled_verification_ends_within_the_configured_timeout() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let Ok((_stream, _)) = listener.accept().await else {
            return;
        };
        std::future::pending::<()>().await;
    });

    let timeout = Duration::from_millis(80);
    let started = Instant::now();
    let result = verify_at(&format!("http://{addr}"), "sk-test", timeout).await;
    let elapsed = started.elapsed();

    assert_eq!(result, Err(ProviderError::Unreachable));
    assert!(elapsed >= timeout, "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    assert_eq!(VERIFY_TIMEOUT, Duration::from_secs(10));
}

#[tokio::test]
async fn verify_preserves_retry_after_and_drops_the_provider_body() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let response = b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 12\r\nContent-Length: 32\r\nConnection: close\r\n\r\nsecret-provider-body-do-not-copy";
        let mut written = 0;
        while written < response.len() {
            if stream.writable().await.is_err() {
                return;
            }
            match stream.try_write(&response[written..]) {
                Ok(0) => return,
                Ok(count) => written += count,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return,
            }
        }
    });

    let result = verify_at(&format!("http://{addr}"), "sk-test", Duration::from_secs(2)).await;
    assert_eq!(
        result,
        Err(ProviderError::RateLimited {
            retry_after: hypergraft::RetryAfter::seconds(12),
        })
    );
    let display = result.expect_err("rate limit");
    assert!(!display.to_string().contains("secret-provider-body"));
}

#[tokio::test]
async fn verify_surfaces_a_bounded_provider_message() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let body =
            r#"{"error":{"message":"You have insufficient credits","secret":"do-not-copy"}}"#;
        let response = format!(
            "HTTP/1.1 402 Payment Required\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let mut written = 0;
        while written < response.len() {
            if stream.writable().await.is_err() {
                return;
            }
            match stream.try_write(&response.as_bytes()[written..]) {
                Ok(0) => return,
                Ok(count) => written += count,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return,
            }
        }
    });

    let result = verify_at(&format!("http://{addr}"), "sk-test", Duration::from_secs(2)).await;
    assert_eq!(
        result,
        Err(ProviderError::Detail(
            "You have insufficient credits".to_owned()
        ))
    );
    let display = result.expect_err("credits");
    assert!(!display.to_string().contains("do-not-copy"));
}

mod scripted_fixture {
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Context, Poll};

    use futures_util::Stream;
    use rig_core::completion::{Message, ToolDefinition};

    use super::super::{
        ChatTurn, CompletionReason, ModelEvent, ModelStream, ProviderConnection, ProviderError,
        ProviderKind, ThinkingEffort,
    };

    #[derive(Clone)]
    enum Script {
        Chunks(Vec<Result<String, ProviderError>>),
        Rounds(Vec<Vec<Result<ModelEvent, ProviderError>>>),
        Hang {
            started: Option<Arc<AtomicBool>>,
            dropped: Option<Arc<AtomicBool>>,
        },
    }

    type CapturedConnection = (ProviderKind, String, Option<ThinkingEffort>);

    #[derive(Clone)]
    pub(crate) struct ScriptedBackend {
        pub(crate) verify_result: Result<(), ProviderError>,
        script: Result<Script, ProviderError>,
        round: Arc<AtomicUsize>,
        last_preamble: Arc<Mutex<Option<String>>>,
        last_tools: Arc<Mutex<Vec<String>>>,
        last_history: Arc<Mutex<Vec<ChatTurn>>>,
        last_connection: Arc<Mutex<Option<CapturedConnection>>>,
    }

    impl ScriptedBackend {
        pub(crate) fn accept() -> Self {
            Self {
                verify_result: Ok(()),
                script: Ok(Script::Chunks(
                    chunk_reply("Hello from Power Plant.")
                        .into_iter()
                        .map(Ok)
                        .collect(),
                )),
                round: Arc::new(AtomicUsize::new(0)),
                last_preamble: Arc::new(Mutex::new(None)),
                last_tools: Arc::new(Mutex::new(Vec::new())),
                last_history: Arc::new(Mutex::new(Vec::new())),
                last_connection: Arc::new(Mutex::new(None)),
            }
        }

        pub(crate) fn chunks<I>(chunks: I) -> Self
        where
            I: IntoIterator<Item = Result<String, ProviderError>>,
        {
            Self {
                verify_result: Ok(()),
                script: Ok(Script::Chunks(chunks.into_iter().collect())),
                round: Arc::new(AtomicUsize::new(0)),
                last_preamble: Arc::new(Mutex::new(None)),
                last_tools: Arc::new(Mutex::new(Vec::new())),
                last_history: Arc::new(Mutex::new(Vec::new())),
                last_connection: Arc::new(Mutex::new(None)),
            }
        }

        pub(crate) fn events(events: Vec<Result<ModelEvent, ProviderError>>) -> Self {
            Self::rounds(vec![events])
        }

        pub(crate) fn rounds(rounds: Vec<Vec<Result<ModelEvent, ProviderError>>>) -> Self {
            let mut backend = Self::accept();
            backend.script = Ok(Script::Rounds(rounds));
            backend
        }

        pub(crate) fn tool_then(name: &str, arguments: serde_json::Value, reply: &str) -> Self {
            Self {
                verify_result: Ok(()),
                script: Ok(Script::Rounds(vec![
                    vec![
                        Ok(ModelEvent::ToolCall {
                            id: "call-1".to_owned(),
                            name: name.to_owned(),
                            arguments,
                        }),
                        Ok(ModelEvent::Complete {
                            reason: CompletionReason::ToolCalls,
                        }),
                    ],
                    {
                        let mut items: Vec<_> = chunk_reply(reply)
                            .into_iter()
                            .map(|text| Ok(ModelEvent::Text(text)))
                            .collect();
                        items.push(Ok(ModelEvent::Complete {
                            reason: CompletionReason::Stop,
                        }));
                        items
                    },
                ])),
                round: Arc::new(AtomicUsize::new(0)),
                last_preamble: Arc::new(Mutex::new(None)),
                last_tools: Arc::new(Mutex::new(Vec::new())),
                last_history: Arc::new(Mutex::new(Vec::new())),
                last_connection: Arc::new(Mutex::new(None)),
            }
        }

        pub(crate) fn hang_watched(started: Arc<AtomicBool>, dropped: Arc<AtomicBool>) -> Self {
            Self {
                verify_result: Ok(()),
                script: Ok(Script::Hang {
                    started: Some(started),
                    dropped: Some(dropped),
                }),
                round: Arc::new(AtomicUsize::new(0)),
                last_preamble: Arc::new(Mutex::new(None)),
                last_tools: Arc::new(Mutex::new(Vec::new())),
                last_history: Arc::new(Mutex::new(Vec::new())),
                last_connection: Arc::new(Mutex::new(None)),
            }
        }

        pub(crate) fn last_preamble(&self) -> Option<String> {
            self.last_preamble
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }

        pub(crate) fn last_tools(&self) -> Vec<String> {
            self.last_tools
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }

        pub(crate) fn last_history(&self) -> Vec<ChatTurn> {
            self.last_history
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }

        pub(crate) fn last_connection(
            &self,
        ) -> Option<(ProviderKind, String, Option<ThinkingEffort>)> {
            self.last_connection
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }

        pub(crate) fn verify(&self, _connection: &ProviderConnection) -> Result<(), ProviderError> {
            self.verify_result.clone()
        }

        pub(crate) fn stream_turn(
            &self,
            connection: &ProviderConnection,
            history: &[ChatTurn],
            _extra: &[Message],
            tools: &[ToolDefinition],
            preamble: &str,
        ) -> Result<ModelStream, ProviderError> {
            *self
                .last_connection
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((
                connection.kind,
                connection.model.clone(),
                connection.thinking.clone(),
            ));
            *self
                .last_preamble
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(preamble.to_owned());
            *self
                .last_history
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = history.to_vec();
            *self
                .last_tools
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                tools.iter().map(|tool| tool.name.clone()).collect();
            match self.script.clone() {
                Ok(Script::Chunks(items)) => {
                    let failed = items.iter().any(Result::is_err);
                    let mut events: Vec<_> = items
                        .into_iter()
                        .map(|item| item.map(ModelEvent::Text))
                        .collect();
                    if !failed {
                        events.push(Ok(ModelEvent::Complete {
                            reason: CompletionReason::Stop,
                        }));
                    }
                    Ok(Box::pin(futures_util::stream::iter(events)))
                }
                Ok(Script::Rounds(rounds)) => {
                    let index = self.round.fetch_add(1, Ordering::SeqCst);
                    let items = rounds.into_iter().nth(index).unwrap_or_default();
                    Ok(Box::pin(futures_util::stream::iter(items)))
                }
                Ok(Script::Hang { started, dropped }) => {
                    Ok(Box::pin(ModelHangStream { started, dropped }))
                }
                Err(error) => Err(error),
            }
        }
    }

    struct ModelHangStream {
        started: Option<Arc<AtomicBool>>,
        dropped: Option<Arc<AtomicBool>>,
    }

    impl Stream for ModelHangStream {
        type Item = Result<ModelEvent, ProviderError>;

        fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            if let Some(started) = &self.started {
                started.store(true, Ordering::SeqCst);
            }
            Poll::Pending
        }
    }

    impl Drop for ModelHangStream {
        fn drop(&mut self) {
            if let Some(dropped) = &self.dropped {
                dropped.store(true, Ordering::SeqCst);
            }
        }
    }

    fn chunk_reply(text: &str) -> Vec<String> {
        if text.chars().count() <= 12 {
            return vec![text.to_owned()];
        }
        let mid = text.chars().count() / 2;
        let mut chars = text.chars();
        let first: String = chars.by_ref().take(mid).collect();
        let second: String = chars.collect();
        vec![first, second]
    }
}

pub(crate) use scripted_fixture::ScriptedBackend;

#[test]
fn assistant_turn_carries_call_arguments_and_matching_results() {
    use crate::providers::{AssistantReply, ChatTurn, ToolOutput};

    let mut reply = AssistantReply::default();
    reply.start_tool(
        "call-1".to_owned(),
        "read".to_owned(),
        serde_json::json!({"path": "src/main.rs"}),
    );
    reply.finish_tool(
        "call-1",
        ToolOutput {
            label: "read src/main.rs".to_owned(),
            output: "fn main() {}".to_owned(),
            command: None,
        },
    );
    let turn = ChatTurn::assistant(reply);
    assert_eq!(turn.calls.len(), 1);
    assert_eq!(
        turn.calls[0].arguments,
        serde_json::json!({"path": "src/main.rs"})
    );
    assert!(turn.calls[0].result.is_some());
    assert!(turn.continuation.is_empty());
}

#[test]
fn continuation_metadata_is_rejected_when_unbounded() {
    use crate::conversations::{ContinuationBlock, ContinuationMetadata};

    let metadata = ContinuationMetadata {
        provider: ProviderKind::Deepseek,
        model: "deepseek-v4-flash".to_owned(),
        reasoning_id: None,
        blocks: vec![ContinuationBlock::Encrypted {
            data: "x".repeat(crate::conversations::history::MAXIMUM_CONTINUATION_BYTES + 1),
        }],
    };
    assert!(!crate::conversations::history::valid_continuation(&[
        metadata
    ]));
}
