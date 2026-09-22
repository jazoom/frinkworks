//! Capacity-safe summary generation.
//!
//! Each request reserves the full output allowance and includes only complete exchanges.
//! No checkpoint replaces the previous context until all requests succeed.

use crate::{
    conversations::{PriceProvenance, RequestId, RequestUsage},
    providers::{
        AuthMethod, ChatTurn, CompletionReason, ModelEvent, ModelUsage, ProviderConnection,
    },
    sessions::Job,
    state::AppState,
};
use futures_util::StreamExt;

use super::{
    CompactionError, MAXIMUM_SUMMARY_BYTES, MAXIMUM_SUMMARY_REQUESTS, SUMMARY_OUTPUT_TOKENS,
    validate_summary,
};

pub(crate) struct SummaryOutcome {
    pub(crate) text: String,
    pub(crate) requests: Vec<RequestUsage>,
}

#[cfg(test)]
mod tests;

pub(crate) async fn generate(
    state: &AppState,
    connection: &ProviderConnection,
    turns: &[ChatTurn],
    preserve: Option<&str>,
    job: &Job,
    mut persist: impl FnMut(&RequestUsage) -> Result<(), &'static str>,
) -> Result<SummaryOutcome, &'static str> {
    let secret = match connection.auth {
        AuthMethod::ApiKey => Some(connection.api_key.expose()),
        AuthMethod::Plan => None,
    };
    let output = state
        .models_dev
        .output_limit(connection.kind, &connection.model);
    let capacity = state
        .models_dev
        .context_limit(connection.kind, &connection.model);
    if output.is_none_or(|limit| limit == 0) || capacity.is_none_or(|limit| limit == 0) {
        return Err(CompactionError::UnknownCapacity.message());
    }
    if output.is_some_and(|limit| limit < SUMMARY_OUTPUT_TOKENS) {
        return Err(CompactionError::Output.message());
    }
    let preamble = super::summary_preamble(preserve).map_err(|error| error.message())?;
    let (previous, source) = super::split_previous_summary(turns);
    let ends = super::exchange_ends(source).map_err(|error| error.message())?;
    let mut groups: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    for end in ends {
        groups.push((start, end));
        start = end + 1;
    }
    if start != source.len() {
        return Err(CompactionError::Unsettled.message());
    }
    if groups.is_empty() {
        return Err(CompactionError::NothingToCompact.message());
    }
    let mut cumulative = previous;
    let mut requests = Vec::new();
    let mut index = 0usize;
    while index < groups.len() {
        if job.cancel_requested() {
            return Err("Context compaction was cancelled. The previous context remains.");
        }
        if requests.len() >= MAXIMUM_SUMMARY_REQUESTS {
            return Err(CompactionError::Requests.message());
        }
        let start_turn = groups[index].0;
        let mut end = index;
        while end + 1 < groups.len()
            && chunk_fits(
                state,
                connection,
                &preamble,
                cumulative.as_deref(),
                source,
                start_turn,
                groups[end + 1].1,
            )?
        {
            end += 1;
        }
        if !chunk_fits(
            state,
            connection,
            &preamble,
            cumulative.as_deref(),
            source,
            start_turn,
            groups[end].1,
        )? {
            return Err(CompactionError::Oversized.message());
        }
        let range = start_turn..=groups[end].1;
        let images: Vec<crate::providers::ChatImage> = source[range.clone()]
            .iter()
            .flat_map(|turn| turn.images.iter().cloned())
            .collect();
        let body = super::chunk_prompt(cumulative.as_deref(), &source[range])
            .map_err(|error| error.message())?;
        let mut request =
            request_usage(state, connection, &source[groups[index].0..=groups[end].1])
                .map_err(|error| error.message())?;
        persist(&request)?;
        let chunk = generate_chunk(
            state,
            connection,
            &preamble,
            body,
            images,
            secret,
            &mut request,
            job,
        )
        .await;
        persist(&request)?;
        let chunk = chunk?;
        cumulative = Some(chunk);
        requests.push(request);
        index = end + 1;
    }
    Ok(SummaryOutcome {
        text: cumulative.unwrap_or_default(),
        requests,
    })
}

