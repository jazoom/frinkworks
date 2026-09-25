use std::{collections::HashSet, time::Duration};

use base64::Engine;
use futures_util::StreamExt;
use rig_core::{
    client::CompletionClient,
    completion::{
        AssistantContent, CompletionError, CompletionModel, FinishReason, Message, ToolDefinition,
        message::{ImageMediaType, ReasoningContent, ToolResultContent, UserContent},
    },
    providers::{chatgpt, deepseek, openai, openrouter, xai},
    streaming::StreamedAssistantContent,
};

use super::{
    AuthMethod, ChatTurn, CompletionReason, DEEPSEEK_BASE_URL, ModelEvent, ModelStream,
    OPENROUTER_BASE_URL, ProviderConnection, ProviderError, ProviderKind, Role, SYNTHETIC_BASE_URL,
    classify_failure_status_for, classify_verify_status, with_json_detail, with_provider_detail,
    xai_plan,
};

const XAI_BASE_URL: &str = "https://api.x.ai/v1";
const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

const MAXIMUM_PROVIDER_ERROR_BYTES: usize = 4_096;

const PREAMBLE: &str = "You are Frinkworks, a local coding agent. Help the user write, explain and review code. Be direct.";

pub(super) const VERIFY_TIMEOUT: Duration = Duration::from_secs(10);

fn base_url(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Xai => XAI_BASE_URL,
        ProviderKind::OpenaiCodex => OPENAI_BASE_URL,
        ProviderKind::Synthetic => SYNTHETIC_BASE_URL,
        ProviderKind::Openrouter => OPENROUTER_BASE_URL,
        ProviderKind::Deepseek => DEEPSEEK_BASE_URL,
    }
}

pub(super) async fn verify(connection: &ProviderConnection) -> Result<(), ProviderError> {
    match connection.auth {
        AuthMethod::ApiKey => {
            verify_at(
                base_url(connection.kind),
                connection.api_key.expose(),
                VERIFY_TIMEOUT,
            )
            .await
        }
        AuthMethod::Plan => match connection.kind {
            ProviderKind::OpenaiCodex => chatgpt_plan_client(connection, false, PREAMBLE)?
                .authorize()
                .await
                .map_err(|_| ProviderError::Reauthenticate),
            ProviderKind::Xai => xai_plan_token(connection).await.map(|_| ()),
            ProviderKind::Synthetic | ProviderKind::Openrouter | ProviderKind::Deepseek => {
                Err(ProviderError::Refused)
            }
        },
    }
}

// An empty `{}` probe still authenticates and does not spend tokens.
// Read only status, Retry-After, and a bounded error.message. Never log the body.
pub(super) async fn verify_at(
    base_url: &str,
    api_key: &str,
    timeout: Duration,
) -> Result<(), ProviderError> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let request = reqwest::Client::new()
        .post(url)
        .bearer_auth(api_key)
        .header("content-type", "application/json")
        .body("{}")
        .send();
    let response = match tokio::time::timeout(timeout, request).await {
        Ok(Ok(response)) => response,
        Ok(Err(_)) | Err(_) => return Err(ProviderError::Unreachable),
    };
    let status = response.status().as_u16();
    let retry_after = retry_after_value(response.headers());
    match classify_verify_status(status, retry_after) {
        Ok(()) => Ok(()),
        Err(error) => {
            let body = bounded_body(response, MAXIMUM_PROVIDER_ERROR_BYTES).await;
            Err(with_provider_detail(error, body.as_deref()))
        }
    }
}

async fn bounded_body(mut response: reqwest::Response, limit: usize) -> Option<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return None;
    }
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body.len().saturating_add(chunk.len()) > limit {
                    return None;
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => return Some(body),
            Err(_) => return None,
        }
    }
}

