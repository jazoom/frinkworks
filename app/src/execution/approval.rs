use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
};

use tokio::sync::Notify;

use rand::{rand_core::TryRng, rngs::SysRng};

use crate::{
    conversations::ConversationId,
    sessions::{Job, JobId, SessionId},
};

const MAXIMUM_PENDING: usize = 1_024;
const MAXIMUM_EXPLANATION_BYTES: usize = 4_096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HostCommandDecision {
    Approved,
    Rejected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ApprovalError {
    Random,
    Invalid,
    Duplicate,
    Cancelled,
}

impl ApprovalError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Random => "Frinkworks could not create a command approval. Try again.",
            Self::Invalid => "That command approval is not valid.",
            Self::Duplicate => "That command was already decided.",
            Self::Cancelled => "Stopped.",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HostCommandRequest {
    pub(crate) token: String,
    pub(crate) session: SessionId,
    pub(crate) job: JobId,
    pub(crate) conversation: ConversationId,
    pub(crate) execution_revision: u32,
    pub(crate) command: String,
    pub(crate) directory: PathBuf,
    pub(crate) explanation: String,
    pub(crate) run: Option<String>,
    pub(crate) step: Option<String>,
    pub(crate) attempt: Option<String>,
}

struct PendingHostCommand {
    request: HostCommandRequest,
    decision: Mutex<Option<HostCommandDecision>>,
    notify: Notify,
    invalidated: AtomicBool,
    claimed: AtomicBool,
}

#[derive(Default)]
pub(crate) struct HostApprovalStore {
    pending: Mutex<HashMap<String, Arc<PendingHostCommand>>>,
    inflight: Mutex<HashMap<String, Arc<PendingHostCommand>>>,
    consumed: Mutex<HashSet<String>>,
}

impl HostApprovalStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn submit(&self, mut request: HostCommandRequest) -> Result<String, ApprovalError> {
        if request.command.is_empty() || request.command.contains('\0') {
            return Err(ApprovalError::Invalid);
        }
        request.explanation = bound_explanation(&request.explanation);
        let token = fresh_token()?;
        request.token = token.clone();
        let job = request.job;
        let pending = Arc::new(PendingHostCommand {
            request,
            decision: Mutex::new(None),
            notify: Notify::new(),
            invalidated: AtomicBool::new(false),
            claimed: AtomicBool::new(false),
        });
        let mut map = lock(&self.pending);
        let mut inflight = lock(&self.inflight);
        if inflight.values().any(|stored| stored.request.job == job) {
            return Err(ApprovalError::Invalid);
        }
        if map.len() >= MAXIMUM_PENDING || inflight.len() >= MAXIMUM_PENDING {
            return Err(ApprovalError::Invalid);
        }
        map.insert(token.clone(), pending.clone());
        inflight.insert(token.clone(), pending);
        Ok(token)
    }

    pub(crate) fn pending_for(
        &self,
        conversation: ConversationId,
        job: JobId,
    ) -> Option<HostCommandRequest> {
        lock(&self.pending)
            .values()
            .find(|pending| {
                pending.request.conversation == conversation && pending.request.job == job
            })
            .map(|pending| pending.request.clone())
    }

    pub(crate) fn decide(
        &self,
        request: &HostCommandRequest,
        decision: HostCommandDecision,
    ) -> Result<(), ApprovalError> {
        let mut consumed = lock(&self.consumed);
        if consumed.contains(&request.token) {
            return Err(ApprovalError::Duplicate);
        }
        let mut map = lock(&self.pending);
        let Some(pending) = map.remove(&request.token) else {
            return Err(ApprovalError::Invalid);
        };
        if pending.request != *request {
            map.insert(request.token.clone(), pending);
            return Err(ApprovalError::Invalid);
        }
        if consumed.len() >= MAXIMUM_PENDING {
            // Evicted tokens still fail because no pending request accepts them.
            consumed.clear();
        }
        consumed.insert(request.token.clone());
        drop(consumed);
        *lock(&pending.decision) = Some(decision);
        pending.notify.notify_one();
        Ok(())
    }

    pub(crate) async fn wait(
        &self,
        token: &str,
        job: &Job,
    ) -> Result<HostCommandDecision, ApprovalError> {
        let pending = lock(&self.inflight)
            .get(token)
            .cloned()
            .ok_or(ApprovalError::Invalid)?;
        if pending.claimed.swap(true, Ordering::AcqRel) {
            return Err(ApprovalError::Duplicate);
        }
        loop {
            if job.cancel_requested() || pending.invalidated.load(Ordering::Acquire) {
                self.invalidate_job(job.id());
                return Err(ApprovalError::Cancelled);
            }
            if let Some(decision) = *lock(&pending.decision) {
                lock(&self.inflight).remove(token);
                return Ok(decision);
            }
            tokio::select! {
                biased;
                _ = job.cancelled() => {
                    self.invalidate_job(job.id());
                    return Err(ApprovalError::Cancelled);
                }
                _ = pending.notify.notified() => {}
            }
        }
    }

    pub(crate) fn invalidate_job(&self, job: JobId) {
        let drop_job = |pending: &Arc<PendingHostCommand>| {
            if pending.request.job == job {
                pending.invalidated.store(true, Ordering::Release);
                pending.notify.notify_one();
                false
            } else {
                true
            }
        };
        lock(&self.pending).retain(|_, pending| drop_job(pending));
        lock(&self.inflight).retain(|_, pending| drop_job(pending));
    }

    pub(crate) fn invalidate_conversation(&self, conversation: ConversationId) {
        let drop_conversation = |pending: &Arc<PendingHostCommand>| {
            if pending.request.conversation == conversation {
                pending.invalidated.store(true, Ordering::Release);
                pending.notify.notify_one();
                false
            } else {
                true
            }
        };
        lock(&self.pending).retain(|_, pending| drop_conversation(pending));
        lock(&self.inflight).retain(|_, pending| drop_conversation(pending));
    }

    pub(crate) fn retain_sessions(&self, live: impl Fn(&SessionId) -> bool) {
        let drop_session = |pending: &Arc<PendingHostCommand>| {
            if live(&pending.request.session) {
                true
            } else {
                pending.invalidated.store(true, Ordering::Release);
                pending.notify.notify_one();
                false
            }
        };
        lock(&self.pending).retain(|_, pending| drop_session(pending));
        lock(&self.inflight).retain(|_, pending| drop_session(pending));
    }
}

pub(crate) fn command_token() -> Result<String, ApprovalError> {
    fresh_token()
}

fn fresh_token() -> Result<String, ApprovalError> {
    let mut bytes = [0_u8; 32];
    SysRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| ApprovalError::Random)?;
    Ok(crate::hex::encode(&bytes))
}

fn bound_explanation(explanation: &str) -> String {
    let mut end = explanation.len().min(MAXIMUM_EXPLANATION_BYTES);
    while end > 0 && !explanation.is_char_boundary(end) {
        end -= 1;
    }
    explanation[..end]
        .chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .collect()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests;
