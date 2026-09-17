use std::{collections::HashSet, time::Duration};

use futures_util::StreamExt;
use rig_core::{
    client::CompletionClient,
    completion::{
        CompletionError, CompletionModel, Message, ToolDefinition, message::ReasoningContent,
    },
    providers::{chatgpt, deepseek, openai, openrouter, xai},
    streaming::StreamedAssistantContent,
};

use super::{
    AuthMethod, ChatTurn, DEEPSEEK_BASE_URL, ModelEvent, ModelStream, OPENROUTER_BASE_URL,
    ProviderConnection, ProviderError, ProviderKind, Role, SYNTHETIC_BASE_URL,
    classify_failure_status_for, classify_verify_status, with_json_detail, with_provider_detail,
    xai_plan,
};

const XAI_BASE_URL: &str = "https://api.x.ai/v1";
const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

const MAXIMUM_PROVIDER_ERROR_BYTES: usize = 4_096;

const PREAMBLE: &str = "You are Power Plant, a local coding agent. Help the user write, explain and review code. Be direct.";

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

fn classify_completion_for(error: CompletionError, auth: AuthMethod) -> ProviderError {
    let classified = match error
        .provider_response_status()
        .map(|status| status.as_u16())
    {
        Some(code) => classify_failure_status_for(code, retry_after_from_completion(&error), auth),
        None => {
            if auth == AuthMethod::Plan && is_invalid_grant(&error) {
                ProviderError::Reauthenticate
            } else {
                ProviderError::Unreachable
            }
        }
    };
    let json = error.provider_response_json().ok().flatten();
    with_json_detail(classified, json.as_ref())
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

async fn stream_messages<M>(
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
    let mut messages = history
        .iter()
        .map(|turn| match turn.role {
            Role::User => Message::user(turn.text.clone()),
            Role::Assistant => Message::assistant(turn.text.clone()),
        })
        .collect::<Vec<_>>();
    messages.extend(extra.iter().cloned());
    let Some(prompt) = messages.pop() else {
        return Err(ProviderError::EmptyReply);
    };
    let mut request = model
        .completion_request(prompt)
        .preamble(preamble.to_owned())
        .messages(messages);
    let parameters = if max_tokens.is_some() {
        match connection.kind {
            ProviderKind::Deepseek => Some(serde_json::json!({"thinking": {"type": "disabled"}})),
            ProviderKind::Openrouter => Some(serde_json::json!({"reasoning": {"enabled": false}})),
            _ => thinking_parameters(connection),
        }
    } else {
        thinking_parameters(connection)
    };
    if let Some(parameters) = parameters {
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
    let mut reasoning_deltas = HashSet::new();
    Ok(Box::pin(response.filter_map(move |item| {
        let event = match item {
            Ok(StreamedAssistantContent::Text(text)) => Some(Ok(ModelEvent::Text(text.text))),
            Ok(StreamedAssistantContent::ToolCall { tool_call, .. }) => {
                Some(Ok(ModelEvent::ToolCall {
                    id: tool_call.id.into_string(),
                    name: tool_call.function.name,
                    arguments: tool_call.function.arguments,
                }))
            }
            Ok(StreamedAssistantContent::ReasoningDelta { id, reasoning, .. }) => {
                reasoning_deltas.insert(id);
                Some(Ok(ModelEvent::Thinking(reasoning)))
            }
            Ok(StreamedAssistantContent::Reasoning { reasoning, id }) => {
                if reasoning_deltas.contains(&id) {
                    None
                } else {
                    let text = reasoning
                        .content
                        .into_iter()
                        .filter_map(|content| match content {
                            ReasoningContent::Text { text, .. }
                            | ReasoningContent::Summary(text) => Some(text),
                            ReasoningContent::Encrypted(_) | ReasoningContent::Redacted { .. } => {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    Some(Ok(ModelEvent::Thinking(text)))
                }
            }
            Ok(StreamedAssistantContent::Final(final_response)) => {
                (final_response.usage.input_tokens > 0).then_some(Ok(ModelEvent::Usage {
                    input_tokens: final_response.usage.input_tokens,
                }))
            }
            Ok(_) => None,
            Err(error) => Some(Err(classify_completion_for(error, auth))),
        };
        std::future::ready(event)
    })))
}