/// Measure the summary preamble, cumulative summary and next source chunk as
/// one request. The full output allowance is reserved.
fn chunk_fits(
    state: &AppState,
    connection: &ProviderConnection,
    preamble: &str,
    previous: Option<&str>,
    source: &[ChatTurn],
    start: usize,
    end: usize,
) -> Result<bool, &'static str> {
    let body = match super::chunk_prompt(previous, &source[start..=end]) {
        Ok(body) => body,
        Err(CompactionError::Bound) => return Ok(false),
        Err(error) => return Err(error.message()),
    };
    let images: Vec<crate::providers::ChatImage> = source[start..=end]
        .iter()
        .flat_map(|turn| turn.images.iter().cloned())
        .collect();
    let mut history = [ChatTurn::user(body)];
    history[0].images = images;
    let request = crate::execution::ContextRequest {
        preamble,
        tools: &[],
        turns: &history,
        extra: &[],
        provider: connection.kind,
        model: &connection.model,
        output_limit: Some(SUMMARY_OUTPUT_TOKENS),
    };
    // The full summary allowance also covers the smaller ordinary response reserve.
    let estimate = match crate::execution::context::measure(
        request,
        state
            .models_dev
            .context_limit(connection.kind, &connection.model),
    ) {
        Ok(estimate) => estimate,
        Err(crate::execution::ContextError::Bound | crate::execution::ContextError::Overflow) => {
            return Ok(false);
        }
        Err(error) => return Err(error.message()),
    };
    Ok(estimate.input_tokens.saturating_add(SUMMARY_OUTPUT_TOKENS) <= estimate.limit)
}

fn request_usage(
    state: &AppState,
    connection: &ProviderConnection,
    turns: &[ChatTurn],
) -> Result<RequestUsage, CompactionError> {
    Ok(RequestUsage {
        id: RequestId::generate().map_err(|_| CompactionError::Persist)?,
        usage: ModelUsage::new(connection.kind, connection.model.clone()),
        auth: connection.auth,
        prices: match connection.auth {
            AuthMethod::ApiKey => state
                .models_dev
                .prices(connection.kind, &connection.model)
                .and_then(PriceProvenance::from_catalogue),
            AuthMethod::Plan => None,
        },
        sources: crate::execution::resources::consumed_sources(&[], turns),
        advertised: Vec::new(),
    })
}

#[allow(clippy::too_many_arguments)]
async fn generate_chunk(
    state: &AppState,
    connection: &ProviderConnection,
    preamble: &str,
    body: String,
    images: Vec<crate::providers::ChatImage>,
    secret: Option<&str>,
    request: &mut RequestUsage,
    job: &Job,
) -> Result<String, &'static str> {
    let mut history = [ChatTurn::user(body)];
    history[0].images = images;
    crate::conversations::attachments::prepare_turns(state, connection, &mut history)?;
    let generate = async {
        let mut events = state
            .chat
            .stream_turn(
                connection,
                &history,
                &[],
                &[],
                preamble,
                Some(SUMMARY_OUTPUT_TOKENS),
            )
            .await
            .map_err(|_| "The summary provider request failed. The previous context remains.")?;
        let mut text = String::new();
        let mut completion = None;
        let mut invalid = None;
        let mut count = 0usize;
        while let Some(event) = events.next().await {
            count += 1;
            if count > MAXIMUM_SUMMARY_BYTES.saturating_mul(2) {
                return Err(CompactionError::Bound.message());
            }
            match event.map_err(
                |_| "The summary provider response failed. The previous context remains.",
            )? {
                ModelEvent::Text(piece) => {
                    if completion.is_some()
                        || text.len().saturating_add(piece.len()) > MAXIMUM_SUMMARY_BYTES
                    {
                        invalid = Some(CompactionError::Bound);
                    }
                    if invalid.is_none() {
                        text.push_str(&piece);
                    }
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
                        invalid = Some(CompactionError::Malformed);
                    }
                }
                ModelEvent::ToolCall { .. } => invalid = Some(CompactionError::Malformed),
                ModelEvent::Thinking(_) | ModelEvent::Continuation(_) => {}
            }
        }
        // Drain rejected responses so terminal usage remains available for accounting.
        if let Some(error) = invalid {
            return Err(error.message());
        }
        if completion != Some(CompletionReason::Stop) {
            return Err(CompactionError::Malformed.message());
        }
        validate_summary(&text, secret).map_err(|error| error.message())
    };
    tokio::select! {
        biased;
        _ = job.cancelled() => Err("Context compaction was cancelled. The previous context remains."),
        result = tokio::time::timeout(std::time::Duration::from_secs(120), generate) => {
            result.unwrap_or(Err("Context compaction timed out. The previous context remains."))
        }
    }
}
