use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;

use rig_core::completion::{AssistantContent, Message};

use crate::{
    agents::{AgentId, DirectoryPolicy, ToolId},
    conversations::ConversationId,
    execution::ToolLocation,
    providers::{
        AssistantActivity, AssistantReply, ChatTurn, CompletionReason, ModelEvent, ModelUsage,
        ProviderConnection, ProviderError, ToolOutput,
    },
    sandbox::GuestSandbox,
    sessions::Job,
    state::AppState,
    tools,
};

#[cfg(test)]
mod tests;

pub(crate) struct AgentRunSpec {
    pub(crate) agent_id: Option<AgentId>,
    pub(crate) revision: u32,
    pub(crate) preamble: String,
    pub(crate) tools: Vec<rig_core::completion::ToolDefinition>,
    pub(crate) tool_ids: Vec<ToolId>,
    pub(crate) policy: DirectoryPolicy,
    pub(crate) connection: ProviderConnection,
    pub(crate) location: ToolLocation,
    pub(crate) sandbox: Option<std::sync::Arc<GuestSandbox>>,
    pub(crate) host: Option<tools::HostRunSpec>,
    pub(crate) output_drafts:
        Option<std::sync::Arc<std::sync::Mutex<crate::workflows::artefacts::output::OutputDrafts>>>,
    pub(crate) required_outputs: Vec<crate::workflows::definition::RequiredOutput>,
    pub(crate) evidence: Option<crate::workflows::AttemptEvidenceContext>,
    pub(crate) output_scope: Option<crate::execution::OutputScope>,
    pub(crate) conversation: Option<ConversationId>,
    pub(crate) steering_session: Option<crate::sessions::SessionId>,
}

pub(super) const MIN_PROGRESS_INTERVAL: Duration = if cfg!(test) {
    Duration::ZERO
} else {
    Duration::from_millis(200)
};
const THINKING_INITIAL_DELAY: Duration = Duration::from_millis(75);
const THINKING_PROGRESS_INTERVAL: Duration = Duration::from_millis(75);
const MAXIMUM_THINKING_PROGRESS_BYTES: usize = 192;

// Stay below the 1 MiB envelope after Markdown HTML and the job-observe patch.
pub(super) const MAXIMUM_MODEL_REPLY_BYTES: usize = 64 * 1024;
pub(super) const MAXIMUM_THINKING_BYTES: usize = 64 * 1024;
const MAXIMUM_VISIBLE_TOOL_BYTES: usize = 64 * 1024;

const MAXIMUM_TOOL_ROUNDS: usize = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentOutcome {
    Completed,
    ProviderFailure,
    ToolFailure,
    AuthorityFailure,
    PersistenceFailure,
    UncertainEffect,
    Cancelled,
}

pub(crate) struct AgentActionEnd {
    pub(crate) outcome: AgentOutcome,
    pub(crate) error: Option<String>,
    pub(crate) reply: AssistantReply,
}

