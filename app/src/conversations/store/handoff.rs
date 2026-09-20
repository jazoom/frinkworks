#[cfg(test)]
mod tests;

use super::*;
use crate::workflows::{WorkflowRun, WorkflowRunStore, handoff::PendingHandoff};

impl ConversationStore {
    pub(crate) fn transfer_prepared(
        &self,
        runs: &WorkflowRunStore,
        expected: &WorkflowRun,
        source_revision: u32,
        destination: &ConversationRecord,
        job: JobId,
        prompt: &str,
    ) -> Result<ConversationRecord, ConversationError> {
        let mut conversations = self.lock();
        let source_id = expected
            .conversation_id
            .ok_or(ConversationError::Conflict)?;
        self.require_durable(&source_id)?;
        self.require_durable(&destination.id)?;
        let source = conversations
            .get(&source_id)
            .ok_or(ConversationError::Missing)?;
        if source.revision != source_revision
            || conversations.get(&destination.id) != Some(destination)
            || destination.active_job.is_some()
            || !destination.messages.is_empty()
        {
            return Err(ConversationError::Conflict);
        }
        let pending = PendingHandoff {
            source: source_id.as_hex(),
            source_job: source.active_job.map(|id| id.as_hex()),
            destination_revision: destination.revision,
            destination_job: job.as_hex(),
            prompt: normalise_message(prompt)?,
        };
        let mut next = conversations.clone();
        apply_handoff(&mut next, destination.id, &pending)?;
        let fingerprint = expected.handoff_fingerprint();
        // The conversation lock protects both revisions through the sole durable ownership commit.
        let committed = runs
            .mutate(&expected.id, |run| {
                run.transfer(&fingerprint, source_id, destination.id)?;
                run.pending_handoff = Some(pending);
                Ok(())
            })
            .map_err(|error| match error {
                crate::workflows::StoreError::Persist => ConversationError::Persist,
                _ => ConversationError::Conflict,
            })?;
        *conversations = next;
        let record = conversations
            .get(&destination.id)
            .cloned()
            .ok_or(ConversationError::Missing)?;
        // The run journal is authoritative after commit. A failed projection retains recovery, not a rollback.
        if let Err(error) = self.finish_handoff(runs, &committed, &conversations) {
            tracing::error!(%error, run = %committed.id.as_hex(), "Prepared-change ownership needs recovery");
        }
        Ok(record)
    }

    pub(crate) fn recover_handoffs(
        &self,
        runs: &WorkflowRunStore,
    ) -> Result<(), ConversationError> {
        for run in runs.active_runs() {
            let Some(pending) = &run.pending_handoff else {
                continue;
            };
            let mut conversations = self.lock();
            let mut next = conversations.clone();
            apply_handoff(
                &mut next,
                run.conversation_id.ok_or(ConversationError::Corrupt)?,
                pending,
            )?;
            self.finish_handoff(runs, &run, &next)?;
            *conversations = next;
        }
        Ok(())
    }

    fn finish_handoff(
        &self,
        runs: &WorkflowRunStore,
        run: &WorkflowRun,
        conversations: &BTreeMap<ConversationId, ConversationRecord>,
    ) -> Result<(), ConversationError> {
        // A replacement can precede a directory-sync failure. Flush the ownership journal before its projections.
        runs.flush_handoff(run)
            .map_err(|_| ConversationError::Persist)?;
        let pending = run
            .pending_handoff
            .as_ref()
            .ok_or(ConversationError::Conflict)?;
        let source = ConversationId::parse(&pending.source).ok_or(ConversationError::Corrupt)?;
        let destination = run.conversation_id.ok_or(ConversationError::Corrupt)?;
        for id in [source, destination] {
            let record = conversations.get(&id).ok_or(ConversationError::Missing)?;
            persist_record(self.path.as_deref(), record)?;
        }
        runs.mutate(&run.id, |current| {
            if current.pending_handoff != run.pending_handoff {
                return Err(crate::workflows::run::TransitionError::Invalid);
            }
            current.pending_handoff = None;
            Ok(())
        })
        .map_err(|_| ConversationError::Persist)?;
        Ok(())
    }