pub(super) async fn stream_turn(
    connection: &ProviderConnection,
    history: &[ChatTurn],
    extra: &[Message],
    tools: &[ToolDefinition],
    preamble: &str,
    max_tokens: Option<u64>,
) -> Result<ModelStream, ProviderError> {
    match (connection.kind, connection.auth) {
        (ProviderKind::Xai, AuthMethod::ApiKey) => {
            let client = xai::Client::new(connection.api_key.expose())
                .map_err(|_| ProviderError::Unreachable)?;
            let model = client.completion_model(&connection.model);
            stream_messages(
                model, history, extra, tools, preamble, connection, max_tokens,
            )
            .await
        }
        (ProviderKind::Xai, AuthMethod::Plan) => {
            let token = xai_plan_token(connection).await?;
            let client = openai::Client::builder()
                .api_key(&token)
                .base_url(xai_plan::XAI_PLAN_BASE_URL)
                .http_headers(xai_plan::proxy_headers())
                .build()
                .map_err(|_| ProviderError::Unreachable)?
                .completions_api();
            let model = client.completion_model(&connection.model);
            stream_messages(
                model, history, extra, tools, preamble, connection, max_tokens,
            )
            .await
        }
        (ProviderKind::OpenaiCodex, AuthMethod::ApiKey) => {
            let client = openai::Client::new(connection.api_key.expose())
                .map_err(|_| ProviderError::Unreachable)?;
            let model = client.completion_model(&connection.model);
            stream_messages(
                model, history, extra, tools, preamble, connection, max_tokens,
            )
            .await
        }
        (ProviderKind::OpenaiCodex, AuthMethod::Plan) => {
            let client = chatgpt_plan_client(connection, false, preamble)?;
            let model = client.completion_model(&connection.model);
            stream_messages(
                model, history, extra, tools, preamble, connection, max_tokens,
            )
            .await
        }
        (ProviderKind::Synthetic, _) => {
            let client = openai::Client::builder()
                .api_key(connection.api_key.expose())
                .base_url(SYNTHETIC_BASE_URL)
                .build()
                .map_err(|_| ProviderError::Unreachable)?
                .completions_api();
            let model = client.completion_model(&connection.model);
            stream_messages(
                model, history, extra, tools, preamble, connection, max_tokens,
            )
            .await
        }
        (ProviderKind::Openrouter, _) => {
            let client = openrouter::Client::new(connection.api_key.expose())
                .map_err(|_| ProviderError::Unreachable)?;
            let model = client.completion_model(&connection.model);
            stream_messages(
                model, history, extra, tools, preamble, connection, max_tokens,
            )
            .await
        }
        (ProviderKind::Deepseek, _) => {
            let client = deepseek::Client::new(connection.api_key.expose())
                .map_err(|_| ProviderError::Unreachable)?;
            let model = client.completion_model(&connection.model);
            stream_messages(
                model, history, extra, tools, preamble, connection, max_tokens,
            )
            .await
        }
    }
}

pub(super) fn thinking_parameters(connection: &ProviderConnection) -> Option<serde_json::Value> {
    let effort = connection.thinking.as_ref()?.as_str();
    Some(match (connection.kind, connection.auth) {
        (ProviderKind::Deepseek, _) => serde_json::json!({
            "thinking": {"type": "enabled"},
            "reasoning_effort": effort,
        }),
        (ProviderKind::Synthetic, _) | (ProviderKind::Xai, AuthMethod::Plan) => {
            serde_json::json!({"reasoning_effort": effort})
        }
        (ProviderKind::Xai | ProviderKind::OpenaiCodex, _) | (ProviderKind::Openrouter, _) => {
            serde_json::json!({"reasoning": {"effort": effort}})
        }
    })
}

fn chatgpt_plan_client(
    connection: &ProviderConnection,
    allow_device_flow: bool,
    preamble: &str,
) -> Result<chatgpt::Client, ProviderError> {
    let path = connection
        .plan_file
        .as_ref()
        .ok_or(ProviderError::Reauthenticate)?;
    chatgpt::Client::builder()
        .oauth()
        .auth_file(path)
        .allow_device_flow(allow_device_flow)
        .default_instructions(preamble)
        .build()
        .map_err(|_| ProviderError::Unreachable)
}

async fn xai_plan_token(connection: &ProviderConnection) -> Result<String, ProviderError> {
    let path = connection
        .plan_file
        .as_ref()
        .ok_or(ProviderError::Reauthenticate)?;
    xai_plan::access_token(path).await
}

pub(super) fn classify_completion_for(error: CompletionError, auth: AuthMethod) -> ProviderError {
    let json = error.provider_response_json().ok().flatten();
    if matches!(
        error
            .provider_response_status()
            .map(|status| status.as_u16()),
        None | Some(400 | 413 | 422)
    ) && context_overflow_payload(json.as_ref(), &error.to_string())
    {
        return ProviderError::ContextOverflow;
    }
    let classified = match error
        .provider_response_status()
        .map(|status| status.as_u16())
    {
        Some(code) => classify_failure_status_for(code, retry_after_from_completion(&error), auth),
        None => {
            if auth == AuthMethod::Plan && is_invalid_grant(&error) {
                ProviderError::Reauthenticate
            } else if transient_transport_error(&error) {
                ProviderError::Unreachable
            } else {
                // Untyped provider errors can include refusals and malformed context.
                ProviderError::Incomplete
            }
        }
    };
    with_json_detail(classified, json.as_ref())
}