pub(crate) async fn run_agent_action(
    state: &AppState,
    spec: AgentRunSpec,
    turns: Vec<ChatTurn>,
    job: Arc<Job>,
) -> AgentActionEnd {
    let agent_id = spec.agent_id;
    tracing::debug!(
        agent_id = ?agent_id,
        agent_revision = spec.revision,
        "agent job started"
    );
    let secret = match spec.connection.auth {
        crate::providers::AuthMethod::ApiKey => Some(spec.connection.api_key.expose().to_owned()),
        crate::providers::AuthMethod::Plan => None,
    };
    let secret = secret.as_deref();
    let mut extra: Vec<Message> = Vec::new();
    let mut reply = AssistantReply::default();
    let mut model_reply_bytes = 0usize;
    let mut thinking_bytes = 0usize;
    let mut visible_tool_bytes = 0usize;
    let mut published_response = 0usize;
    let mut thinking_progress = ThinkingProgress::default();
    let mut last_emit = Instant::now();
    let mut output_visible = false;
    let mut response_redactor = StreamRedactor::new(secret);
    let mut thinking_redactor = StreamRedactor::new(secret);
    let mut event_count = 0usize;

    'round: for _ in 0..MAXIMUM_TOOL_ROUNDS {
        if let Err(error) = persist_output(state, spec.conversation, &job, &reply, false) {
            return store_failure(&reply, error);
        }
        let mut request_tools = spec.tools.clone();
        if spec
            .output_scope
            .as_ref()
            .is_some_and(|scope| state.outputs.has_scope(scope))
        {
            request_tools.push(tools::read_output_definition());
        }
        if spec.conversation.is_some()
            && spec
                .steering_session
                .is_some_and(|session| state.sessions.contains_live(&session))
        {
            request_tools.push(tools::ask_user_definition());
        }
        let committed_reply = reply.clone();
        let committed_response = published_response;
        let committed_thinking = thinking_progress.published;
        let committed_model_bytes = model_reply_bytes;
        let committed_thinking_bytes = thinking_bytes;
        let committed_visible_tool_bytes = visible_tool_bytes;
        let mut retry_attempts = 0u32;
        let mut retry_waited = Duration::ZERO;
        let mut text;
        let mut calls;
        let mut completion;
        'provider: loop {
            reply.completion = Some(CompletionReason::Unknown);
            thinking_progress.begin_phase();
            if job.cancel_requested() {
                return cancel_action(&job, &reply);
            }
            job.clear_retry();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
            let mut events = tokio::select! {
                biased;
                _ = job.cancelled() => {
                    return cancel_action(&job, &reply);
                }
                _ = tokio::time::sleep_until(deadline) => {
                    return AgentActionEnd {
                        outcome: AgentOutcome::ProviderFailure,
                        error: Some(ProviderError::Unreachable.message().to_owned()),
                        reply,
                    };
                }
                result = state.chat.stream_turn(
                    &spec.connection,
                    &turns,
                    &extra,
                    &request_tools,
                    &spec.preamble,
                ) => match result {
                    Ok(stream) => stream,
                    Err(error) => {
                        if let Some(delay) = take_retry(&error, &mut retry_attempts, &mut retry_waited) {
                            if let Err(error) = record_failed_attempt(state, &spec, &job, &reply, &committed_reply, &error) {
                                return store_failure(&reply, error);
                            }
                            restore_request(
                                &mut reply,
                                &committed_reply,
                                &mut published_response,
                                committed_response,
                                &mut thinking_progress,
                                committed_thinking,
                                &mut model_reply_bytes,
                                committed_model_bytes,
                                &mut thinking_bytes,
                                committed_thinking_bytes,
                                &mut visible_tool_bytes,
                                committed_visible_tool_bytes,
                                &mut response_redactor,
                                &mut thinking_redactor,
                                &mut event_count,
                                secret,
                                &job,
                            );
                            if let Err(error) =
                                persist_output(state, spec.conversation, &job, &reply, false)
                            {
                                return store_failure(&reply, error);
                            }
                            job.set_retry(retry_attempts, delay, error.message());
                            if wait_retry(&job, delay).await {
                                return cancel_action(&job, &reply);
                            }
                            continue 'provider;
                        }
                        thinking_progress.flush(&job, &reply.thinking);
                        publish_reply_remaining(
                            &job,
                            &reply,
                            published_response,
                            thinking_progress.published,
                        );
                        return AgentActionEnd {
                            outcome: AgentOutcome::ProviderFailure,
                            error: Some(error.message().to_owned()),
                            reply: reply.clone(),
                        };
                    }
                },
            };
            text = String::new();
            calls = Vec::new();
            completion = None;
            loop {
                if let Err(error) = persist_output(state, spec.conversation, &job, &reply, false) {
                    return store_failure(&reply, error);
                }
                let thinking_deadline = thinking_progress.deadline(&reply.thinking);
                let wait_for_thinking = async {
                    match thinking_deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
                        None => std::future::pending().await,
                    }
                };
                let chunk = tokio::select! {
                    biased;
                    _ = job.cancelled() => {
                        return cancel_action(&job, &reply);
                    }
                    _ = tokio::time::sleep_until(deadline) => {
                        reply.completion = Some(CompletionReason::Unknown);
                        return AgentActionEnd {
                            outcome: AgentOutcome::ProviderFailure,
                            error: Some(ProviderError::Unreachable.message().to_owned()),
                            reply,
                        };
                    }
                    _ = wait_for_thinking => {
                        thinking_progress.publish_due(&job, &reply.thinking, Instant::now());
                        continue;
                    }
                    chunk = events.next() => chunk,
                };
                let Some(chunk) = chunk else {
                    break;
                };
                event_count += 1;
                if event_count > 4096
                    || reply.activity.len() >= 256
                    || matches!(&chunk, Ok(ModelEvent::Text(text) | ModelEvent::Thinking(text)) if text.contains('\0'))
                {
                    return AgentActionEnd {
                        outcome: AgentOutcome::ProviderFailure,
                        error: Some(ProviderError::ReplyTooLong.message().to_owned()),
                        reply,
                    };
                }
                match chunk {
                    Ok(ModelEvent::Text(piece)) => {
                        let tail = thinking_redactor.finish_boundary();
                        append_thinking_piece(&mut reply, &tail, &mut thinking_bytes);
                        thinking_progress.flush(&job, &reply.thinking);
                        let piece = response_redactor.push(&piece);
                        if let Some(evidence) = &spec.evidence {
                            evidence.response(&piece, secret);
                        }
                        text.push_str(&piece);
                        let truncated =
                            append_model_piece(&mut reply, &piece, &mut model_reply_bytes);
                        publish_progress(
                            &job,
                            &reply.text,
                            &mut published_response,
                            OutputChannel::Response,
                            &mut last_emit,
                            &mut output_visible,
                        );
                        if truncated {
                            reply.completion = Some(CompletionReason::Length);
                            publish_reply_remaining(
                                &job,
                                &reply,
                                published_response,
                                thinking_progress.published,
                            );
                            return AgentActionEnd {
                                outcome: AgentOutcome::ProviderFailure,
                                error: Some(ProviderError::ReplyTooLong.message().to_owned()),
                                reply: reply.clone(),
                            };
                        }
                    }
                    Ok(ModelEvent::Thinking(piece)) => {
                        let tail = response_redactor.finish_boundary();
                        text.push_str(&tail);
                        append_model_piece(&mut reply, &tail, &mut model_reply_bytes);
                        publish_remaining(
                            &job,
                            &reply.text,
                            published_response,
                            OutputChannel::Response,
                        );
                        published_response = reply.text.len();
                        if !matches!(reply.activity.last(), Some(AssistantActivity::Thinking(_))) {
                            reply.push_thinking("");
                            job.push_thinking(String::new());
                        }
                        let piece = thinking_redactor.push(&piece);
                        if let Some(evidence) = &spec.evidence {
                            evidence.thinking(&piece, secret);
                        }
                        append_thinking_piece(&mut reply, &piece, &mut thinking_bytes);
                        thinking_progress.note_pending(&reply.thinking, Instant::now());
                    }
                    Ok(ModelEvent::ToolCall {
                        id,
                        name,
                        arguments,
                    }) => {
                        let tail = response_redactor.finish_boundary();
                        text.push_str(&tail);
                        append_model_piece(&mut reply, &tail, &mut model_reply_bytes);
                        let tail = thinking_redactor.finish_boundary();
                        append_thinking_piece(&mut reply, &tail, &mut thinking_bytes);
                        publish_reply_before_tools(
                            &job,
                            &reply,
                            &mut published_response,
                            &mut thinking_progress,
                        );
                        if id.len() > 512
                            || name.len() > 512
                            || id.contains('\0')
                            || name.contains('\0')
                        {
                            return AgentActionEnd {
                                outcome: AgentOutcome::ProviderFailure,
                                error: Some(ProviderError::Refused.message().to_owned()),
                                reply,
                            };
                        }
                        let visible_id = tools::redact(&id, secret);
                        let visible_name = tools::redact(&name, secret);
                        visible_tool_bytes += visible_id.len() + visible_name.len();
                        if visible_id.len() > 512
                            || visible_name.len() > 512
                            || visible_tool_bytes > MAXIMUM_VISIBLE_TOOL_BYTES
                        {
                            return AgentActionEnd {
                                outcome: AgentOutcome::ProviderFailure,
                                error: Some(ProviderError::ReplyTooLong.message().to_owned()),
                                reply,
                            };
                        }
                        if !arguments.is_object() {
                            return AgentActionEnd {
                                outcome: AgentOutcome::ProviderFailure,
                                error: Some(ProviderError::Refused.message().to_owned()),
                                reply,
                            };
                        }
                        if crate::conversations::history::contains_credential(
                            &(&id, &name, &arguments),
                            secret,
                        ) || serde_json::to_vec(&arguments)
                            .map_or(true, |bytes| bytes.len() > 64 * 1024)
                        {
                            return AgentActionEnd {
                                outcome: AgentOutcome::ProviderFailure,
                                error: Some(ProviderError::Refused.message().to_owned()),
                                reply,
                            };
                        }
                        job.start_tool(visible_id.clone(), visible_name.clone(), arguments.clone());
                        reply.start_tool(visible_id, visible_name, arguments.clone());
                        calls.push((id, name, arguments));
                    }
                    Ok(ModelEvent::Usage { input_tokens }) => {
                        let usage = ModelUsage {
                            provider: spec.connection.kind,
                            model: spec.connection.model.clone(),
                            input_tokens,
                        };
                        reply.usage = Some(usage.clone());
                        if let Some(evidence) = &spec.evidence {
                            evidence.usage(&usage);
                        }
                        job.push_usage(usage);
                    }
                    Ok(ModelEvent::Continuation(metadata)) => {
                        reply.continuation.push(metadata);
                        if !crate::conversations::history::valid_continuation(&reply.continuation)
                            || crate::conversations::history::contains_credential(
                                &reply.continuation,
                                secret,
                            )
                        {
                            reply.continuation.pop();
                            return AgentActionEnd {
                                outcome: AgentOutcome::ProviderFailure,
                                error: Some(
                                    crate::conversations::history::HistoryError::Continuation
                                        .message()
                                        .to_owned(),
                                ),
                                reply,
                            };
                        }
                    }
                    Ok(ModelEvent::Complete { reason }) => {
                        completion = match completion {
                            Some(previous) if previous != reason => Some(CompletionReason::Unknown),
                            Some(previous) => Some(previous),
                            None => Some(reason),
                        };
                        reply.completion = completion;
                    }
                    Err(error) => {
                        if let Some(delay) =
                            take_retry(&error, &mut retry_attempts, &mut retry_waited)
                        {
                            let tail = response_redactor.finish_boundary();
                            append_model_piece(&mut reply, &tail, &mut model_reply_bytes);
                            let tail = thinking_redactor.finish_boundary();
                            append_thinking_piece(&mut reply, &tail, &mut thinking_bytes);
                            if let Err(error) = record_failed_attempt(
                                state,
                                &spec,
                                &job,
                                &reply,
                                &committed_reply,
                                &error,
                            ) {
                                return store_failure(&reply, error);
                            }
                            restore_request(
                                &mut reply,
                                &committed_reply,
                                &mut published_response,
                                committed_response,
                                &mut thinking_progress,
                                committed_thinking,
                                &mut model_reply_bytes,
                                committed_model_bytes,
                                &mut thinking_bytes,
                                committed_thinking_bytes,
                                &mut visible_tool_bytes,
                                committed_visible_tool_bytes,
                                &mut response_redactor,
                                &mut thinking_redactor,
                                &mut event_count,
                                secret,
                                &job,
                            );
                            if let Err(error) =
                                persist_output(state, spec.conversation, &job, &reply, false)
                            {
                                return store_failure(&reply, error);
                            }
                            job.set_retry(retry_attempts, delay, error.message());
                            if wait_retry(&job, delay).await {
                                return cancel_action(&job, &reply);
                            }
                            continue 'provider;
                        }
                        reply.completion = Some(CompletionReason::Unknown);
                        thinking_progress.flush(&job, &reply.thinking);
                        publish_reply_remaining(
                            &job,
                            &reply,
                            published_response,
                            thinking_progress.published,
                        );
                        return AgentActionEnd {
                            outcome: AgentOutcome::ProviderFailure,
                            error: Some(error.message().to_owned()),
                            reply: reply.clone(),
                        };
                    }
                }
            }

            if job.cancel_requested() {
                return cancel_action(&job, &reply);
            }
            reply.completion = Some(completion.unwrap_or(CompletionReason::Unknown));

            if calls.is_empty() {
                let response_tail = response_redactor.finish();
                if let Some(evidence) = &spec.evidence {
                    evidence.response(&response_tail, secret);
                }
                let thinking_tail = thinking_redactor.finish();
                if let Some(evidence) = &spec.evidence {
                    evidence.thinking(&thinking_tail, secret);
                }
                let truncated =
                    append_model_piece(&mut reply, &response_tail, &mut model_reply_bytes);
                append_thinking_piece(&mut reply, &thinking_tail, &mut thinking_bytes);
                thinking_progress.flush(&job, &reply.thinking);
                publish_reply_remaining(
                    &job,
                    &reply,
                    published_response,
                    thinking_progress.published,
                );
                if truncated {
                    reply.completion = Some(CompletionReason::Length);
                    return AgentActionEnd {
                        outcome: AgentOutcome::ProviderFailure,
                        error: Some(ProviderError::ReplyTooLong.message().to_owned()),
                        reply: reply.clone(),
                    };
                }
                if matches!(completion, Some(CompletionReason::Stop) | None)
                    && reply.text.trim().is_empty()
                    && let Some(delay) = take_retry(
                        &ProviderError::EmptyReply,
                        &mut retry_attempts,
                        &mut retry_waited,
                    )
                {
                    if let Err(error) = record_failed_attempt(
                        state,
                        &spec,
                        &job,
                        &reply,
                        &committed_reply,
                        &ProviderError::EmptyReply,
                    ) {
                        return store_failure(&reply, error);
                    }
                    restore_request(
                        &mut reply,
                        &committed_reply,
                        &mut published_response,
                        committed_response,
                        &mut thinking_progress,
                        committed_thinking,
                        &mut model_reply_bytes,
                        committed_model_bytes,
                        &mut thinking_bytes,
                        committed_thinking_bytes,
                        &mut visible_tool_bytes,
                        committed_visible_tool_bytes,
                        &mut response_redactor,
                        &mut thinking_redactor,
                        &mut event_count,
                        secret,
                        &job,
                    );
                    if let Err(error) =
                        persist_output(state, spec.conversation, &job, &reply, false)
                    {
                        return store_failure(&reply, error);
                    }
                    job.set_retry(retry_attempts, delay, ProviderError::EmptyReply.message());
                    if wait_retry(&job, delay).await {
                        return cancel_action(&job, &reply);
                    }
                    continue 'provider;
                }
                let end = finish_without_tools(&reply, completion);
                if end.outcome != AgentOutcome::Completed {
                    return end;
                }
                match take_steering(state, &spec, &job, &reply) {
                    Ok(Some(steering)) => {
                        text.push_str(&response_tail);
                        extra.push(Message::assistant(text));
                        extra.push(Message::user(steering));
                        let _ = response_redactor.finish_boundary();
                        let _ = thinking_redactor.finish_boundary();
                        reset_round(
                            &mut reply,
                            &mut model_reply_bytes,
                            &mut thinking_bytes,
                            &mut visible_tool_bytes,
                            &mut published_response,
                            &mut thinking_progress,
                            &mut output_visible,
                        );
                        continue 'round;
                    }
                    Ok(None) => return end,
                    Err(error) => return store_failure(&reply, error),
                }
            }
            let _ = response_redactor.finish_boundary();
            let _ = thinking_redactor.finish_boundary();
            if !completion.is_some_and(CompletionReason::allows_tool_dispatch) {
                thinking_progress.flush(&job, &reply.thinking);
                publish_reply_remaining(
                    &job,
                    &reply,
                    published_response,
                    thinking_progress.published,
                );
                return AgentActionEnd {
                    outcome: AgentOutcome::ProviderFailure,
                    error: Some(ProviderError::Incomplete.message().to_owned()),
                    reply: reply.clone(),
                };
            }
            if let Err(error) = validate_tool_batch(&calls, &request_tools) {
                thinking_progress.flush(&job, &reply.thinking);
                publish_reply_remaining(
                    &job,
                    &reply,
                    published_response,
                    thinking_progress.published,
                );
                return AgentActionEnd {
                    outcome: AgentOutcome::ProviderFailure,
                    error: Some(error.message().to_owned()),
                    reply: reply.clone(),
                };
            }
            break 'provider;
        }

        publish_reply_before_tools(
            &job,
            &reply,
            &mut published_response,
            &mut thinking_progress,
        );
        extra.push(assistant_tool_message(&text, &calls));
        let host = spec.host.as_ref().map(|host| tools::HostToolContext {
            state,
            settings: &host.settings,
            secret,
            session: host.session,
            conversation: host.conversation,
            execution_revision: host.execution_revision,
            directory: host.directory.clone(),
            run: host.run.clone(),
            step: host.step.clone(),
            attempt: host.attempt.clone(),
        });
        let sandbox = if spec.location == ToolLocation::Host {
            None
        } else {
            spec.sandbox.as_deref()
        };
        let context = tools::AgentToolContext {
            sandbox,
            policy: &spec.policy,
            job: &job,
            tools: &spec.tool_ids,
            location: spec.location,
            host,
            secret,
            outputs: Some(&state.outputs),
            output_scope: spec.output_scope.clone(),
            output_drafts: spec.output_drafts.as_deref(),
            required_outputs: &spec.required_outputs,
            questions: spec.conversation.zip(spec.steering_session).map(
                |(conversation, session)| tools::QuestionToolContext {
                    state,
                    session,
                    conversation,
                },
            ),
        };
        if let Err(error) = persist_output(state, spec.conversation, &job, &reply, true) {
            return store_failure(&reply, error);
        }
        let mut resolved_calls: Vec<crate::providers::ChatToolCall> = Vec::new();
        let mut pending = calls.into_iter();
        while let Some((id, name, arguments)) = pending.next() {
            let trace = tools::invoke(&context, &id, &name, &arguments).await;
            let output = tools::redact(&trace.output, secret);

            let label = tools::redact(&trace.label, secret);
            let command = trace.command.map(|command| command.redacted(secret));
            let command = command.map(|command| match &spec.output_scope {
                Some(scope) => tools::retain_command(state, scope.clone(), job.id(), &id, command),
                None => command,
            });
            let mut failure = trace.failure;
            if command.as_ref().is_some_and(|command| {
                command.termination == crate::execution::CommandTermination::StorageFailure
            }) {
                failure = Some(tools::ToolFailureKind::Persistence);
            }
            let footer = command.as_ref().map(|command| {
                let mut footer = format!("\nCommand outcome: {}.", command.status_text());
                if let Some(retained) = &command.retained {
                    footer.push_str(&format!(
                        "\nRetained output reference: {}. Read with read_output, offset 1. Retained bytes: {}. Storage truncated: {}.",
                        retained.reference, retained.bytes, retained.truncated,
                    ));
                }
                footer
            }).unwrap_or_default();
            let output = format!(
                "{}{footer}",
                bound_visible_text(
                    &output,
                    crate::tools::MAXIMUM_TOOL_BYTES.saturating_sub(footer.len()),
                )
            );
            if let Some(evidence) = &spec.evidence {
                evidence.tool(
                    &ToolOutput {
                        label: label.clone(),
                        output: output.clone(),
                        command: command.clone(),
                    },
                    secret,
                );
            }
            resolved_calls.push(crate::providers::ChatToolCall {
                id: id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
                result: Some(ToolOutput {
                    label: label.clone(),
                    output: output.clone(),
                    command: command.clone(),
                }),
            });
            let visible_id = tools::redact(&id, secret);
            reply.finish_tool(
                &visible_id,
                ToolOutput {
                    label: label.clone(),
                    output: output.clone(),
                    command: command.clone(),
                },
            );
            if let Err(error) = persist_output(state, spec.conversation, &job, &reply, true) {
                return store_failure(&reply, error);
            }
            if let Some(visible) =
                visible_tool_output(label, &output, command, &mut visible_tool_bytes)
            {
                job.finish_tool(visible_id, visible);
                output_visible = true;
            }
            extra.push(Message::tool_result(id, name, output.clone()));
            if job.cancel_requested() {
                record_not_dispatched(
                    pending,
                    secret,
                    &mut reply,
                    &mut resolved_calls,
                    &mut extra,
                    &job,
                    &mut visible_tool_bytes,
                );
                if let Err(error) = persist_output(state, spec.conversation, &job, &reply, true) {
                    return store_failure(&reply, error);
                }
                return cancel_action(&job, &reply);
            }
            if let Some(kind) = failure.filter(|kind| kind.stops_loop()) {
                record_not_dispatched(
                    pending,
                    secret,
                    &mut reply,
                    &mut resolved_calls,
                    &mut extra,
                    &job,
                    &mut visible_tool_bytes,
                );
                if let Err(error) = persist_output(state, spec.conversation, &job, &reply, true) {
                    return store_failure(&reply, error);
                }
                return AgentActionEnd {
                    outcome: outcome_for_tool(kind),
                    error: Some(output),
                    reply: reply.clone(),
                };
            }
        }
        if let Some(evidence) = &spec.evidence
            && !resolved_calls.is_empty()
            && let Err(error) = evidence.turn(&crate::providers::ChatTurn {
                role: crate::providers::Role::Assistant,
                text: text.clone(),
                thinking: String::new(),
                tools: Vec::new(),
                activity: Vec::new(),
                usage: None,
                calls: resolved_calls,
                continuation: reply.continuation.clone(),
            })
        {
            return AgentActionEnd {
                outcome: AgentOutcome::PersistenceFailure,
                error: Some(error.message().to_owned()),
                reply,
            };
        }
        match take_steering(state, &spec, &job, &reply) {
            Ok(Some(text)) => {
                extra.push(Message::user(text));
                let _ = response_redactor.finish_boundary();
                let _ = thinking_redactor.finish_boundary();
                reset_round(
                    &mut reply,
                    &mut model_reply_bytes,
                    &mut thinking_bytes,
                    &mut visible_tool_bytes,
                    &mut published_response,
                    &mut thinking_progress,
                    &mut output_visible,
                );
            }
            Ok(None) => {}
            Err(error) => return store_failure(&reply, error),
        }
    }

    thinking_progress.flush(&job, &reply.thinking);
    publish_reply_remaining(
        &job,
        &reply,
        published_response,
        thinking_progress.published,
    );
    AgentActionEnd {
        outcome: AgentOutcome::ToolFailure,
        error: Some(TOOL_LOOP_LIMIT.to_owned()),
        reply,
    }
}

