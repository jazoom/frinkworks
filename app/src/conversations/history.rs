//! Durable, provider-independent conversation history.
//!
//! The transcript and the model projection are separate projections of these
//! entries. A tool result always identifies the call that produced it, and
//! opaque provider continuation data stays separate from visible text.

use serde::{Deserialize, Serialize};

use crate::execution::BudgetSnapshot;
use crate::providers::{
    AssistantActivity, AuthMethod, ChatToolCall, ChatTurn, CompletionReason, ModelSelection,
    ModelUsage, ToolOutput,
};
use crate::sessions::JobId;
use crate::workflows::{AttemptId, RunId};

use super::id::{CheckpointId, MessageId, RequestId};

pub(crate) const MAXIMUM_ACTIVITY_ITEMS: usize = 256;
pub(crate) const MAXIMUM_ACTIVITY_BYTES: usize = 256 * 1024;
pub(crate) const MAXIMUM_CALL_IDENTIFIER_BYTES: usize = 512;
const MAXIMUM_ARGUMENT_BYTES: usize = 64 * 1024;
pub(crate) const MAXIMUM_CONTINUATION_BLOCKS: usize = 64;
pub(crate) const MAXIMUM_CONTINUATION_BYTES: usize = 256 * 1024;
pub(crate) const MAXIMUM_REQUEST_USAGE: usize = 256;
const PRICE_SOURCE_MODELS_DEV: &str = "models.dev";
const MICROS_PER_MILLION: u64 = 1_000_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MessageRole {
    User,
    Assistant,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MessageStatus {
    Complete,
    Pending,
    Interrupted,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ConversationMessage {
    pub(crate) id: MessageId,
    pub(crate) role: MessageRole,
    pub(crate) text: String,
    pub(crate) activity: Vec<AssistantActivity>,
    pub(crate) continuation: Vec<ContinuationMetadata>,
    pub(crate) status: MessageStatus,
    pub(crate) error: Option<String>,
    pub(crate) request: Option<JobId>,
    pub(crate) completion: Option<CompletionReason>,
    pub(crate) requests: Vec<RequestUsage>,
}

/// One provider attempt, including retries. Token fields stay absent when the
/// provider omitted them. Catalogue prices are snapshotted so later catalogue
/// changes do not rewrite historical estimates.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct RequestUsage {
    pub(crate) id: RequestId,
    pub(crate) usage: ModelUsage,
    pub(crate) auth: AuthMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) prices: Option<PriceProvenance>,
    /// Instruction sources consumed by this exact request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) sources: Vec<crate::execution::ResourceSource>,
    /// Skills advertised to the model for this exact request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) advertised: Vec<crate::execution::ResourceSource>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct PriceProvenance {
    pub(crate) source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) input: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) output: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cache_read: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cache_write: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CostCoverage {
    pub(crate) known_micros: Option<u64>,
    pub(crate) incomplete: bool,
}