pub(super) fn context_overflow_payload(json: Option<&serde_json::Value>, text: &str) -> bool {
    let mut haystack = text.to_ascii_lowercase();
    if let Some(json) = json {
        haystack.push(' ');
        haystack.push_str(&json.to_string().to_ascii_lowercase());
    }
    haystack.contains("context_length_exceeded")
        || haystack.contains("context length")
        || haystack.contains("maximum context")
        || haystack.contains("prompt is too long")
        || haystack.contains("too many tokens")
}

fn transient_transport_error(error: &CompletionError) -> bool {
    match error {
        CompletionError::HttpError(rig_core::http_client::Error::StreamEnded) => true,
        CompletionError::HttpError(rig_core::http_client::Error::Instance(error)) => error
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|error| error.is_connect() || error.is_timeout() || error.is_body()),
        _ => false,
    }
}

fn is_invalid_grant(error: &CompletionError) -> bool {
    error
        .provider_response_json()
        .ok()
        .flatten()
        .as_ref()
        .and_then(|value| value.get("error"))
        .and_then(serde_json::Value::as_str)
        == Some("invalid_grant")
}

fn retry_after_value(headers: &reqwest::header::HeaderMap) -> Option<&str> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
}

fn retry_after_from_completion(error: &CompletionError) -> Option<&str> {
    error
        .provider_response_headers()?
        .get("retry-after")?
        .to_str()
        .ok()
}

/// Dispatch and context estimation share this projection to exclude local
/// metadata and duplicate activity from both requests.
pub(crate) fn project_messages(
    kind: ProviderKind,
    model: &str,
    history: &[ChatTurn],
    extra: &[Message],
) -> Result<Vec<Message>, ProviderError> {
    for turn in history {
        if !crate::conversations::history::valid_continuation(&turn.continuation)
            || turn
                .continuation
                .iter()
                .any(|metadata| metadata.provider == kind && metadata.model != model)
        {
            return Err(ProviderError::Detail(
                crate::conversations::history::HistoryError::Continuation
                    .message()
                    .to_owned(),
            ));
        }
        if turn.calls.iter().any(|call| call.result.is_none()) {
            return Err(ProviderError::Detail(
                crate::conversations::history::HistoryError::Unsettled
                    .message()
                    .to_owned(),
            ));
        }
    }
    let mut messages = history
        .iter()
        .flat_map(|turn| match turn.role {
            Role::User => {
                if turn.images.is_empty() {
                    vec![Message::user(turn.text.clone())]
                } else {
                    let mut content = Vec::new();
                    if !turn.text.is_empty() {
                        content.push(UserContent::text(turn.text.clone()));
                    }
                    for image in &turn.images {
                        content.push(UserContent::image_base64(
                            base64::engine::general_purpose::STANDARD.encode(&image.bytes),
                            Some(image_media_type(image.format)),
                            None,
                        ));
                    }
                    vec![Message::User { content }]
                }
            }
            Role::Assistant => {
                let mut content = Vec::new();
                if !turn.text.is_empty() {
                    content.push(AssistantContent::text(turn.text.clone()));
                }
                for metadata in &turn.continuation {
                    if metadata.provider != kind {
                        continue;
                    }
                    let blocks = metadata
                        .blocks
                        .iter()
                        .map(continuation_block)
                        .collect::<Vec<_>>();
                    if !blocks.is_empty() || metadata.reasoning_id.is_some() {
                        content.push(AssistantContent::Reasoning(
                            rig_core::completion::message::Reasoning {
                                id: metadata.reasoning_id.clone(),
                                content: blocks,
                            },
                        ));
                    }
                }
                for call in &turn.calls {
                    content.push(AssistantContent::tool_call(
                        call.id.clone(),
                        call.name.clone(),
                        call.arguments.clone(),
                    ));
                }
                let mut messages = vec![Message::Assistant { id: None, content }];
                for call in &turn.calls {
                    if let Some(result) = &call.result {
                        messages.push(Message::tool_result(
                            call.id.clone(),
                            call.name.clone(),
                            result.output.clone(),
                        ));
                    }
                }
                messages
            }
        })
        .collect::<Vec<_>>();
    messages.extend(extra.iter().cloned());
    Ok(messages)
}