// A provider can split a credential across arbitrary stream events.
pub(crate) struct StreamRedactor<'a> {
    secret: Option<&'a str>,
    pending: String,
}

impl<'a> StreamRedactor<'a> {
    pub(crate) fn new(secret: Option<&'a str>) -> Self {
        Self {
            secret: secret.filter(|secret| !secret.is_empty()),
            pending: String::new(),
        }
    }

    pub(crate) fn push(&mut self, piece: &str) -> String {
        self.pending.push_str(piece);
        let Some(secret) = self.secret else {
            return std::mem::take(&mut self.pending);
        };
        let redacted = tools::redact(&self.pending, Some(secret));
        let retained = (1..secret.len().min(redacted.len() + 1))
            .rev()
            .find(|&length| {
                secret.is_char_boundary(length) && redacted.ends_with(&secret[..length])
            })
            .unwrap_or(0);
        let split = redacted.len() - retained;
        self.pending = redacted[split..].to_owned();
        redacted[..split].to_owned()
    }

    pub(crate) fn finish(&mut self) -> String {
        std::mem::take(&mut self.pending)
    }

    // A channel boundary must not move a possible credential prefix into a later block.
    pub(crate) fn finish_boundary(&mut self) -> String {
        if self.pending.is_empty() {
            String::new()
        } else {
            self.pending.clear();
            "[redacted]".to_owned()
        }
    }
}

