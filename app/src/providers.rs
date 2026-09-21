pub(crate) mod plan;
mod rig;
mod xai_plan;

use std::fmt;
use std::pin::Pin;
use std::time::Duration;

use futures_util::Stream;
use rig_core::completion::{Message, ToolDefinition};
use serde::{Deserialize, Serialize};

pub(crate) const SYNTHETIC_BASE_URL: &str = "https://api.synthetic.new/openai/v1";
pub(crate) const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
pub(crate) const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com";

pub(crate) const MAXIMUM_API_KEY_BYTES: usize = 4_096;
pub(crate) const MAXIMUM_MODEL_BYTES: usize = 256;
pub(crate) const MAXIMUM_FAVOURITES: usize = 50;
pub(crate) const MAXIMUM_PROVIDER_DETAIL_BYTES: usize = 400;

/// Retry count for one provider request. It is separate from tool-round limits.
pub(crate) const MAXIMUM_PROVIDER_RETRY_ATTEMPTS: u32 = 3;
/// Cap on total wait across retries of one provider request.
pub(crate) const MAXIMUM_PROVIDER_RETRY_WAIT: Duration = Duration::from_secs(60);
pub(crate) const DEFAULT_PROVIDER_RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ProviderKind {
    Xai,
    OpenaiCodex,
    Synthetic,
    Openrouter,
    Deepseek,
}

