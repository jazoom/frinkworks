//! Server-owned chat jobs. Events keep a monotonic sequence for observation.

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rand::rand_core::TryRng;
use rand::rngs::SysRng;
use tokio::sync::Notify;

use crate::conversations::ConversationId;
use crate::conversations::RequestUsage;
use crate::hex;
use crate::providers::{AssistantReply, ToolOutput};

/// Live progress is bounded separately from terminal tool results.
pub(crate) const MAXIMUM_TOOL_PROGRESS_EVENTS: usize = 256;
pub(crate) const MAXIMUM_TOOL_PROGRESS_BYTES: usize = 32 * 1024;

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub(crate) struct JobId([u8; 16]);

impl JobId {
    pub(crate) fn generate() -> Result<Self, JobIdError> {
        let mut bytes = [0u8; 16];
        SysRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| JobIdError::RandomUnavailable)?;
        Ok(Self(bytes))
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        hex::decode(value).map(Self)
    }

    pub(crate) fn as_hex(&self) -> String {
        hex::encode(&self.0)
    }
}

impl std::fmt::Display for JobId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.as_hex())
    }
}

impl std::fmt::Debug for JobId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JobId(")?;
        formatter.write_str(&self.as_hex())?;
        formatter.write_str(")")
    }
}

#[derive(Debug)]
pub(crate) enum JobIdError {
    RandomUnavailable,
}

impl std::fmt::Display for JobIdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("system random source unavailable")
    }
}

