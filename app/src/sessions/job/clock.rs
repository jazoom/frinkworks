use std::sync::Arc;
use std::time::{Duration, Instant};

use super::JobId;
use crate::conversations::{ConversationId, ConversationStore};

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct WorkClock {
    elapsed: Duration,
    active_since: Option<Instant>,
    checkpoint: Option<Instant>,
    store: Option<(Arc<ConversationStore>, ConversationId, JobId)>,
}

pub(super) struct WorkCheckpoint {
    store: Arc<ConversationStore>,
    conversation: ConversationId,
    job: JobId,
    elapsed_ms: u64,
}

impl WorkClock {
    pub(super) fn track(
        &mut self,
        store: Arc<ConversationStore>,
        conversation: ConversationId,
        job: JobId,
    ) {
        self.store = Some((store, conversation, job));
    }

    pub(super) fn elapsed_ms(&self) -> Option<u64> {
        self.checkpoint?;
        let elapsed = self.elapsed
            + self
                .active_since
                .map_or(Duration::ZERO, |start| start.elapsed());
        Some(elapsed.as_millis().min(u128::from(u64::MAX)) as u64)
    }

    pub(super) fn pause(&mut self) {
        if let Some(start) = self.active_since.take() {
            self.elapsed += start.elapsed();
        }
    }

    pub(super) fn resume(&mut self) {
        let now = Instant::now();
        self.checkpoint.get_or_insert(now);
        self.active_since.get_or_insert(now);
    }

    pub(super) fn checkpoint(&mut self, force: bool) -> Option<WorkCheckpoint> {
        let checkpoint = self.checkpoint?;
        if !force && (self.active_since.is_none() || checkpoint.elapsed() < Duration::from_secs(5))
        {
            return None;
        }
        let (store, conversation, job) = self.store.as_ref()?;
        let saved = WorkCheckpoint {
            store: store.clone(),
            conversation: *conversation,
            job: *job,
            elapsed_ms: self.elapsed_ms()?,
        };
        self.checkpoint = Some(Instant::now());
        Some(saved)
    }
}

impl WorkCheckpoint {
    // Persistence must not hold a session, job or question mutex. Those locks
    // also occur on paths that acquire the conversation database first.
    pub(super) fn save(self) {
        if let Err(error) =
            self.store
                .record_work_time(&self.conversation, self.job, self.elapsed_ms)
        {
            tracing::warn!(%error, "Frinkworks did not save the active-work duration.");
        }
    }
}