impl ProviderKind {
    pub(crate) const ALL: [Self; 5] = [
        Self::Xai,
        Self::OpenaiCodex,
        Self::Synthetic,
        Self::Openrouter,
        Self::Deepseek,
    ];

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "xai" => Some(Self::Xai),
            "openai-codex" => Some(Self::OpenaiCodex),
            "synthetic" => Some(Self::Synthetic),
            "openrouter" => Some(Self::Openrouter),
            "deepseek" => Some(Self::Deepseek),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Xai => "xai",
            Self::OpenaiCodex => "openai-codex",
            Self::Synthetic => "synthetic",
            Self::Openrouter => "openrouter",
            Self::Deepseek => "deepseek",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Xai => "xAI (Grok)",
            Self::OpenaiCodex => "OpenAI Codex",
            Self::Synthetic => "Synthetic",
            Self::Openrouter => "OpenRouter",
            Self::Deepseek => "DeepSeek",
        }
    }

    pub(crate) fn default_model(self) -> &'static str {
        match self {
            Self::Xai => "grok-4.6",
            Self::OpenaiCodex => "gpt-5.6-sol",
            Self::Synthetic => "hf:moonshotai/Kimi-K3",
            Self::Openrouter => "openai/gpt-4o-mini",
            Self::Deepseek => "deepseek-v4-flash",
        }
    }

    pub(crate) fn supports_plan(self) -> bool {
        matches!(self, Self::Xai | Self::OpenaiCodex)
    }

    pub(crate) fn plan_file_name(self) -> Option<&'static str> {
        match self {
            Self::Xai => Some("xai-auth.json"),
            Self::OpenaiCodex => Some("chatgpt-auth.json"),
            Self::Synthetic | Self::Openrouter | Self::Deepseek => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ThinkingEffort(String);

impl ThinkingEffort {
    pub(crate) fn new(value: String) -> Option<Self> {
        let value = value.trim();
        if value.is_empty()
            || value.len() > 32
            || value.chars().any(char::is_control)
            || value == "default"
        {
            return None;
        }
        Some(Self(value.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn label(&self) -> String {
        if self.0 == "none" {
            return "Off".to_owned();
        }
        let mut characters = self.0.chars();
        match characters.next() {
            Some(first) => first.to_uppercase().chain(characters).collect(),
            None => String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AuthMethod {
    ApiKey,
    Plan,
}

impl AuthMethod {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "api_key" => Some(Self::ApiKey),
            "plan" => Some(Self::Plan),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::Plan => "plan",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ApiKey => "API key",
            Self::Plan => "Plan login",
        }
    }
}

pub(crate) fn api_key_is_bounded(key: &str) -> bool {
    let key = key.trim();
    !key.is_empty()
        && key.len() <= MAXIMUM_API_KEY_BYTES
        && !key.chars().any(|character| character.is_control())
}

pub(crate) fn model_is_bounded(model: &str) -> bool {
    model.trim().len() <= MAXIMUM_MODEL_BYTES
        && !model.chars().any(|character| character.is_control())
}

#[derive(Clone)]
pub(crate) struct SecretString(String);

impl SecretString {
    pub(crate) fn new(value: String) -> Self {
        Self(value.trim().to_owned())
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString(<redacted>)")
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct ModelSelection {
    pub(crate) provider: ProviderKind,
    pub(crate) model: String,
    #[serde(deserialize_with = "crate::storage::required_option")]
    pub(crate) thinking: Option<ThinkingEffort>,
}

impl ModelSelection {
    pub(crate) fn new(
        provider: ProviderKind,
        model: String,
        thinking: Option<ThinkingEffort>,
    ) -> Option<Self> {
        let model = model.trim();
        if model.is_empty()
            || !model_is_bounded(model)
            || thinking.as_ref().is_some_and(|effort| {
                ThinkingEffort::new(effort.as_str().to_owned()).as_ref() != Some(effort)
            })
        {
            return None;
        }
        Some(Self {
            provider,
            model: model.to_owned(),
            thinking,
        })
    }
}

#[derive(Clone)]
pub(crate) struct ProviderConnection {
    pub(crate) kind: ProviderKind,
    pub(crate) auth: AuthMethod,
    pub(crate) api_key: SecretString,
    pub(crate) model: String,
    pub(crate) thinking: Option<ThinkingEffort>,
    pub(crate) plan_file: Option<std::path::PathBuf>,
}

impl ProviderConnection {
    pub(crate) fn with_key(
        kind: ProviderKind,
        key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            auth: AuthMethod::ApiKey,
            api_key: SecretString::new(key.into()),
            model: model.into(),
            thinking: None,
            plan_file: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Role {
    User,
    Assistant,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolOutput {
    pub(crate) label: String,
    pub(crate) output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) command: Option<crate::execution::CommandResult>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum AssistantActivity {
    Response(String),
    Thinking(String),
    Tool(ToolOutput),
    ToolCall {
        id: String,
        name: String,
        #[serde(default)]
        arguments: serde_json::Value,
        result: Option<ToolOutput>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct ModelUsage {
    pub(crate) provider: ProviderKind,
    pub(crate) model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cache_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cache_creation_tokens: Option<u64>,
}

impl ModelUsage {
    pub(crate) fn new(provider: ProviderKind, model: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_creation_tokens: None,
        }
    }

    pub(crate) fn valid(&self) -> bool {
        model_is_bounded(&self.model) && !self.model.contains('\0') && self.cache_sum().is_ok()
    }

    /// Combined cache tokens. `Err` means the reported cache counts overflow.
    pub(crate) fn cache_sum(&self) -> Result<Option<u64>, ()> {
        match (self.cache_read_tokens, self.cache_creation_tokens) {
            (None, None) => Ok(None),
            (Some(left), None) | (None, Some(left)) => Ok(Some(left)),
            (Some(left), Some(right)) => left.checked_add(right).ok_or(()).map(Some),
        }
    }

    pub(crate) fn has_tokens(&self) -> bool {
        self.input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.cache_read_tokens.is_some()
            || self.cache_creation_tokens.is_some()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ToolProgress {
    pub(crate) id: String,
    pub(crate) stream: crate::execution::CommandStream,
    pub(crate) text: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CompletionReason {
    Stop,
    ToolCalls,
    Length,
    Refusal,
    Unknown,
}

impl CompletionReason {
    pub(crate) fn allows_tool_dispatch(self) -> bool {
        matches!(self, Self::ToolCalls)
    }

    pub(crate) fn incomplete(self) -> bool {
        matches!(self, Self::Length | Self::Refusal | Self::Unknown)
    }

    pub(crate) fn status_label(self) -> Option<&'static str> {
        match self {
            Self::Length | Self::Unknown => Some("Incomplete response"),
            Self::Refusal => Some("Response refused"),
            Self::Stop | Self::ToolCalls => None,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct AssistantReply {
    pub(crate) text: String,
    pub(crate) thinking: String,
    pub(crate) tools: Vec<ToolOutput>,
    pub(crate) activity: Vec<AssistantActivity>,
    pub(crate) usage: Vec<crate::conversations::RequestUsage>,
    /// Bounded provider-tagged continuation data that is not visible thought text.
    pub(crate) continuation: Vec<crate::conversations::ContinuationMetadata>,
    /// Transient live output for an active command. It is not durable history.
    pub(crate) progress: Vec<ToolProgress>,
    pub(crate) completion: Option<CompletionReason>,
}

impl AssistantReply {
    pub(crate) fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
            && self.thinking.trim().is_empty()
            && self.tools.is_empty()
            && self.activity.is_empty()
    }

    pub(crate) fn push_response(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.text.push_str(delta);
        match self.activity.last_mut() {
            Some(AssistantActivity::Response(text)) => text.push_str(delta),
            _ => self
                .activity
                .push(AssistantActivity::Response(delta.to_owned())),
        }
    }

    pub(crate) fn push_thinking(&mut self, delta: &str) {
        self.thinking.push_str(delta);
        match self.activity.last_mut() {
            Some(AssistantActivity::Thinking(thinking)) => thinking.push_str(delta),
            _ => self
                .activity
                .push(AssistantActivity::Thinking(delta.to_owned())),
        }
    }

    pub(crate) fn start_tool(&mut self, id: String, name: String, arguments: serde_json::Value) {
        self.activity.push(AssistantActivity::ToolCall {
            id,
            name,
            arguments,
            result: None,
        });
    }

    pub(crate) fn finish_tool(&mut self, id: &str, tool: ToolOutput) {
        self.progress.retain(|progress| progress.id != id);
        if let Some(AssistantActivity::ToolCall { result, .. }) = self.activity.iter_mut().rev().find(|activity| {
            matches!(activity, AssistantActivity::ToolCall { id: call_id, result: None, .. } if call_id == id)
        }) {
            *result = Some(tool.clone());
            self.tools.push(tool);
        } else {
            self.push_tool(tool);
        }
    }

    pub(crate) fn push_tool(&mut self, tool: ToolOutput) {
        self.tools.push(tool.clone());
        self.activity.push(AssistantActivity::Tool(tool));
    }

    pub(crate) fn push_tool_progress(
        &mut self,
        id: &str,
        stream: crate::execution::CommandStream,
        delta: &str,
    ) {
        if delta.is_empty() {
            return;
        }
        if let Some(progress) = self
            .progress
            .last_mut()
            .filter(|progress| progress.id == id && progress.stream == stream)
        {
            progress.text.push_str(delta);
        } else {
            self.progress.push(ToolProgress {
                id: id.to_owned(),
                stream,
                text: delta.to_owned(),
            });
        }
    }
}

impl From<String> for AssistantReply {
    fn from(text: String) -> Self {
        Self {
            text,
            ..Self::default()
        }
    }
}

impl From<&str> for AssistantReply {
    fn from(text: &str) -> Self {
        text.to_owned().into()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct ChatToolCall {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) arguments: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) result: Option<ToolOutput>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct ChatTurn {
    pub(crate) role: Role,
    pub(crate) text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) thinking: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) tools: Vec<ToolOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) activity: Vec<AssistantActivity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) usage: Vec<crate::conversations::RequestUsage>,
    /// Durable assistant tool calls with their matching structured results.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) calls: Vec<ChatToolCall>,
    /// Bounded provider-tagged continuation data. Opaque fields stay out of `text`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) continuation: Vec<crate::conversations::ContinuationMetadata>,
}

impl ChatTurn {
    pub(crate) fn user(text: String) -> Self {
        Self {
            role: Role::User,
            text,
            thinking: String::new(),
            tools: Vec::new(),
            activity: Vec::new(),
            usage: Vec::new(),
            calls: Vec::new(),
            continuation: Vec::new(),
        }
    }

    pub(crate) fn assistant(reply: AssistantReply) -> Self {
        let calls = reply
            .activity
            .iter()
            .filter_map(|activity| match activity {
                AssistantActivity::ToolCall {
                    id,
                    name,
                    arguments,
                    result,
                } => Some(ChatToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                    result: result.clone(),
                }),
                _ => None,
            })
            .collect();
        Self {
            role: Role::Assistant,
            text: reply.text,
            thinking: reply.thinking,
            tools: reply.tools,
            activity: reply.activity,
            usage: reply.usage,
            calls,
            continuation: reply.continuation,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ProviderError {
    Rejected,
    Reauthenticate,
    AccountInactive,
    Refused,
    Unreachable,
    RateLimited {
        retry_after: Option<hypergraft::RetryAfter>,
    },
    EmptyReply,
    ReplyTooLong,
    Incomplete,
    ContextOverflow,
    Detail(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for ProviderError {}

impl ProviderError {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Rejected => "That key was rejected. Check the provider and try again.",
            Self::Reauthenticate => "This plan login expired. Sign in again from Providers.",
            Self::AccountInactive => {
                "This provider account is not active. Check the subscription and try again."
            }
            Self::Refused => "The provider refused this request. Check the account and try again.",
            Self::Unreachable => "The provider could not be reached. Try again.",
            Self::RateLimited { .. } => {
                "The provider rate-limited this request. Try again shortly."
            }
            Self::EmptyReply => "The model returned an empty reply. Try again.",
            Self::ReplyTooLong => {
                "Power Plant truncated the model reply because it was too long. Try again."
            }
            Self::Incomplete => "The model response was incomplete. No tool ran.",
            Self::ContextOverflow => {
                "The provider rejected this request because it exceeded the model context."
            }
            Self::Detail(text) => text,
        }
    }

    pub(crate) fn patch_status(&self) -> hypergraft::PatchStatus {
        match self {
            Self::Rejected => hypergraft::PatchStatus::Unauthorized,
            Self::Reauthenticate => hypergraft::PatchStatus::UnprocessableEntity,
            Self::RateLimited { retry_after } => hypergraft::PatchStatus::TooManyRequests(
                retry_after.unwrap_or_else(default_retry_after),
            ),
            Self::AccountInactive
            | Self::Refused
            | Self::Unreachable
            | Self::EmptyReply
            | Self::ReplyTooLong
            | Self::Incomplete
            | Self::ContextOverflow
            | Self::Detail(_) => hypergraft::PatchStatus::UnprocessableEntity,
        }
    }

    pub(crate) fn retry_eligible(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } | Self::Unreachable | Self::EmptyReply
        )
    }

    pub(crate) fn retry_delay(&self) -> Option<Duration> {
        if !self.retry_eligible() {
            return None;
        }
        match self {
            Self::RateLimited { retry_after } => Some(
                retry_after
                    .map(|delay| Duration::from_secs(delay.as_seconds()))
                    .unwrap_or(DEFAULT_PROVIDER_RETRY_DELAY),
            ),
            Self::Unreachable | Self::EmptyReply => Some(DEFAULT_PROVIDER_RETRY_DELAY),
            _ => None,
        }
    }

    pub(crate) fn bounded_retry_delay(&self, waited: Duration) -> Option<Duration> {
        if waited >= MAXIMUM_PROVIDER_RETRY_WAIT {
            return None;
        }
        let delay = self.retry_delay()?;
        // Do not retry before the provider's requested delay.
        (delay <= MAXIMUM_PROVIDER_RETRY_WAIT - waited).then_some(delay)
    }
}

fn default_retry_after() -> hypergraft::RetryAfter {
    hypergraft::RetryAfter::seconds(1).expect("one second is a valid retry interval")
}

// 400 and 422 mean the empty probe was authenticated. Never treat them as a rejected key.
pub(crate) fn classify_verify_status(
    status: u16,
    retry_after: Option<&str>,
) -> Result<(), ProviderError> {
    match status {
        200..=299 | 400 | 422 => Ok(()),
        other => Err(classify_failure_status(other, retry_after)),
    }
}

pub(crate) fn classify_failure_status(status: u16, retry_after: Option<&str>) -> ProviderError {
    classify_failure_status_for(status, retry_after, AuthMethod::ApiKey)
}

pub(crate) fn classify_failure_status_for(
    status: u16,
    retry_after: Option<&str>,
    auth: AuthMethod,
) -> ProviderError {
    match status {
        401 | 403 if auth == AuthMethod::Plan => ProviderError::Reauthenticate,
        401 | 403 => ProviderError::Rejected,
        // 402 is an inactive or unpaid account. Do not call it unreachable.
        402 => ProviderError::AccountInactive,
        429 => ProviderError::RateLimited {
            retry_after: parse_retry_after(retry_after),
        },
        408 => ProviderError::Unreachable,
        400..=499 => ProviderError::Refused,
        _ => ProviderError::Unreachable,
    }
}

fn parse_retry_after(value: Option<&str>) -> Option<hypergraft::RetryAfter> {
    let seconds = value?.trim().parse().ok()?;
    hypergraft::RetryAfter::seconds(seconds)
}

pub(crate) fn with_provider_detail(error: ProviderError, body: Option<&[u8]>) -> ProviderError {
    with_extracted_detail(error, body.and_then(provider_detail))
}

pub(crate) fn with_json_detail(
    error: ProviderError,
    json: Option<&serde_json::Value>,
) -> ProviderError {
    with_extracted_detail(error, json.and_then(detail_from_json))
}

fn with_extracted_detail(error: ProviderError, detail: Option<String>) -> ProviderError {
    match error {
        ProviderError::Rejected
        | ProviderError::Reauthenticate
        | ProviderError::RateLimited { .. }
        | ProviderError::Unreachable => error,
        other => detail.map(ProviderError::Detail).unwrap_or(other),
    }
}

// Read only a bounded error.message. Never log the provider body.
pub(crate) fn provider_detail(body: &[u8]) -> Option<String> {
    detail_from_json(&serde_json::from_slice(body).ok()?)
}

fn detail_from_json(value: &serde_json::Value) -> Option<String> {
    let text = value
        .pointer("/error/message")
        .and_then(serde_json::Value::as_str)?;
    sanitise_detail(text)
}

pub(crate) fn sanitise_detail(text: &str) -> Option<String> {
    let mut out = String::new();
    for character in text.chars() {
        if character.is_control() {
            continue;
        }
        if out.len().saturating_add(character.len_utf8()) > MAXIMUM_PROVIDER_DETAIL_BYTES {
            break;
        }
        out.push(character);
    }
    let out = out.trim().to_owned();
    if out.is_empty() { None } else { Some(out) }
}

#[derive(Clone, Debug)]
pub(crate) enum ModelEvent {
    Text(String),
    Thinking(String),
    /// Bounded provider-tagged opaque continuation data for the selected provider.
    Continuation(crate::conversations::ContinuationMetadata),
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    Usage {
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        cache_read_tokens: Option<u64>,
        cache_creation_tokens: Option<u64>,
    },
    Complete {
        reason: CompletionReason,
    },
}

pub(crate) type ModelStream = Pin<Box<dyn Stream<Item = Result<ModelEvent, ProviderError>> + Send>>;

pub(crate) enum ChatBackend {
    Rig,
    #[cfg(test)]
    Scripted(crate::tests::ScriptedBackend),
}

impl ChatBackend {
    pub(crate) async fn verify(
        &self,
        connection: &ProviderConnection,
    ) -> Result<(), ProviderError> {
        match self {
            Self::Rig => rig::verify(connection).await,
            #[cfg(test)]
            Self::Scripted(backend) => backend.verify(connection),
        }
    }

    pub(crate) async fn stream_title(
        &self,
        connection: &ProviderConnection,
        prompt: String,
        preamble: &str,
    ) -> Result<ModelStream, ProviderError> {
        let history = [ChatTurn::user(prompt)];
        match self {
            Self::Rig => {
                rig::stream_turn(
                    connection,
                    &history,
                    &[],
                    &[],
                    preamble,
                    Some(crate::models::models_dev::TITLE_OUTPUT_TOKENS),
                )
                .await
            }
            #[cfg(test)]
            Self::Scripted(backend) => {
                backend.stream_turn(connection, &history, &[], &[], preamble, None)
            }
        }
    }

    pub(crate) async fn stream_turn(
        &self,
        connection: &ProviderConnection,
        history: &[ChatTurn],
        extra: &[Message],
        tools: &[ToolDefinition],
        preamble: &str,
        max_tokens: Option<u64>,
    ) -> Result<ModelStream, ProviderError> {
        match self {
            Self::Rig => {
                rig::stream_turn(connection, history, extra, tools, preamble, max_tokens).await
            }
            #[cfg(test)]
            Self::Scripted(backend) => {
                backend.stream_turn(connection, history, extra, tools, preamble, max_tokens)
            }
        }
    }
}

#[cfg(test)]
pub(super) mod tests;