const TOOL_LOOP_LIMIT: &str = "The agent stopped after too many tool calls. Try again.";

fn finish_without_tools(
    reply: &AssistantReply,
    completion: Option<CompletionReason>,
) -> AgentActionEnd {
    let error = match completion {
        Some(CompletionReason::Stop) if !reply.text.trim().is_empty() => None,
        Some(CompletionReason::Stop) => Some(ProviderError::EmptyReply),
        Some(CompletionReason::Refusal) => Some(ProviderError::Refused),
        Some(
            CompletionReason::Length | CompletionReason::Unknown | CompletionReason::ToolCalls,
        )
        | None => Some(ProviderError::Incomplete),
    };
    AgentActionEnd {
        outcome: if error.is_some() {
            AgentOutcome::ProviderFailure
        } else {
            AgentOutcome::Completed
        },
        error: error.map(|error| error.message().to_owned()),
        reply: reply.clone(),
    }
}

fn validate_tool_batch(
    calls: &[(String, String, serde_json::Value)],
    definitions: &[rig_core::completion::ToolDefinition],
) -> Result<(), ProviderError> {
    let mut seen = std::collections::BTreeSet::new();
    for (id, name, arguments) in calls {
        if id.is_empty()
            || name.is_empty()
            || id.len() > crate::conversations::history::MAXIMUM_CALL_IDENTIFIER_BYTES
            || name.len() > crate::conversations::history::MAXIMUM_CALL_IDENTIFIER_BYTES
            || !arguments.is_object()
        {
            return Err(ProviderError::Refused);
        }
        if !seen.insert(id) {
            return Err(ProviderError::Refused);
        }
        let definition = definitions
            .iter()
            .find(|definition| definition.name == *name)
            .ok_or(ProviderError::Refused)?;
        let schema = &definition.parameters;
        if schema["required"].as_array().is_some_and(|required| {
            required
                .iter()
                .any(|key| key.as_str().is_none_or(|key| arguments.get(key).is_none()))
        }) {
            return Err(ProviderError::Refused);
        }
        for (key, value) in arguments.as_object().ok_or(ProviderError::Refused)? {
            let property = &schema["properties"][key];
            if !schema_value_matches(property, value) {
                return Err(ProviderError::Refused);
            }
        }
    }
    Ok(())
}

