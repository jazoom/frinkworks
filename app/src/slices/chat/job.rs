use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;

use rig_core::completion::{AssistantContent, Message};

use crate::{
    agents::{AgentId, DirectoryPolicy, ToolId},
    execution::ToolLocation,
    providers::{
        AssistantActivity, AssistantReply, ChatTurn, ModelEvent, ModelUsage, ProviderConnection,
        ProviderError, ToolOutput,
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

    for _ in 0..MAXIMUM_TOOL_ROUNDS {
        thinking_progress.begin_phase();
        if job.cancel_requested() {
            return cancel_action(&job, &reply);
        }
        let mut events = tokio::select! {
            biased;
            _ = job.cancelled() => {
                return cancel_action(&job, &reply);
            }
            result = state.chat.stream_turn(
                &spec.connection,
                &turns,
                &extra,
                &spec.tools,
                &spec.preamble,
            ) => match result {
                Ok(stream) => stream,
                Err(error) => {
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

        let mut text = String::new();
        let mut calls = Vec::new();
        loop {
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
                _ = wait_for_thinking => {
                    thinking_progress.publish_due(&job, &reply.thinking, Instant::now());
                    continue;
                }
                chunk = events.next() => chunk,
            };
            let Some(chunk) = chunk else {
                break;
            };
            match chunk {
                Ok(ModelEvent::Text(piece)) => {
                    thinking_progress.flush(&job, &reply.thinking);
                    let piece = response_redactor.push(&piece);
                    if let Some(evidence) = &spec.evidence {
                        evidence.response(&piece, secret);
                    }
                    text.push_str(&piece);
                    let truncated =
                        append_model_piece(&mut reply.text, &piece, &mut model_reply_bytes);
                    publish_progress(
                        &job,
                        &reply.text,
                        &mut published_response,
                        OutputChannel::Response,
                        &mut last_emit,
                        &mut output_visible,
                    );
                    if truncated {
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
                }) => calls.push((id, name, arguments)),
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
                Err(error) => {
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
                append_model_piece(&mut reply.text, &response_tail, &mut model_reply_bytes);
            append_thinking_piece(&mut reply, &thinking_tail, &mut thinking_bytes);
            thinking_progress.flush(&job, &reply.thinking);
            publish_reply_remaining(
                &job,
                &reply,
                published_response,
                thinking_progress.published,
            );
            if truncated || reply.text.trim().is_empty() {
                return AgentActionEnd {
                    outcome: AgentOutcome::ProviderFailure,
                    error: Some(
                        if truncated {
                            ProviderError::ReplyTooLong
                        } else {
                            ProviderError::EmptyReply
                        }
                        .message()
                        .to_owned(),
                    ),
                    reply: reply.clone(),
                };
            }
            return AgentActionEnd {
                outcome: AgentOutcome::Completed,
                error: None,
                reply: reply.clone(),
            };
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
            output_drafts: spec.output_drafts.as_deref(),
            required_outputs: &spec.required_outputs,
        };
        for (id, name, arguments) in calls {
            let trace = tools::invoke(&context, &name, &arguments).await;
            if job.cancel_requested() {
                return cancel_action(&job, &reply);
            }
            let output = tools::redact(&trace.output, secret);
            if let Some(evidence) = &spec.evidence {
                evidence.tool(
                    &ToolOutput {
                        label: trace.label.clone(),
                        output: output.clone(),
                    },
                    secret,
                );
            }
            if let Some(visible) =
                visible_tool_output(trace.label, &output, &mut visible_tool_bytes)
            {
                job.push_tool(visible.clone());
                reply.push_tool(visible);
                output_visible = true;
            }
            if trace.failed && spec.host.as_ref().is_some_and(|host| host.run.is_some()) {
                return AgentActionEnd {
                    outcome: AgentOutcome::ToolFailure,
                    error: Some(output),
                    reply: reply.clone(),
                };
            }
            extra.push(Message::tool_result(id, name, output));
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
struct StreamRedactor<'a> {
    secret: Option<&'a str>,
    pending: String,
}

impl<'a> StreamRedactor<'a> {
    fn new(secret: Option<&'a str>) -> Self {
        Self {
            secret: secret.filter(|secret| !secret.is_empty()),
            pending: String::new(),
        }
    }

    fn push(&mut self, piece: &str) -> String {
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

    fn finish(&mut self) -> String {
        std::mem::take(&mut self.pending)
    }
}

const TOOL_LOOP_LIMIT: &str = "The agent stopped after too many tool calls. Try again.";

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

fn append_model_piece(reply: &mut String, piece: &str, model_reply_bytes: &mut usize) -> bool {
    let remaining = MAXIMUM_MODEL_REPLY_BYTES.saturating_sub(*model_reply_bytes);
    if piece.len() <= remaining {
        reply.push_str(piece);
        *model_reply_bytes += piece.len();
        return false;
    }
    let mut end = remaining;
    while end > 0 && !piece.is_char_boundary(end) {
        end -= 1;
    }
    reply.push_str(&piece[..end]);
    *model_reply_bytes += end;
    true
}

fn append_thinking_piece(reply: &mut AssistantReply, piece: &str, thinking_bytes: &mut usize) {
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
    visible_tool_bytes: &mut usize,
) -> Option<ToolOutput> {
    const MARKER: &str = "\n[output truncated]";
    let remaining = MAXIMUM_VISIBLE_TOOL_BYTES.saturating_sub(*visible_tool_bytes);
    let output_limit = remaining.checked_sub(label.len())?;
    if output_limit == 0 {
        return None;
    }
    let visible = if output.len() <= output_limit {
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
    };
    *visible_tool_bytes += label.len().saturating_add(visible.len());
    Some(ToolOutput {
        label,
        output: visible,
    })
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
    truncate_utf8(&mut bounded.text, MAXIMUM_MODEL_REPLY_BYTES);
    if bounded.activity.is_empty() {
        truncate_utf8(&mut bounded.thinking, MAXIMUM_THINKING_BYTES);
        let mut tool_bytes = 0usize;
        bounded.tools.retain_mut(|tool| {
            let Some(visible) =
                visible_tool_output(tool.label.clone(), &tool.output, &mut tool_bytes)
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
    for activity in std::mem::take(&mut bounded.activity) {
        match activity {
            AssistantActivity::Thinking(mut thinking) => {
                let remaining = MAXIMUM_THINKING_BYTES.saturating_sub(thinking_bytes);
                truncate_utf8(&mut thinking, remaining);
                if !thinking.is_empty() {
                    thinking_bytes += thinking.len();
                    bounded.thinking.push_str(&thinking);
                    activities.push(AssistantActivity::Thinking(thinking));
                }
            }
            AssistantActivity::Tool(tool) => {
                let ToolOutput { label, output } = tool;
                if let Some(tool) = visible_tool_output(label, &output, &mut tool_bytes) {
                    bounded.tools.push(tool.clone());
                    activities.push(AssistantActivity::Tool(tool));
                }
            }
        }
    }
    bounded.activity = activities;
    bounded
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