impl std::error::Error for JobIdError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum JobOwner {
    Conversation(ConversationId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum JobStatus {
    Running,
    AwaitingDecision,
    AwaitingQuestion,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum JobEventKind {
    Response {
        delta: String,
    },
    Thinking {
        delta: String,
    },
    ToolStarted {
        id: String,
        name: String,
        #[allow(dead_code)]
        arguments: serde_json::Value,
    },
    ToolProgress {
        id: String,
        stream: crate::execution::CommandStream,
        delta: String,
    },
    ToolFinished {
        id: String,
        output: ToolOutput,
    },
    Usage {
        usage: RequestUsage,
    },
    Retrying {
        attempt: u32,
        delay: Duration,
        reason: String,
    },
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JobEvent {
    pub(crate) seq: u64,
    pub(crate) kind: JobEventKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JobRetry {
    pub(crate) attempt: u32,
    pub(crate) delay: Duration,
    pub(crate) reason: String,
}

#[derive(Clone, Debug)]
pub(crate) struct JobSnapshot {
    pub(crate) id: JobId,
    pub(crate) owner: JobOwner,
    pub(crate) status: JobStatus,
    pub(crate) output: AssistantReply,
    pub(crate) latest_seq: u64,
    pub(crate) retry: Option<JobRetry>,
}

pub(crate) struct Job {
    id: JobId,
    owner: JobOwner,
    inner: Mutex<JobInner>,
    notify: Notify,
    cancel: AtomicBool,
    output_visible: AtomicBool,
}

struct JobInner {
    status: JobStatus,
    events: Vec<JobEvent>,
    output: AssistantReply,
    output_base: AssistantReply,
    latest_seq: u64,
    error: Option<String>,
    progress_events: usize,
    progress_bytes: usize,
    progress_truncated: bool,
    retry: Option<JobRetry>,
}

impl Job {
    pub(crate) fn for_conversation(id: JobId, conversation: ConversationId) -> Arc<Self> {
        Self::with_owner(id, JobOwner::Conversation(conversation))
    }

    fn with_owner(id: JobId, owner: JobOwner) -> Arc<Self> {
        Arc::new(Self {
            id,
            owner,
            inner: Mutex::new(JobInner {
                status: JobStatus::Running,
                events: Vec::new(),
                output: AssistantReply::default(),
                output_base: AssistantReply::default(),
                latest_seq: 0,
                error: None,
                progress_events: 0,
                progress_bytes: 0,
                progress_truncated: false,
                retry: None,
            }),
            notify: Notify::new(),
            cancel: AtomicBool::new(false),
            output_visible: AtomicBool::new(true),
        })
    }

    pub(crate) fn id(&self) -> JobId {
        self.id
    }

    pub(crate) fn is_running(&self) -> bool {
        self.lock().status == JobStatus::Running
    }

    pub(crate) fn set_awaiting_decision(&self) -> Option<u64> {
        self.set_awaiting(JobStatus::AwaitingDecision)
    }

    pub(crate) fn set_awaiting_question(&self) -> Option<u64> {
        self.set_awaiting(JobStatus::AwaitingQuestion)
    }

    fn set_awaiting(&self, status: JobStatus) -> Option<u64> {
        if !matches!(
            status,
            JobStatus::AwaitingDecision | JobStatus::AwaitingQuestion
        ) {
            return None;
        }
        let mut inner = self.lock();
        if inner.status != JobStatus::Running {
            return None;
        }
        inner.status = status;
        inner.latest_seq += 1;
        let seq = inner.latest_seq;
        inner.events.push(JobEvent {
            seq,
            kind: JobEventKind::Completed,
        });
        drop(inner);
        self.notify.notify_waiters();
        Some(seq)
    }

    pub(crate) fn resume(&self) -> bool {
        let mut inner = self.lock();
        if !matches!(
            inner.status,
            JobStatus::AwaitingDecision | JobStatus::AwaitingQuestion
        ) {
            return false;
        }
        inner.status = JobStatus::Running;
        inner.error = None;
        drop(inner);
        self.notify.notify_waiters();
        true
    }

    pub(crate) fn set_output_visible(&self, visible: bool) {
        self.output_visible.store(visible, Ordering::SeqCst);
    }

    pub(crate) fn cancel_requested(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    pub(crate) fn request_cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub(crate) async fn cancelled(&self) {
        loop {
            if self.cancel_requested() {
                return;
            }
            let notified = self.notify.notified();
            if self.cancel_requested() {
                return;
            }
            notified.await;
        }
    }

    pub(crate) fn snapshot(&self) -> JobSnapshot {
        let inner = self.lock();
        JobSnapshot {
            id: self.id,
            owner: self.owner,
            status: inner.status,
            output: inner.output.clone(),
            latest_seq: inner.latest_seq,
            retry: inner.retry.clone(),
        }
    }

    pub(crate) fn latest_seq(&self) -> u64 {
        self.lock().latest_seq
    }

    pub(crate) fn output_up_to(&self, cursor: u64) -> AssistantReply {
        let inner = self.lock();
        if cursor >= inner.latest_seq {
            return inner.output.clone();
        }
        let mut output = inner.output_base.clone();
        for event in &inner.events {
            if event.seq > cursor {
                break;
            }
            apply_output_event(&mut output, &event.kind);
        }
        output
    }

    pub(crate) fn push_response(&self, delta: String) -> Option<u64> {
        self.push_output_event(JobEventKind::Response { delta })
    }

    pub(crate) fn push_thinking(&self, delta: String) -> Option<u64> {
        self.push_output_event(JobEventKind::Thinking { delta })
    }

    pub(crate) fn start_tool(
        &self,
        id: String,
        name: String,
        arguments: serde_json::Value,
    ) -> Option<u64> {
        self.push_output_event(JobEventKind::ToolStarted {
            id,
            name,
            arguments,
        })
    }

    /// Publish a bounded slice of live command output. The final tool result
    /// replaces progress, so dropping progress never loses the terminal status.
    pub(crate) fn push_tool_progress(
        &self,
        id: String,
        stream: crate::execution::CommandStream,
        delta: String,
    ) -> Option<u64> {
        if delta.is_empty() || !self.output_visible.load(Ordering::SeqCst) {
            return None;
        }
        let mut inner = self.lock();
        if inner.status != JobStatus::Running || inner.progress_truncated {
            return None;
        }
        if inner.progress_events >= MAXIMUM_TOOL_PROGRESS_EVENTS
            || inner.progress_bytes.saturating_add(delta.len()) > MAXIMUM_TOOL_PROGRESS_BYTES
        {
            inner.progress_truncated = true;
            return None;
        }
        inner.progress_events += 1;
        inner.progress_bytes += delta.len();
        inner.latest_seq += 1;
        let seq = inner.latest_seq;
        let kind = JobEventKind::ToolProgress { id, stream, delta };
        apply_output_event(&mut inner.output, &kind);
        inner.events.push(JobEvent { seq, kind });
        drop(inner);
        self.notify.notify_waiters();
        Some(seq)
    }

    pub(crate) fn finish_tool(&self, id: String, output: ToolOutput) -> Option<u64> {
        self.push_output_event(JobEventKind::ToolFinished { id, output })
    }

    pub(crate) fn push_usage(&self, usage: RequestUsage) -> Option<u64> {
        self.push_output_event(JobEventKind::Usage { usage })
    }

    pub(crate) fn restore_output(&self, output: AssistantReply) {
        let mut inner = self.lock();
        if inner.status != JobStatus::Running {
            return;
        }
        let output = if self.output_visible.load(Ordering::SeqCst) {
            output
        } else {
            AssistantReply::default()
        };
        // Old cursors must not reconstruct a failed attempt as successful output.
        inner.output_base = output.clone();
        inner.output = output;
        inner.events.clear();
        inner.latest_seq += 1;
        inner.retry = None;
        drop(inner);
        self.notify.notify_waiters();
    }

    pub(crate) fn set_retry(&self, attempt: u32, delay: Duration, reason: &str) -> Option<u64> {
        self.push_output_event(JobEventKind::Retrying {
            attempt,
            delay,
            reason: reason.to_owned(),
        })
    }

    pub(crate) fn clear_retry(&self) {
        let mut inner = self.lock();
        if inner.retry.take().is_some() {
            inner.latest_seq += 1;
            drop(inner);
            self.notify.notify_waiters();
        }
    }

    fn push_output_event(&self, kind: JobEventKind) -> Option<u64> {
        let empty = match &kind {
            JobEventKind::Response { delta } => delta.is_empty(),
            JobEventKind::Thinking { .. }
            | JobEventKind::ToolStarted { .. }
            | JobEventKind::ToolProgress { .. }
            | JobEventKind::ToolFinished { .. }
            | JobEventKind::Usage { .. }
            | JobEventKind::Retrying { .. } => false,
            JobEventKind::Completed | JobEventKind::Failed | JobEventKind::Cancelled => true,
        };
        if empty
            || (!self.output_visible.load(Ordering::SeqCst)
                && !matches!(kind, JobEventKind::Retrying { .. }))
        {
            return None;
        }
        let mut inner = self.lock();
        if inner.status != JobStatus::Running {
            return None;
        }
        inner.latest_seq += 1;
        let seq = inner.latest_seq;
        if let JobEventKind::Retrying {
            attempt,
            delay,
            reason,
        } = &kind
        {
            inner.retry = Some(JobRetry {
                attempt: *attempt,
                delay: *delay,
                reason: reason.clone(),
            });
        }
        apply_output_event(&mut inner.output, &kind);
        inner.events.push(JobEvent { seq, kind });
        drop(inner);
        self.notify.notify_waiters();
        Some(seq)
    }

    pub(crate) fn finish(&self, status: JobStatus, error: Option<&str>) -> Option<u64> {
        if !matches!(
            status,
            JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled
        ) {
            return None;
        }
        let mut inner = self.lock();
        if !matches!(
            inner.status,
            JobStatus::Running | JobStatus::AwaitingDecision | JobStatus::AwaitingQuestion
        ) {
            return None;
        }
        inner.status = status;
        inner.retry = None;
        inner.error = error.map(str::to_owned);
        inner.latest_seq += 1;
        let seq = inner.latest_seq;
        inner.events.push(JobEvent {
            seq,
            kind: match status {
                JobStatus::Completed => JobEventKind::Completed,
                JobStatus::Failed => JobEventKind::Failed,
                JobStatus::Cancelled => JobEventKind::Cancelled,
                JobStatus::Running | JobStatus::AwaitingDecision | JobStatus::AwaitingQuestion => {
                    unreachable!("terminal status checked above")
                }
            },
        });
        drop(inner);
        self.notify.notify_waiters();
        Some(seq)
    }

    pub(crate) async fn wait_after(&self, cursor: u64, timeout: Duration) {
        if timeout.is_zero() {
            return;
        }
        let deadline = Instant::now() + timeout;
        loop {
            if self.latest_seq() > cursor || !self.is_running() {
                return;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return;
            }
            let notified = self.notify.notified();
            if self.latest_seq() > cursor || !self.is_running() {
                return;
            }
            if tokio::time::timeout(remaining, notified).await.is_err() {
                return;
            }
        }
    }

    pub(crate) async fn wait_for_terminal(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if matches!(
                self.snapshot().status,
                JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled
            ) {
                return true;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let notified = self.notify.notified();
            if matches!(
                self.snapshot().status,
                JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled
            ) {
                return true;
            }
            if tokio::time::timeout(remaining, notified).await.is_err() {
                return false;
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, JobInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn apply_output_event(output: &mut AssistantReply, event: &JobEventKind) {
    match event {
        JobEventKind::Response { delta } => output.push_response(delta),
        JobEventKind::Thinking { delta } => output.push_thinking(delta),
        JobEventKind::ToolStarted {
            id,
            name,
            arguments,
        } => output.start_tool(id.clone(), name.clone(), arguments.clone()),
        JobEventKind::ToolProgress { id, stream, delta } => {
            output.push_tool_progress(id, *stream, delta)
        }
        JobEventKind::ToolFinished { id, output: tool } => output.finish_tool(id, tool.clone()),
        JobEventKind::Usage { usage } => {
            if let Some(existing) = output
                .usage
                .iter_mut()
                .find(|existing| existing.id == usage.id)
            {
                *existing = usage.clone();
            } else {
                output.usage.push(usage.clone());
            }
        }
        JobEventKind::Retrying { .. }
        | JobEventKind::Completed
        | JobEventKind::Failed
        | JobEventKind::Cancelled => {}
    }
}