fn schema_value_matches(schema: &serde_json::Value, value: &serde_json::Value) -> bool {
    if schema["enum"]
        .as_array()
        .is_some_and(|values| !values.contains(value))
    {
        return false;
    }
    match schema["type"].as_str() {
        Some("string") => value.as_str().is_some_and(|text| !text.contains('\0')),
        Some("integer") => value.as_u64().is_some(),
        Some("boolean") => value.as_bool().is_some(),
        Some("array") => value.as_array().is_some_and(|items| {
            items
                .iter()
                .all(|item| schema_value_matches(&schema["items"], item))
        }),
        Some("object") => {
            let Some(object) = value.as_object() else {
                return false;
            };
            if schema["required"].as_array().is_some_and(|required| {
                required
                    .iter()
                    .any(|key| key.as_str().is_none_or(|key| !object.contains_key(key)))
            }) {
                return false;
            }
            object.iter().all(|(key, item)| {
                let property = &schema["properties"][key];
                schema_value_matches(property, item)
            })
        }
        _ => false,
    }
}

fn assistant_tool_message(text: &str, calls: &[(String, String, serde_json::Value)]) -> Message {
    let mut content = Vec::new();
    if !text.is_empty() {
        content.push(AssistantContent::text(text));
    }
    for (id, name, arguments) in calls {
        content.push(AssistantContent::tool_call(
            id.clone(),
            name.clone(),
            arguments.clone(),
        ));
    }
    Message::Assistant { id: None, content }
}