    pub(crate) fn restore_prepared(
        &self,
        runs: &WorkflowRunStore,
        expected: &WorkflowRun,
        owner: &ConversationRecord,
        job: JobId,
    ) -> Result<ConversationRecord, ConversationError> {
        let mut conversations = self.lock();
        self.require_durable(&owner.id)?;
        if conversations.get(&owner.id) != Some(owner) || expected.conversation_id != Some(owner.id)
        {
            return Err(ConversationError::Conflict);
        }
        let mut updated = owner.clone();
        let message = updated
            .messages
            .last_mut()
            .filter(|message| message.role == MessageRole::Assistant)
            .ok_or(ConversationError::Conflict)?;
        message.status = MessageStatus::Pending;
        message.error = None;
        message.request = Some(job);
        updated.active_job = Some(job);
        updated.revision = updated
            .revision
            .checked_add(1)
            .ok_or(ConversationError::Revision)?;
        updated.updated_at_ms = now_ms().max(updated.updated_at_ms);
        runs.mutate(&expected.id, |current| {
            if current != expected
                || !current.recoverable_gate()
                || current.pending_handoff.is_some()
            {
                return Err(crate::workflows::run::TransitionError::Invalid);
            }
            Ok(())
        })
        .map_err(|_| ConversationError::Conflict)?;
        match persist_record(self.path.as_deref(), &updated) {
            Ok(()) => {
                conversations.insert(owner.id, updated.clone());
                Ok(updated)
            }
            Err(ConversationError::Unsettled) => {
                conversations.insert(owner.id, updated);
                self.uncertain
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(owner.id);
                Err(ConversationError::Unsettled)
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) fn interrupt_requests(&self) -> Result<(), ConversationError> {
        let mut conversations = self.lock();
        let mut next = conversations.clone();
        if interrupt_recovered_requests(&mut next) {
            persist_map(self.path.as_deref(), &next)?;
            *conversations = next;
        }
        Ok(())
    }
}

fn apply_handoff(
    conversations: &mut BTreeMap<ConversationId, ConversationRecord>,
    destination: ConversationId,
    pending: &PendingHandoff,
) -> Result<(), ConversationError> {
    let source = ConversationId::parse(&pending.source).ok_or(ConversationError::Corrupt)?;
    let job = JobId::parse(&pending.destination_job).ok_or(ConversationError::Corrupt)?;
    let record = conversations
        .get_mut(&destination)
        .ok_or(ConversationError::Corrupt)?;
    if !record
        .messages
        .iter()
        .any(|message| message.request == Some(job))
    {
        if record.revision != pending.destination_revision
            || record.active_job.is_some()
            || !record.messages.is_empty()
        {
            return Err(ConversationError::Conflict);
        }
        record.messages.push(ConversationMessage {
            id: MessageId::generate().map_err(|_| ConversationError::Random)?,
            role: MessageRole::User,
            text: pending.prompt.clone(),
            activity: Vec::new(),
            continuation: Vec::new(),
            status: MessageStatus::Complete,
            error: None,
            request: None,
        });
        record.messages.push(ConversationMessage {
            id: MessageId::generate().map_err(|_| ConversationError::Random)?,
            role: MessageRole::Assistant,
            text: String::new(),
            activity: Vec::new(),
            continuation: Vec::new(),
            status: MessageStatus::Pending,
            error: None,
            request: Some(job),
        });
        record.active_job = Some(job);
        record.revision = record
            .revision
            .checked_add(1)
            .ok_or(ConversationError::Revision)?;
        record.updated_at_ms = now_ms().max(record.updated_at_ms);
    }
    if let Some(source_job) = pending.source_job.as_deref().and_then(JobId::parse)
        && let Some(record) = conversations.get_mut(&source)
        && let Some(message) = record
            .messages
            .iter_mut()
            .find(|message| message.request == Some(source_job))
        && matches!(
            message.status,
            MessageStatus::Pending | MessageStatus::Interrupted
        )
    {
        message.status = MessageStatus::Complete;
        message.error = None;
        message.text = format!(
            "Prepared changes now belong to [the destination conversation](/conversations/{}). No files were applied or discarded.",
            destination.as_hex()
        );
        if record.active_job == Some(source_job) {
            record.active_job = None;
        }
        record.revision = record
            .revision
            .checked_add(1)
            .ok_or(ConversationError::Revision)?;
        record.updated_at_ms = now_ms().max(record.updated_at_ms);
    }
    Ok(())
}