/// Collect the text that a tokenizer can count from a projected request.
/// Visible text, tool arguments and tool-result text contribute. Local usage
/// records and duplicated `activity` never reach the projection. Opaque
/// continuation encodings have no known text token count.
pub(crate) fn projected_token_text(messages: &[Message]) -> String {
    let mut text = String::new();
    for message in messages {
        match message {
            Message::System { content } => push_line(&mut text, content),
            Message::User { content } => {
                for item in content {
                    match item {
                        UserContent::Text(value) => push_line(&mut text, &value.text),
                        UserContent::ToolResult(result) => {
                            for part in &result.content {
                                match part {
                                    ToolResultContent::Text(value) => {
                                        push_line(&mut text, &value.text);
                                    }
                                    ToolResultContent::Json { value } => {
                                        if let Ok(encoded) = serde_json::to_string(value) {
                                            push_line(&mut text, &encoded);
                                        }
                                    }
                                    ToolResultContent::Image(_) => {}
                                }
                            }
                        }
                        UserContent::Image(_)
                        | UserContent::Audio(_)
                        | UserContent::Video(_)
                        | UserContent::Document(_) => {}
                    }
                }
            }
            Message::Assistant { content, .. } => {
                for item in content {
                    match item {
                        AssistantContent::Text(value) => push_line(&mut text, &value.text),
                        AssistantContent::ToolCall(call) => {
                            push_line(&mut text, &call.function.name);
                            if let Ok(arguments) = serde_json::to_string(&call.function.arguments) {
                                push_line(&mut text, &arguments);
                            }
                        }
                        AssistantContent::Reasoning(reasoning) => {
                            for part in &reasoning.content {
                                if let ReasoningContent::Text { text: value, .. } = part {
                                    push_line(&mut text, value);
                                }
                            }
                        }
                        AssistantContent::Image(_) => {}
                    }
                }
            }
        }
    }
    text
}

fn push_line(text: &mut String, value: &str) {
    if value.is_empty() {
        return;
    }
    text.push_str(value);
    text.push('\n');
}

pub(super) async fn stream_messages<M>(
    model: M,
    history: &[ChatTurn],
    extra: &[Message],
    tools: &[ToolDefinition],
    preamble: &str,
    connection: &ProviderConnection,
    max_tokens: Option<u64>,
) -> Result<ModelStream, ProviderError>
where
    M: CompletionModel + Clone,
{
    let mut messages = project_messages(connection.kind, &connection.model, history, extra)?;
    let Some(prompt) = messages.pop() else {
        return Err(ProviderError::EmptyReply);
    };
    let mut request = model
        .completion_request(prompt)
        .preamble(preamble.to_owned())
        .messages(messages);
    // Reasoning selection follows the saved thinking effort only. An output
    // allowance is a capacity bound and never changes the reasoning decision.
    if let Some(parameters) = thinking_parameters(connection) {
        request = request.additional_params(parameters);
    }
    if let Some(limit) = max_tokens {
        request = request.max_tokens(limit);
    }
    if !tools.is_empty() {
        request = request.tools(tools.to_vec());
    }
    let response = request
        .stream()
        .await
        .map_err(|error| classify_completion_for(error, connection.auth))?;
    let auth = connection.auth;
    let provider = connection.kind;
    let model = connection.model.clone();
    let mut reasoning_deltas = HashSet::new();
    Ok(Box::pin(response.flat_map(move |item| {
        let events =
            match item {
                Ok(StreamedAssistantContent::Text(text)) => {
                    vec![Ok(ModelEvent::Text(text.text))]
                }
                Ok(StreamedAssistantContent::ToolCall { tool_call, .. }) => {
                    vec![Ok(ModelEvent::ToolCall {
                        id: tool_call.id.into_string(),
                        name: tool_call.function.name,
                        arguments: tool_call.function.arguments,
                    })]
                }
                Ok(StreamedAssistantContent::ReasoningDelta { id, reasoning, .. }) => {
                    reasoning_deltas.insert(id);
                    vec![Ok(ModelEvent::Thinking(reasoning))]
                }
                Ok(StreamedAssistantContent::Reasoning { reasoning, id }) => {
                    let superseded = reasoning_deltas.contains(&id);
                    let mut text = String::new();
                    let mut blocks = Vec::new();
                    for content in reasoning.content {
                        match content {
                            ReasoningContent::Text {
                                text: value,
                                signature,
                            } => {
                                if let Some(signature) = signature {
                                    blocks.push(crate::conversations::ContinuationBlock::Text {
                                        text: value.clone(),
                                        signature: Some(signature),
                                    });
                                }
                                text.push_str(&value);
                            }
                            ReasoningContent::Summary(value) => text.push_str(&value),
                            ReasoningContent::Encrypted(data) => blocks
                                .push(crate::conversations::ContinuationBlock::Encrypted { data }),
                            ReasoningContent::Redacted { data } => blocks
                                .push(crate::conversations::ContinuationBlock::Redacted { data }),
                        }
                    }
                    let mut events = Vec::new();
                    if !superseded && !text.is_empty() {
                        events.push(Ok(ModelEvent::Thinking(text)));
                    }
                    if !blocks.is_empty() || reasoning.id.is_some() {
                        events.push(Ok(ModelEvent::Continuation(
                            crate::conversations::ContinuationMetadata {
                                provider,
                                model: model.clone(),
                                reasoning_id: reasoning.id,
                                blocks,
                            },
                        )));
                    }
                    events
                }
                Ok(StreamedAssistantContent::Final(final_response)) => {
                    let mut events = Vec::new();
                    if let Some(event) = reported_usage(&final_response) {
                        events.push(Ok(event));
                    }
                    events.push(Ok(ModelEvent::Complete {
                        reason: map_finish_reason(final_response.finish_reason.as_ref()),
                    }));
                    events
                }
                Ok(_) => Vec::new(),
                Err(error) => vec![Err(classify_completion_for(error, auth))],
            };
        futures_util::stream::iter(events)
    })))
}