fn append_model_piece(
    reply: &mut AssistantReply,
    piece: &str,
    model_reply_bytes: &mut usize,
) -> bool {
    let remaining = MAXIMUM_MODEL_REPLY_BYTES.saturating_sub(*model_reply_bytes);
    if piece.len() <= remaining {
        reply.push_response(piece);
        *model_reply_bytes += piece.len();
        return false;
    }
    let mut end = remaining;
    while end > 0 && !piece.is_char_boundary(end) {
        end -= 1;
    }
    reply.push_response(&piece[..end]);
    *model_reply_bytes += end;
    true
}

fn append_thinking_piece(reply: &mut AssistantReply, piece: &str, thinking_bytes: &mut usize) {
    if piece.is_empty() {
        return;
    }
    let remaining = MAXIMUM_THINKING_BYTES.saturating_sub(*thinking_bytes);
    let mut end = piece.len().min(remaining);
    while end > 0 && !piece.is_char_boundary(end) {
        end -= 1;
    }
    reply.push_thinking(&piece[..end]);
    *thinking_bytes += end;
}

fn visible_tool_output(
    label: String,
    output: &str,
    command: Option<crate::execution::CommandResult>,
    visible_tool_bytes: &mut usize,
) -> Option<ToolOutput> {
    let mut label = label.replace('\0', "\u{fffd}");
    let output = output.replace('\0', "\u{fffd}");
    let remaining = MAXIMUM_VISIBLE_TOOL_BYTES.saturating_sub(*visible_tool_bytes);
    if remaining <= label.len() && command.is_none() {
        return None;
    }
    truncate_utf8(&mut label, remaining);
    *visible_tool_bytes += label.len();
    // Keep structured output before its duplicate plain-text projection.
    let command = command.map(|command| {
        let remaining = MAXIMUM_VISIBLE_TOOL_BYTES
            .saturating_sub(*visible_tool_bytes)
            .min(crate::execution::OUTPUT_PREVIEW_BYTES);
        let (bounded, _) = command.bounded(remaining);
        *visible_tool_bytes += bounded
            .chunks
            .iter()
            .map(|chunk| chunk.text.len())
            .sum::<usize>();
        bounded
    });
    let output_limit = MAXIMUM_VISIBLE_TOOL_BYTES.saturating_sub(*visible_tool_bytes);
    let visible = bound_visible_text(&output, output_limit);
    *visible_tool_bytes += visible.len();
    Some(ToolOutput {
        label,
        output: visible,
        command,
    })
}

fn bound_visible_text(output: &str, output_limit: usize) -> String {
    const MARKER: &str = "\n[output truncated]";
    if output.len() <= output_limit {
        output.to_owned()
    } else if output_limit > MARKER.len() {
        let mut end = output_limit - MARKER.len();
        while end > 0 && !output.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}{}", &output[..end], MARKER)
    } else {
        let mut end = output_limit;
        while end > 0 && !output.is_char_boundary(end) {
            end -= 1;
        }
        output[..end].to_owned()
    }
}

#[derive(Clone, Copy)]
enum OutputChannel {
    Response,
    Thinking,
}

#[derive(Default)]
struct ThinkingProgress {
    published: usize,
    phase_started: Option<Instant>,
    last_emit: Option<Instant>,
}

impl ThinkingProgress {
    fn begin_phase(&mut self) {
        self.phase_started = None;
        self.last_emit = None;
    }

    fn note_pending(&mut self, text: &str, now: Instant) {
        if self.published < text.len() && self.phase_started.is_none() {
            self.phase_started = Some(now);
        }
    }

    fn deadline(&self, text: &str) -> Option<Instant> {
        if self.published >= text.len() {
            return None;
        }
        self.last_emit
            .map(|last_emit| last_emit + THINKING_PROGRESS_INTERVAL)
            .or_else(|| {
                self.phase_started
                    .map(|started| started + THINKING_INITIAL_DELAY)
            })
    }

    fn publish_due(&mut self, job: &Job, text: &str, now: Instant) -> bool {
        if self.deadline(text).is_none_or(|deadline| now < deadline) {
            return false;
        }
        self.publish_next(job, text, now)
    }

    fn flush(&mut self, job: &Job, text: &str) {
        while self.published < text.len() {
            self.publish_next(job, text, Instant::now());
        }
    }

    fn publish_next(&mut self, job: &Job, text: &str, now: Instant) -> bool {
        let end = bounded_progress_end(text, self.published, MAXIMUM_THINKING_PROGRESS_BYTES);
        if end <= self.published {
            return false;
        }
        publish_range(job, text, self.published, end, OutputChannel::Thinking);
        self.published = end;
        self.last_emit = Some(now);
        true
    }
}

