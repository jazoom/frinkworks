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
use crate::hex;
use crate::providers::{AssistantReply, ModelUsage, ToolOutput};

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
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum JobEventKind {
    Response { delta: String },
    Thinking { delta: String },
    ToolStarted { id: String, name: String },
    ToolFinished { id: String, output: ToolOutput },
    Usage { usage: ModelUsage },
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JobEvent {
    pub(crate) seq: u64,
    pub(crate) kind: JobEventKind,
}

#[derive(Clone, Debug)]
pub(crate) struct JobSnapshot {
    pub(crate) id: JobId,
    pub(crate) owner: JobOwner,
    pub(crate) status: JobStatus,
    pub(crate) output: AssistantReply,
    pub(crate) latest_seq: u64,
    pub(crate) assistant_index: usize,
}

pub(crate) struct Job {
    id: JobId,
    owner: JobOwner,
    assistant_index: usize,
    inner: Mutex<JobInner>,
    notify: Notify,
    cancel: AtomicBool,
    output_visible: AtomicBool,
}

struct JobInner {
    status: JobStatus,
    events: Vec<JobEvent>,
    output: AssistantReply,
    latest_seq: u64,
    error: Option<String>,
}

impl Job {
    pub(crate) fn for_conversation(
        id: JobId,
        conversation: ConversationId,
        assistant_index: usize,
    ) -> Arc<Self> {
        Self::with_owner(id, JobOwner::Conversation(conversation), assistant_index)
    }

    fn with_owner(id: JobId, owner: JobOwner, assistant_index: usize) -> Arc<Self> {
        Arc::new(Self {
            id,
            owner,
            assistant_index,
            inner: Mutex::new(JobInner {
                status: JobStatus::Running,
                events: Vec::new(),
                output: AssistantReply::default(),
                latest_seq: 0,
                error: None,
            }),
            notify: Notify::new(),
            cancel: AtomicBool::new(false),
            output_visible: AtomicBool::new(true),
        })
    }

    pub(crate) fn id(&self) -> JobId {
        self.id
    }

    pub(crate) fn assistant_index(&self) -> usize {
        self.assistant_index
    }

    pub(crate) fn is_running(&self) -> bool {
        self.lock().status == JobStatus::Running
    }

    pub(crate) fn set_awaiting_decision(&self) -> Option<u64> {
        let mut inner = self.lock();
        if inner.status != JobStatus::Running {
            return None;
        }
        inner.status = JobStatus::AwaitingDecision;
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
        if inner.status != JobStatus::AwaitingDecision {
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
            assistant_index: self.assistant_index,
        }
    }

    pub(crate) fn latest_seq(&self) -> u64 {
        self.lock().latest_seq
    }

    pub(crate) fn output_up_to(&self, cursor: u64) -> AssistantReply {
        let inner = self.lock();
        let mut output = AssistantReply::default();
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

    pub(crate) fn start_tool(&self, id: String, name: String) -> Option<u64> {
        self.push_output_event(JobEventKind::ToolStarted { id, name })
    }

    pub(crate) fn finish_tool(&self, id: String, output: ToolOutput) -> Option<u64> {
        self.push_output_event(JobEventKind::ToolFinished { id, output })
    }

    pub(crate) fn push_usage(&self, usage: ModelUsage) -> Option<u64> {
        self.push_output_event(JobEventKind::Usage { usage })
    }

    fn push_output_event(&self, kind: JobEventKind) -> Option<u64> {
        let empty = match &kind {
            JobEventKind::Response { delta } => delta.is_empty(),
            JobEventKind::Thinking { .. }
            | JobEventKind::ToolStarted { .. }
            | JobEventKind::ToolFinished { .. }
            | JobEventKind::Usage { .. } => false,
            JobEventKind::Completed | JobEventKind::Failed | JobEventKind::Cancelled => true,
        };
        if empty || !self.output_visible.load(Ordering::SeqCst) {
            return None;
        }
        let mut inner = self.lock();
        if inner.status != JobStatus::Running {
            return None;
        }
        inner.latest_seq += 1;
        let seq = inner.latest_seq;
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
            JobStatus::Running | JobStatus::AwaitingDecision
        ) {
            return None;
        }
        inner.status = status;
        inner.error = error.map(str::to_owned);
        inner.latest_seq += 1;
        let seq = inner.latest_seq;
        inner.events.push(JobEvent {
            seq,
            kind: match status {
                JobStatus::Completed => JobEventKind::Completed,
                JobStatus::Failed => JobEventKind::Failed,
                JobStatus::Cancelled => JobEventKind::Cancelled,
                JobStatus::Running | JobStatus::AwaitingDecision => {
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
        JobEventKind::ToolStarted { id, name } => output.start_tool(id.clone(), name.clone()),
        JobEventKind::ToolFinished { id, output: tool } => output.finish_tool(id, tool.clone()),
        JobEventKind::Usage { usage } => output.usage = Some(usage.clone()),
        JobEventKind::Completed | JobEventKind::Failed | JobEventKind::Cancelled => {}
    }
}
