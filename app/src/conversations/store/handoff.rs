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
        let mut database = self.database();
        let source_id = expected
            .conversation_id
            .ok_or(ConversationError::Conflict)?;
        self.require_durable(&source_id)?;
        self.require_durable(&destination.id)?;
        let source = database
            .load(&source_id)?
            .ok_or(ConversationError::Missing)?;
        let stored_destination = database
            .load(&destination.id)?
            .ok_or(ConversationError::Missing)?;
        if source.revision != source_revision
            || stored_destination != *destination
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
        let mut next_source = source.clone();
        let mut next_destination = destination.clone();
        apply_handoff(&mut next_source, &mut next_destination, &pending)?;
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
        // The run journal is authoritative after commit. A failed projection
        // retains recovery, not a rollback.
        if let Err(error) = self.commit_handoff(
            &mut database,
            runs,
            &committed,
            [
                (&source, &next_source),
                (&stored_destination, &next_destination),
            ],
        ) {
            tracing::error!(%error, run = %committed.id.as_hex(), "Prepared-change ownership needs recovery");
        }
        Ok(next_destination)
    }

    pub(crate) fn recover_handoffs(
        &self,
        runs: &WorkflowRunStore,
    ) -> Result<(), ConversationError> {
        for run in runs.active_runs() {
            let Some(pending) = &run.pending_handoff else {
                continue;
            };
            let mut database = self.database();
            let source_id =
                ConversationId::parse(&pending.source).ok_or(ConversationError::Corrupt)?;
            let destination_id = run.conversation_id.ok_or(ConversationError::Corrupt)?;
            let source = database
                .load(&source_id)?
                .ok_or(ConversationError::Missing)?;
            let destination = database
                .load(&destination_id)?
                .ok_or(ConversationError::Missing)?;
            let mut next_source = source.clone();
            let mut next_destination = destination.clone();
            apply_handoff(&mut next_source, &mut next_destination, pending)?;
            self.commit_handoff(
                &mut database,
                runs,
                &run,
                [(&source, &next_source), (&destination, &next_destination)],
            )?;
        }
        Ok(())
    }

    /// Flush the ownership journal, commit both conversation projections in one
    /// transaction, then clear the journal. Recovery is idempotent before that
    /// clearance, so a failed projection is retried rather than rolled back.
    fn commit_handoff(
        &self,
        database: &mut Database,
        runs: &WorkflowRunStore,
        run: &WorkflowRun,
        projections: [(&ConversationRecord, &ConversationRecord); 2],
    ) -> Result<(), ConversationError> {
        runs.flush_handoff(run)
            .map_err(|_| ConversationError::Persist)?;
        self.persist_pair(
            database,
            Some(projections[0].0),
            projections[0].1,
            Some(projections[1].0),
            projections[1].1,
        )?;
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
        let mut database = self.database();
        self.require_durable(&owner.id)?;
        let stored = database
            .load(&owner.id)?
            .ok_or(ConversationError::Missing)?;
        if stored != *owner || expected.conversation_id != Some(owner.id) {
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
        self.persist(&mut database, Some(owner), &updated)?;
        Ok(updated)
    }

    pub(crate) fn interrupt_requests(&self) -> Result<(), ConversationError> {
        let mut database = self.database();
        for id in database.active_ids()? {
            let Some(current) = database.load(&id)? else {
                continue;
            };
            let mut updated = current.clone();
            if interrupt_recovered_request(&mut updated) {
                self.persist(&mut database, Some(&current), &updated)?;
            }
        }
        Ok(())
    }
}

fn apply_handoff(
    source: &mut ConversationRecord,
    destination: &mut ConversationRecord,
    pending: &PendingHandoff,
) -> Result<(), ConversationError> {
    let job = JobId::parse(&pending.destination_job).ok_or(ConversationError::Corrupt)?;
    if !destination
        .messages
        .iter()
        .any(|message| message.request == Some(job))
    {
        if destination.revision != pending.destination_revision
            || destination.active_job.is_some()
            || !destination.messages.is_empty()
        {
            return Err(ConversationError::Conflict);
        }
        let user_id = MessageId::generate().map_err(|_| ConversationError::Random)?;
        let assistant_id = MessageId::generate().map_err(|_| ConversationError::Random)?;
        destination
            .messages
            .push(super::user_message(user_id, pending.prompt.clone()));
        destination
            .messages
            .push(super::pending_phase(assistant_id, assistant_id, job));
        destination.active_job = Some(job);
        destination.revision = destination
            .revision
            .checked_add(1)
            .ok_or(ConversationError::Revision)?;
        destination.updated_at_ms = now_ms().max(destination.updated_at_ms);
    }
    if let Some(source_job) = pending.source_job.as_deref().and_then(JobId::parse)
        && let Some(message) = source
            .messages
            .iter_mut()
            .rev()
            .find(|message| message.request == Some(source_job))
        && matches!(
            message.status,
            MessageStatus::Pending | MessageStatus::Interrupted
        )
    {
        message.status = MessageStatus::Complete;
        message.final_phase = true;
        message.error = None;
        message.text = format!(
            "Prepared changes now belong to [the destination conversation](/conversations/{}). No files were applied or discarded.",
            destination.id.as_hex()
        );
        if source.active_job == Some(source_job) {
            source.active_job = None;
        }
        source.revision = source
            .revision
            .checked_add(1)
            .ok_or(ConversationError::Revision)?;
        source.updated_at_ms = now_ms().max(source.updated_at_ms);
    }
    super::project_active_path(source);
    super::project_active_path(destination);
    Ok(())
}