fn bounded_progress_end(text: &str, published: usize, maximum: usize) -> usize {
    let mut end = published.saturating_add(maximum).min(text.len());
    while end > published && !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

fn publish_progress(
    job: &Job,
    text: &str,
    published: &mut usize,
    channel: OutputChannel,
    last_emit: &mut Instant,
    output_visible: &mut bool,
) {
    if progress_due(*output_visible, *last_emit) {
        publish_remaining(job, text, *published, channel);
        *published = text.len();
        *output_visible = *published > 0;
        *last_emit = Instant::now();
    }
}

pub(crate) fn bound_reply(reply: &AssistantReply) -> AssistantReply {
    let mut bounded = reply.clone();
    // Live progress is not durable terminal content.
    bounded.progress.clear();
    truncate_utf8(&mut bounded.text, MAXIMUM_MODEL_REPLY_BYTES);
    if bounded.activity.is_empty() {
        truncate_utf8(&mut bounded.thinking, MAXIMUM_THINKING_BYTES);
        let mut tool_bytes = 0usize;
        bounded.tools.retain_mut(|tool| {
            let command = tool.command.take();
            let Some(visible) =
                visible_tool_output(tool.label.clone(), &tool.output, command, &mut tool_bytes)
            else {
                return false;
            };
            *tool = visible;
            true
        });
        return bounded;
    }

    bounded.thinking.clear();
    bounded.tools.clear();
    let mut activities = Vec::new();
    let mut thinking_bytes = 0usize;
    let mut tool_bytes = 0usize;
    let mut response_bytes = 0usize;
    let ordered_response = bounded
        .activity
        .iter()
        .any(|activity| matches!(activity, AssistantActivity::Response(_)));
    if ordered_response {
        bounded.text.clear();
    }
    for activity in std::mem::take(&mut bounded.activity).into_iter().take(256) {
        match activity {
            AssistantActivity::Response(mut text) => {
                truncate_utf8(
                    &mut text,
                    MAXIMUM_MODEL_REPLY_BYTES.saturating_sub(response_bytes),
                );
                response_bytes += text.len();
                bounded.text.push_str(&text);
                activities.push(AssistantActivity::Response(text));
            }
            AssistantActivity::ToolCall {
                id,
                name,
                arguments,
                result,
            } => {
                // Durable call results must not inherit the transcript preview limit.
                if let Some(tool) = &result {
                    bounded.tools.push(tool.clone());
                }
                activities.push(AssistantActivity::ToolCall {
                    id,
                    name,
                    arguments,
                    result,
                });
            }
            AssistantActivity::Thinking(mut thinking) => {
                let remaining = MAXIMUM_THINKING_BYTES.saturating_sub(thinking_bytes);
                truncate_utf8(&mut thinking, remaining);
                thinking_bytes += thinking.len();
                bounded.thinking.push_str(&thinking);
                activities.push(AssistantActivity::Thinking(thinking));
            }
            AssistantActivity::Tool(tool) => {
                let ToolOutput {
                    label,
                    output,
                    command,
                } = tool;
                if let Some(tool) = visible_tool_output(label, &output, command, &mut tool_bytes) {
                    bounded.tools.push(tool.clone());
                    activities.push(AssistantActivity::Tool(tool));
                }
            }
        }
    }
    bounded.activity = activities;
    bounded
}

fn persist_output(
    state: &AppState,
    conversation: Option<ConversationId>,
    job: &Job,
    reply: &AssistantReply,
    checkpoint: bool,
) -> Result<(), crate::conversations::ConversationError> {
    let Some(conversation) = conversation else {
        return Ok(());
    };
    if job.snapshot().owner != crate::sessions::JobOwner::Conversation(conversation)
        || reply.is_empty()
        || state.conversations.get(&conversation).is_none()
    {
        return Ok(());
    }
    if checkpoint {
        state
            .conversations
            .checkpoint_output(&conversation, job.id(), reply.clone())
    } else {
        state
            .conversations
            .append_output(&conversation, job.id(), reply.clone())
    }
}

fn take_steering(
    state: &AppState,
    spec: &AgentRunSpec,
    job: &Job,
    reply: &AssistantReply,
) -> Result<Option<String>, crate::conversations::ConversationError> {
    let Some(session) = spec.steering_session else {
        return Ok(None);
    };
    if !state.sessions.contains_live(&session) || job.cancel_requested() {
        return Ok(None);
    }
    if job.snapshot().status != crate::sessions::JobStatus::Running {
        return Ok(None);
    }
    let Some(conversation) = spec.conversation else {
        return Ok(None);
    };
    if !state
        .sessions
        .owns_conversation_job(&session, conversation, job.id())
    {
        return Ok(None);
    }
    let Some(record) = state.conversations.get(&conversation) else {
        return Ok(None);
    };
    let Some(item) = record.queue.next_steering(job.id()).cloned() else {
        return Ok(None);
    };
    let text =
        state
            .conversations
            .deliver_steering(&conversation, job.id(), item.id, reply.clone())?;
    job.restore_output(AssistantReply::default());
    Ok(Some(text))
}

fn reset_round(
    reply: &mut AssistantReply,
    model_reply_bytes: &mut usize,
    thinking_bytes: &mut usize,
    visible_tool_bytes: &mut usize,
    published_response: &mut usize,
    thinking_progress: &mut ThinkingProgress,
    output_visible: &mut bool,
) {
    *reply = AssistantReply::default();
    *model_reply_bytes = 0;
    *thinking_bytes = 0;
    *visible_tool_bytes = 0;
    *published_response = 0;
    *thinking_progress = ThinkingProgress::default();
    *output_visible = false;
}

fn store_failure(
    reply: &AssistantReply,
    error: crate::conversations::ConversationError,
) -> AgentActionEnd {
    AgentActionEnd {
        outcome: AgentOutcome::PersistenceFailure,
        error: Some(error.message().to_owned()),
        reply: reply.clone(),
    }
}

fn record_failed_attempt(
    state: &AppState,
    spec: &AgentRunSpec,
    job: &Job,
    reply: &AssistantReply,
    committed: &AssistantReply,
    error: &ProviderError,
) -> Result<(), crate::conversations::ConversationError> {
    let Some(conversation) = spec.conversation else {
        return Ok(());
    };
    let mut failed = AssistantReply::default();
    failed.push_response(reply.text.strip_prefix(&committed.text).unwrap_or_default());
    failed.push_thinking(
        reply
            .thinking
            .strip_prefix(&committed.thinking)
            .unwrap_or_default(),
    );
    state.conversations.record_provider_failure(
        &conversation,
        job.id(),
        failed,
        committed,
        error.message().to_owned(),
    )
}

fn take_retry(
    error: &ProviderError,
    attempts: &mut u32,
    waited: &mut Duration,
) -> Option<Duration> {
    if *attempts >= crate::providers::MAXIMUM_PROVIDER_RETRY_ATTEMPTS {
        return None;
    }
    let delay = error.bounded_retry_delay(*waited)?;
    *attempts += 1;
    *waited = waited.saturating_add(delay);
    Some(delay)
}

async fn wait_retry(job: &Job, delay: Duration) -> bool {
    if delay.is_zero() {
        return job.cancel_requested();
    }
    tokio::select! {
        biased;
        _ = job.cancelled() => true,
        _ = tokio::time::sleep(delay) => job.cancel_requested(),
    }
}

#[allow(clippy::too_many_arguments)]
fn restore_request(
    reply: &mut AssistantReply,
    committed: &AssistantReply,
    published_response: &mut usize,
    committed_response: usize,
    thinking_progress: &mut ThinkingProgress,
    committed_thinking: usize,
    model_reply_bytes: &mut usize,
    committed_model_bytes: usize,
    thinking_bytes: &mut usize,
    committed_thinking_bytes: usize,
    visible_tool_bytes: &mut usize,
    committed_visible_tool_bytes: usize,
    response_redactor: &mut StreamRedactor<'_>,
    thinking_redactor: &mut StreamRedactor<'_>,
    event_count: &mut usize,
    _secret: Option<&str>,
    job: &Job,
) {
    *reply = committed.clone();
    *published_response = committed_response;
    *thinking_progress = ThinkingProgress {
        published: committed_thinking,
        ..ThinkingProgress::default()
    };
    *model_reply_bytes = committed_model_bytes;
    *thinking_bytes = committed_thinking_bytes;
    *visible_tool_bytes = committed_visible_tool_bytes;
    let _ = response_redactor.finish();
    let _ = thinking_redactor.finish();
    *event_count = 0;
    job.restore_output(committed.clone());
}

fn outcome_for_tool(kind: tools::ToolFailureKind) -> AgentOutcome {
    match kind {
        tools::ToolFailureKind::Ordinary | tools::ToolFailureKind::Rejected => {
            AgentOutcome::ToolFailure
        }
        tools::ToolFailureKind::Authority => AgentOutcome::AuthorityFailure,
        tools::ToolFailureKind::Persistence => AgentOutcome::PersistenceFailure,
        tools::ToolFailureKind::Cancellation => AgentOutcome::Cancelled,
        tools::ToolFailureKind::Uncertain => AgentOutcome::UncertainEffect,
    }
}

fn record_not_dispatched(
    remaining: impl Iterator<Item = (String, String, serde_json::Value)>,
    secret: Option<&str>,
    reply: &mut AssistantReply,
    resolved_calls: &mut Vec<crate::providers::ChatToolCall>,
    extra: &mut Vec<Message>,
    job: &Job,
    visible_tool_bytes: &mut usize,
) {
    let command = crate::execution::CommandResult::new(
        Vec::new(),
        crate::execution::CommandTermination::NotDispatched,
    );
    let output = ToolOutput {
        label: "not dispatched".to_owned(),
        output: "This tool did not run.".to_owned(),
        command: Some(command),
    };
    for (id, name, arguments) in remaining {
        let visible_id = tools::redact(&id, secret);
        resolved_calls.push(crate::providers::ChatToolCall {
            id: id.clone(),
            name: name.clone(),
            arguments,
            result: Some(output.clone()),
        });
        reply.finish_tool(&visible_id, output.clone());
        extra.push(Message::tool_result(id, name, output.output.clone()));
        if let Some(visible) = visible_tool_output(
            output.label.clone(),
            &output.output,
            output.command.clone(),
            visible_tool_bytes,
        ) {
            job.finish_tool(visible_id, visible);
        }
    }
}

fn truncate_utf8(text: &mut String, maximum: usize) {
    if text.len() <= maximum {
        return;
    }
    let mut end = maximum;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

fn cancel_action(job: &Job, reply: &AssistantReply) -> AgentActionEnd {
    let published = job.snapshot().output;
    let mut thinking_progress = ThinkingProgress {
        published: published.thinking.len().min(reply.thinking.len()),
        ..ThinkingProgress::default()
    };
    thinking_progress.flush(job, &reply.thinking);
    publish_reply_remaining(
        job,
        reply,
        published.text.len().min(reply.text.len()),
        thinking_progress.published,
    );
    AgentActionEnd {
        outcome: AgentOutcome::Cancelled,
        error: None,
        reply: reply.clone(),
    }
}

fn publish_reply_before_tools(
    job: &Job,
    reply: &AssistantReply,
    published_response: &mut usize,
    thinking_progress: &mut ThinkingProgress,
) {
    thinking_progress.flush(job, &reply.thinking);
    publish_reply_remaining(job, reply, *published_response, thinking_progress.published);
    *published_response = reply.text.len();
}

fn publish_reply_remaining(
    job: &Job,
    reply: &AssistantReply,
    published_response: usize,
    published_thinking: usize,
) {
    publish_remaining(
        job,
        &reply.text,
        published_response,
        OutputChannel::Response,
    );
    publish_remaining(
        job,
        &reply.thinking,
        published_thinking,
        OutputChannel::Thinking,
    );
}

fn publish_remaining(job: &Job, text: &str, published: usize, channel: OutputChannel) {
    publish_range(job, text, published, text.len(), channel);
}

fn publish_range(job: &Job, text: &str, published: usize, end: usize, channel: OutputChannel) {
    if published >= end || end > text.len() {
        return;
    }
    let delta = text[published..end].to_owned();
    match channel {
        OutputChannel::Response => {
            let _ = job.push_response(delta);
        }
        OutputChannel::Thinking => {
            let _ = job.push_thinking(delta);
        }
    }
}

fn progress_due(output_visible: bool, last_emit: Instant) -> bool {
    !output_visible || last_emit.elapsed() >= MIN_PROGRESS_INTERVAL
}
