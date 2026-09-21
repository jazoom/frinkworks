use crate::{
    conversations::{PriceProvenance, RequestId, RequestUsage},
    providers::{
        AuthMethod, ChatTurn, CompletionReason, ModelEvent, ModelUsage, ProviderConnection,
    },
    sessions::Job,
    state::AppState,
};
use futures_util::StreamExt;

use super::{CompactionError, MAXIMUM_SUMMARY_BYTES, SUMMARY_OUTPUT_TOKENS, SUMMARY_PREAMBLE};

pub(crate) async fn generate(
    state: &AppState,
    connection: &ProviderConnection,
    turns: &[ChatTurn],
    job: &Job,
    mut persist: impl FnMut(&RequestUsage) -> Result<(), &'static str>,
) -> Result<(String, RequestUsage), &'static str> {
    let secret = match connection.auth {
        AuthMethod::ApiKey => Some(connection.api_key.expose()),
        AuthMethod::Plan => None,
    };
    let prompt = super::summary_prompt(turns).map_err(|error| error.message())?;
    let history = [ChatTurn::user(crate::tools::redact(&prompt, secret))];
    let estimate = crate::execution::context::measure(
        crate::execution::ContextRequest {
            preamble: SUMMARY_PREAMBLE,
            tools: &[],
            turns: &history,
            extra: &[],
        },
        state
            .models_dev
            .context_limit(connection.kind, &connection.model),
    )
    .map_err(|error| error.message())?;
    let mut request = RequestUsage {
        id: RequestId::generate()
            .map_err(|_| crate::conversations::ConversationError::Random.message())?,
        usage: ModelUsage::new(connection.kind, connection.model.clone()),
        auth: connection.auth,
        prices: match connection.auth {
            AuthMethod::ApiKey => state
                .models_dev
                .prices(connection.kind, &connection.model)
                .and_then(PriceProvenance::from_catalogue),
            AuthMethod::Plan => None,
        },
    };
    persist(&request)?;
    let generate = async {
        let mut events = state
            .chat
            .stream_turn(
                connection,
                &history,
                &[],
                &[],
                SUMMARY_PREAMBLE,
                Some(SUMMARY_OUTPUT_TOKENS.min(estimate.output_allowance)),
            )
            .await
            .map_err(|_| "The summary provider request failed. The previous context remains.")?;
        let mut text = String::new();
        let mut completion = None;
        let mut count = 0usize;
        while let Some(event) = events.next().await {
            count += 1;
            if count > 4096 {
                return Err(CompactionError::Bound.message());
            }
            match event.map_err(
                |_| "The summary provider response failed. The previous context remains.",
            )? {
                ModelEvent::Text(piece) => {
                    if completion.is_some()
                        || text.len().saturating_add(piece.len()) > MAXIMUM_SUMMARY_BYTES
                    {
                        return Err(CompactionError::Bound.message());
                    }
                    text.push_str(&piece);
                }
                ModelEvent::Usage {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_creation_tokens,
                } => {
                    request.usage.input_tokens = input_tokens;
                    request.usage.output_tokens = output_tokens;
                    request.usage.cache_read_tokens = cache_read_tokens;
                    request.usage.cache_creation_tokens = cache_creation_tokens;
                }
                ModelEvent::Complete { reason } => {
                    if completion.replace(reason).is_some() || reason != CompletionReason::Stop {
                        return Err(CompactionError::Malformed.message());
                    }
                }
                ModelEvent::ToolCall { .. } => return Err(CompactionError::Malformed.message()),
                ModelEvent::Thinking(_) | ModelEvent::Continuation(_) => {}
            }
        }
        if completion != Some(CompletionReason::Stop) {
            return Err(CompactionError::Malformed.message());
        }
        super::validate_summary(&text, secret).map_err(|error| error.message())
    };
    let result = tokio::select! {
        biased;
        _ = job.cancelled() => Err("Context compaction was cancelled. The previous context remains."),
        result = tokio::time::timeout(std::time::Duration::from_secs(120), generate) => {
            result.unwrap_or(Err("Context compaction timed out. The previous context remains."))
        }
    };
    persist(&request)?;
    result.map(|text| (text, request))
}