/// Opaque blocks require the original provider and model. A provider change
/// retains portable text and tool exchanges without these blocks.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct ContinuationMetadata {
    pub(crate) provider: crate::providers::ProviderKind,
    pub(crate) model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reasoning_id: Option<String>,
    pub(crate) blocks: Vec<ContinuationBlock>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum ContinuationBlock {
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    Encrypted {
        data: String,
    },
    Redacted {
        data: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContinuationCheckpoint {
    pub(crate) id: CheckpointId,
    pub(crate) boundary: MessageId,
    pub(crate) pinned: crate::execution::ExecutionSettings,
    pub(crate) budget: BudgetSnapshot,
    pub(crate) run: Option<RunId>,
    pub(crate) attempt: Option<AttemptId>,
    pub(crate) step: Option<String>,
    pub(crate) drafts: Vec<PausedOutputDraft>,
    pub(crate) created_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PausedOutputDraft {
    pub(crate) key: String,
    pub(crate) kind: String,
    pub(crate) markdown: String,
    pub(crate) verdict: Option<String>,
    pub(crate) outcome: Option<String>,
}

impl ContinuationCheckpoint {
    pub(crate) fn valid(&self, messages: &[ConversationMessage]) -> bool {
        self.budget.valid()
            && self.drafts.len() <= 16
            && self.drafts.iter().all(|draft| {
                !draft.key.is_empty()
                    && draft.key.len() <= 64
                    && !draft.kind.is_empty()
                    && draft.kind.len() <= 32
                    && draft.markdown.len() <= 64 * 1024
                    && !draft.key.contains('\0')
                    && !draft.markdown.contains('\0')
            })
            && self
                .step
                .as_ref()
                .is_none_or(|step| !step.is_empty() && step.len() <= 64 && !step.contains('\0'))
            && self.run.is_some() == self.attempt.is_some()
            && self.step.is_some() == self.run.is_some()
            && messages.iter().any(|message| {
                message.id == self.boundary
                    && message.role == MessageRole::Assistant
                    && message.status == MessageStatus::Complete
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HistoryError {
    MalformedCall,
    DuplicateIdentifier,
    OrphanResult,
    Bound,
    Continuation,
    Unsettled,
    Usage,
}

impl HistoryError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::MalformedCall => "Stored history contains a malformed tool call.",
            Self::DuplicateIdentifier => "Stored history contains a duplicate tool call.",
            Self::OrphanResult => "Stored history contains a tool result without its call.",
            Self::Bound => "Stored history exceeds the conversation limit.",
            Self::Continuation => "Stored model continuation data is unavailable or incompatible.",
            Self::Unsettled => {
                "A stored tool call has no trustworthy outcome. Resolve recovery before further work."
            }
            Self::Usage => "Stored history contains invalid request usage.",
        }
    }
}

impl std::fmt::Display for HistoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for HistoryError {}

impl RequestUsage {
    pub(crate) fn valid(&self) -> bool {
        self.usage.valid()
            && self.prices.as_ref().is_none_or(PriceProvenance::valid)
            && self.sources.len() <= crate::execution::resources::MAXIMUM_RESOURCE_SOURCES
            && self
                .sources
                .iter()
                .all(crate::execution::ResourceSource::valid)
            && self.advertised.len() <= crate::execution::resources::MAXIMUM_SKILLS
            && self
                .advertised
                .iter()
                .all(crate::execution::ResourceSource::valid)
    }
}

impl PriceProvenance {
    pub(crate) fn from_catalogue(prices: crate::models::models_dev::ModelPrices) -> Option<Self> {
        if prices.input.is_none()
            && prices.output.is_none()
            && prices.cache_read.is_none()
            && prices.cache_write.is_none()
        {
            return None;
        }
        Some(Self {
            source: PRICE_SOURCE_MODELS_DEV.to_owned(),
            input: prices.input,
            output: prices.output,
            cache_read: prices.cache_read,
            cache_write: prices.cache_write,
        })
    }

    pub(crate) fn valid(&self) -> bool {
        self.source == PRICE_SOURCE_MODELS_DEV
            && (self.input.is_some()
                || self.output.is_some()
                || self.cache_read.is_some()
                || self.cache_write.is_some())
    }
}

impl CostCoverage {
    fn unknown() -> Self {
        Self {
            known_micros: None,
            incomplete: true,
        }
    }
}

/// Catalogue prices are millionths of a US dollar per million tokens.
pub(crate) fn token_cost_micros(price: u64, tokens: u64) -> Option<u64> {
    let product = price.checked_mul(tokens)?;
    Some(product.div_ceil(MICROS_PER_MILLION))
}

pub(crate) fn request_cost(request: &RequestUsage) -> CostCoverage {
    if request.auth != AuthMethod::ApiKey {
        return CostCoverage::unknown();
    }
    if !request.usage.has_tokens() {
        return CostCoverage::unknown();
    }
    let Some(prices) = request.prices.as_ref().filter(|prices| prices.valid()) else {
        return CostCoverage::unknown();
    };
    let mut cost = 0u64;
    let mut any = false;
    let mut incomplete =
        request.usage.input_tokens.is_none() || request.usage.output_tokens.is_none();
    match priced_input(request, prices) {
        Ok(Some(value)) => {
            cost = match cost.checked_add(value) {
                Some(total) => total,
                None => return CostCoverage::unknown(),
            };
            any = true;
        }
        Ok(None) => {}
        Err(()) => incomplete = true,
    }
    match priced_tokens(request.usage.output_tokens, prices.output) {
        Ok(Some(value)) => {
            cost = match cost.checked_add(value) {
                Some(total) => total,
                None => return CostCoverage::unknown(),
            };
            any = true;
        }
        Ok(None) => {}
        Err(()) => incomplete = true,
    }
    if !any {
        return CostCoverage {
            known_micros: None,
            incomplete: true,
        };
    }
    CostCoverage {
        known_micros: Some(cost),
        incomplete,
    }
}

pub(crate) fn total_cost(requests: &[RequestUsage]) -> CostCoverage {
    if requests.is_empty() {
        return CostCoverage {
            known_micros: None,
            incomplete: false,
        };
    }
    let mut known = 0u64;
    let mut any = false;
    let mut incomplete = false;
    for request in requests {
        let part = request_cost(request);
        incomplete |= part.incomplete;
        if let Some(value) = part.known_micros {
            known = match known.checked_add(value) {
                Some(total) => total,
                None => {
                    return CostCoverage {
                        known_micros: None,
                        incomplete: true,
                    };
                }
            };
            any = true;
        }
    }
    CostCoverage {
        known_micros: any.then_some(known),
        incomplete,
    }
}

fn priced_input(request: &RequestUsage, prices: &PriceProvenance) -> Result<Option<u64>, ()> {
    let cache = match request.usage.cache_sum() {
        Ok(value) => value.unwrap_or(0),
        Err(()) => return Err(()),
    };
    let cache_read = request.usage.cache_read_tokens;
    let cache_write = request.usage.cache_creation_tokens;
    if cache_read.is_none() && cache_write.is_none() {
        return priced_tokens(request.usage.input_tokens, prices.input);
    }
    let mut cost = 0u64;
    let mut any = false;
    if let Some(input) = request.usage.input_tokens {
        if cache > input {
            return Err(());
        }
        match priced_tokens(Some(input - cache), prices.input) {
            Ok(Some(value)) => {
                cost = cost.checked_add(value).ok_or(())?;
                any = true;
            }
            Ok(None) => {}
            Err(()) => return Err(()),
        }
    }
    match priced_tokens(cache_read, prices.cache_read) {
        Ok(Some(value)) => {
            cost = cost.checked_add(value).ok_or(())?;
            any = true;
        }
        Ok(None) => {}
        Err(()) => return Err(()),
    }
    match priced_tokens(cache_write, prices.cache_write) {
        Ok(Some(value)) => {
            cost = cost.checked_add(value).ok_or(())?;
            any = true;
        }
        Ok(None) => {}
        Err(()) => return Err(()),
    }
    if any { Ok(Some(cost)) } else { Err(()) }
}

fn priced_tokens(tokens: Option<u64>, price: Option<u64>) -> Result<Option<u64>, ()> {
    match (tokens, price) {
        (None, _) => Ok(None),
        (Some(0), _) => Ok(Some(0)),
        (Some(_), None) => Err(()),
        (Some(tokens), Some(price)) => token_cost_micros(price, tokens).map(Some).ok_or(()),
    }
}

pub(crate) fn valid_requests(requests: &[RequestUsage], role: MessageRole) -> bool {
    if role == MessageRole::User {
        return requests.is_empty();
    }
    if requests.len() > MAXIMUM_REQUEST_USAGE {
        return false;
    }
    let mut seen = std::collections::BTreeSet::new();
    requests
        .iter()
        .all(|request| request.valid() && seen.insert(request.id))
}

/// Validate one assistant activity list against its visible response text.
pub(crate) fn valid_activity(activity: &[AssistantActivity], text: &str) -> bool {
    if activity.is_empty() {
        return true;
    }
    if activity.len() > MAXIMUM_ACTIVITY_ITEMS {
        return false;
    }
    let mut bytes = 0usize;
    let mut response = String::new();
    let mut identifiers: Vec<&str> = Vec::new();
    for item in activity {
        let parts: Vec<&str> = match item {
            AssistantActivity::Response(value) => {
                response.push_str(value);
                vec![value]
            }
            AssistantActivity::Thinking(value) => vec![value],
            AssistantActivity::Tool(tool) => {
                if tool.resource.as_ref().is_some_and(|source| !source.valid())
                    || !valid_command(tool.command.as_ref())
                {
                    return false;
                }
                let mut parts = vec![tool.label.as_str(), tool.output.as_str()];
                if let Some(command) = &tool.command {
                    parts.extend(command.chunks.iter().map(|chunk| chunk.text.as_str()));
                }
                parts
            }
            AssistantActivity::ToolCall {
                id,
                name,
                arguments,
                result,
            } => {
                if id.is_empty()
                    || name.is_empty()
                    || id.len() > MAXIMUM_CALL_IDENTIFIER_BYTES
                    || name.len() > MAXIMUM_CALL_IDENTIFIER_BYTES
                    || !arguments.is_object()
                    || serialised_argument_bytes(arguments) > MAXIMUM_ARGUMENT_BYTES
                {
                    return false;
                }
                if identifiers.contains(&id.as_str()) {
                    return false;
                }
                identifiers.push(id);
                if let Some(command) = result.as_ref().and_then(|tool| tool.command.as_ref())
                    && !valid_command(Some(command))
                {
                    return false;
                }
                bytes = bytes.saturating_add(serialised_argument_bytes(arguments));
                let mut parts = vec![id.as_str(), name.as_str()];
                if let Some(tool) = result {
                    if tool.resource.as_ref().is_some_and(|source| !source.valid()) {
                        return false;
                    }
                    parts.extend([tool.label.as_str(), tool.output.as_str()]);
                    if let Some(command) = &tool.command {
                        parts.extend(command.chunks.iter().map(|chunk| chunk.text.as_str()));
                    }
                }
                parts
            }
        };
        for part in parts {
            bytes = bytes.saturating_add(part.len());
            if part.contains('\0') || bytes > MAXIMUM_ACTIVITY_BYTES {
                return false;
            }
        }
    }
    response == text
}

pub(crate) fn valid_command(command: Option<&crate::execution::CommandResult>) -> bool {
    command.is_none_or(crate::execution::CommandResult::is_bounded)
}

pub(crate) fn valid_message_error(status: MessageStatus, error: Option<&str>) -> bool {
    error.is_none_or(|text| {
        status == MessageStatus::Failed
            && !text.trim().is_empty()
            && text.len() <= crate::providers::MAXIMUM_PROVIDER_DETAIL_BYTES
            && !text.chars().any(char::is_control)
    })
}

pub(crate) fn contains_credential(value: &impl Serialize, secret: Option<&str>) -> bool {
    secret.is_some_and(|secret| {
        if secret.is_empty() {
            return false;
        }
        let Ok(encoded_secret) = serde_json::to_string(secret) else {
            return true;
        };
        serde_json::to_string(value).map_or(true, |encoded| {
            encoded.contains(&encoded_secret[1..encoded_secret.len() - 1])
        })
    })
}

pub(crate) fn valid_continuation(metadata: &[ContinuationMetadata]) -> bool {
    if metadata.len() > 8 {
        return false;
    }
    let mut total = 0usize;
    for item in metadata {
        if item.model.is_empty()
            || item.model.len() > crate::providers::MAXIMUM_MODEL_BYTES
            || item.model.chars().any(char::is_control)
            || item.blocks.len() > MAXIMUM_CONTINUATION_BLOCKS
            || item.reasoning_id.as_ref().is_some_and(|id| {
                id.is_empty()
                    || id.len() > MAXIMUM_CALL_IDENTIFIER_BYTES
                    || id.chars().any(char::is_control)
            })
        {
            return false;
        }
        for block in &item.blocks {
            let size = match block {
                ContinuationBlock::Text { text, signature } => text
                    .len()
                    .saturating_add(signature.as_ref().map_or(0, String::len)),
                ContinuationBlock::Encrypted { data } | ContinuationBlock::Redacted { data } => {
                    data.len()
                }
            };
            total = total.saturating_add(size);
            if total > MAXIMUM_CONTINUATION_BYTES {
                return false;
            }
        }
    }
    true
}

/// Build the provider request projection. Incompatible opaque continuation
/// fields are excluded rather than replayed across a provider change.
pub(crate) fn project(
    messages: &[ConversationMessage],
    selection: Option<&ModelSelection>,
) -> Result<Vec<ChatTurn>, HistoryError> {
    validate_exchange(messages)?;
    let mut history = Vec::new();
    for message in messages {
        match message.role {
            MessageRole::User => {
                if message.text.contains('\0') {
                    return Err(HistoryError::Bound);
                }
                history.push(ChatTurn::user(message.text.clone()));
            }
            MessageRole::Assistant => {
                if message.status == MessageStatus::Pending {
                    continue;
                }
                if !valid_activity(&message.activity, &message.text) {
                    return Err(HistoryError::Bound);
                }
                if !valid_continuation(&message.continuation) {
                    return Err(HistoryError::Continuation);
                }
                let (mut text, calls, tools) = assistant_projection(message)?;
                if calls.iter().any(|call| {
                    call.result.as_ref().is_none_or(|result| {
                        result.command.as_ref().is_some_and(|command| {
                            matches!(
                                command.termination,
                                crate::execution::CommandTermination::Unknown
                            )
                        })
                    })
                }) {
                    return Err(HistoryError::Unsettled);
                }
                if message.status != MessageStatus::Complete {
                    text.clear();
                    if calls.is_empty() {
                        continue;
                    }
                }
                let continuation = continuation_for(&message.continuation, selection)?;
                history.push(ChatTurn {
                    role: crate::providers::Role::Assistant,
                    text,
                    thinking: String::new(),
                    tools,
                    activity: message.activity.clone(),
                    usage: message.requests.clone(),
                    calls,
                    continuation,
                });
            }
        }
    }
    Ok(history)
}

fn assistant_projection(
    message: &ConversationMessage,
) -> Result<(String, Vec<ChatToolCall>, Vec<ToolOutput>), HistoryError> {
    let mut text = if message.activity.is_empty() {
        message.text.clone()
    } else {
        message
            .activity
            .iter()
            .filter_map(|activity| match activity {
                AssistantActivity::Response(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    if text.is_empty() && !message.text.is_empty() && !message.activity.is_empty() {
        text = message.text.clone();
    }
    let mut calls = Vec::new();
    let mut tools = Vec::new();
    let mut identifiers: Vec<&str> = Vec::new();
    for activity in &message.activity {
        match activity {
            AssistantActivity::ToolCall {
                id,
                name,
                arguments,
                result,
            } => {
                if id.is_empty() || name.is_empty() {
                    return Err(HistoryError::MalformedCall);
                }
                if identifiers.contains(&id.as_str()) {
                    return Err(HistoryError::DuplicateIdentifier);
                }
                identifiers.push(id);
                calls.push(ChatToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                    result: result.clone(),
                });
                if let Some(result) = result {
                    tools.push(result.clone());
                }
            }
            AssistantActivity::Tool(tool) => tools.push(tool.clone()),
            _ => {}
        }
    }
    Ok((text, calls, tools))
}

fn continuation_for(
    metadata: &[ContinuationMetadata],
    selection: Option<&ModelSelection>,
) -> Result<Vec<ContinuationMetadata>, HistoryError> {
    let Some(selection) = selection else {
        return Ok(Vec::new());
    };
    let mut kept = Vec::new();
    for item in metadata {
        if item.provider != selection.provider {
            // A provider change keeps portable text and tool exchanges only.
            continue;
        }
        if item.model != selection.model || !valid_continuation(std::slice::from_ref(item)) {
            return Err(HistoryError::Continuation);
        }
        kept.push(item.clone());
    }
    Ok(kept)
}

fn serialised_argument_bytes(arguments: &serde_json::Value) -> usize {
    serde_json::to_vec(arguments)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX)
}

/// Reject a result that has no originating call and duplicate call identifiers
/// across the whole retained exchange.
pub(crate) fn validate_exchange(messages: &[ConversationMessage]) -> Result<(), HistoryError> {
    let mut known: Vec<&str> = Vec::new();
    let mut request_ids = std::collections::BTreeSet::new();
    for message in messages {
        if !valid_requests(&message.requests, message.role)
            || message
                .requests
                .iter()
                .any(|request| !request_ids.insert(request.id))
        {
            return Err(if message.requests.len() > MAXIMUM_REQUEST_USAGE {
                HistoryError::Bound
            } else {
                HistoryError::Usage
            });
        }
        for activity in &message.activity {
            match activity {
                AssistantActivity::ToolCall { id, .. } => {
                    if id.is_empty() {
                        return Err(HistoryError::MalformedCall);
                    }
                    if known.contains(&id.as_str()) {
                        return Err(HistoryError::DuplicateIdentifier);
                    }
                    known.push(id);
                }
                AssistantActivity::Tool(_) => return Err(HistoryError::OrphanResult),
                _ => {}
            }
        }
    }
    Ok(())
}

pub(crate) fn settle_interrupted_questions(message: &mut ConversationMessage) {
    for activity in &mut message.activity {
        if let AssistantActivity::ToolCall { name, result, .. } = activity
            && name == crate::conversations::questions::ASK_USER
            && result.is_none()
        {
            *result = Some(ToolOutput {
                resource: None,
                label: crate::conversations::questions::ASK_USER.to_owned(),
                output: crate::conversations::questions::INTERRUPTED_RESULT.to_owned(),
                command: None,
            });
        }
    }
}

#[cfg(test)]
mod tests;