fn reported_tokens(value: u64) -> Option<u64> {
    (value > 0).then_some(value)
}

/// The raw terminal preserves optional counts that Rig otherwise replaces with
/// zero or derives from totals. Never use those derived totals as reported usage.
pub(super) fn reported_usage(
    final_response: &rig_core::streaming::StreamFinal,
) -> Option<ModelEvent> {
    let usage = &final_response.usage;
    let raw = final_response.raw.get("usage");
    let (input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens) =
        if let Some(raw) = raw {
            // Required wire counters also use zero in Rig's missing-usage sentinel.
            // Optional counters retain presence, including an explicit zero.
            let input = raw
                .get("input_tokens")
                .or_else(|| raw.get("prompt_tokens"))
                .and_then(serde_json::Value::as_u64);
            let output = raw
                .get("output_tokens")
                .or_else(|| raw.get("completion_tokens"))
                .and_then(serde_json::Value::as_u64);
            let details = raw
                .get("input_tokens_details")
                .or_else(|| raw.get("prompt_tokens_details"));
            let cache_read = details
                .and_then(|details| details.get("cached_tokens"))
                .and_then(serde_json::Value::as_u64)
                .or_else(|| {
                    raw.get("prompt_cache_hit_tokens")
                        .and_then(serde_json::Value::as_u64)
                });
            let cache_write = details
                .and_then(|details| details.get("cache_write_tokens"))
                .and_then(serde_json::Value::as_u64);
            let present = usage.input_tokens > 0
                || usage.output_tokens > 0
                || raw
                    .get("completion_tokens")
                    .is_some_and(|value| value.is_u64())
                || details.is_some_and(serde_json::Value::is_object);
            if !present {
                return None;
            }
            (input, output, cache_read, cache_write)
        } else {
            (
                reported_tokens(usage.input_tokens),
                reported_tokens(usage.output_tokens),
                reported_tokens(usage.cached_input_tokens),
                reported_tokens(usage.cache_creation_input_tokens),
            )
        };
    if input_tokens.is_none()
        && output_tokens.is_none()
        && cache_read_tokens.is_none()
        && cache_creation_tokens.is_none()
    {
        return None;
    }
    Some(ModelEvent::Usage {
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_creation_tokens,
    })
}

pub(super) fn map_finish_reason(reason: Option<&FinishReason>) -> CompletionReason {
    match reason {
        Some(FinishReason::Stop) => CompletionReason::Stop,
        Some(FinishReason::ToolCalls) => CompletionReason::ToolCalls,
        Some(FinishReason::Length) => CompletionReason::Length,
        Some(FinishReason::ContentFilter) => CompletionReason::Refusal,
        Some(FinishReason::Other(_)) | None => CompletionReason::Unknown,
    }
}

fn image_media_type(format: crate::conversations::AttachmentFormat) -> ImageMediaType {
    match format {
        crate::conversations::AttachmentFormat::Png => ImageMediaType::PNG,
        crate::conversations::AttachmentFormat::Jpeg => ImageMediaType::JPEG,
        crate::conversations::AttachmentFormat::Webp => ImageMediaType::WEBP,
    }
}

fn continuation_block(block: &crate::conversations::ContinuationBlock) -> ReasoningContent {
    match block {
        crate::conversations::ContinuationBlock::Text { text, signature } => {
            ReasoningContent::Text {
                text: text.clone(),
                signature: signature.clone(),
            }
        }
        crate::conversations::ContinuationBlock::Encrypted { data } => {
            ReasoningContent::Encrypted(data.clone())
        }
        crate::conversations::ContinuationBlock::Redacted { data } => {
            ReasoningContent::Redacted { data: data.clone() }
        }
    }
}
