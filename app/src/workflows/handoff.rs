use std::{collections::HashMap, sync::Mutex};

use crate::{conversations::ConversationId, sessions::SessionId};

use super::{RunId, WorkflowRun};

#[derive(Clone)]
pub(crate) struct PreparedHandoff {
    pub(crate) source: ConversationId,
    pub(crate) source_revision: u32,
    pub(crate) run: RunId,
    pub(crate) fingerprint: String,
}

#[derive(Default)]
pub(crate) struct HandoffDrafts {
    drafts: Mutex<HashMap<(SessionId, String), PreparedHandoff>>,
}

impl HandoffDrafts {
    pub(crate) fn insert(
        &self,
        session: SessionId,
        nonce: String,
        draft: PreparedHandoff,
        sessions: &crate::sessions::SessionStore,
    ) -> Result<(), &'static str> {
        let mut drafts = self
            .drafts
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        drafts.retain(|(owner, _), _| *owner != session && sessions.contains_live(owner));
        if drafts.len() >= 64 {
            return Err(
                "Too many handoff drafts are open. Close a browser session before another handoff.",
            );
        }
        drafts.insert((session, nonce), draft);
        Ok(())
    }

    pub(crate) fn get(&self, session: SessionId, nonce: &str) -> Option<PreparedHandoff> {
        self.drafts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&(session, nonce.to_owned()))
            .cloned()
    }

    pub(crate) fn remove(&self, session: SessionId, nonce: &str) {
        self.drafts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&(session, nonce.to_owned()));
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PendingHandoff {
    pub(crate) source: String,
    pub(crate) source_job: Option<String>,
    pub(crate) destination_revision: u32,
    pub(crate) destination_job: String,
    pub(crate) prompt: String,
}

impl PendingHandoff {
    pub(crate) fn valid(&self) -> bool {
        ConversationId::parse(&self.source).is_some()
            && self
                .source_job
                .as_deref()
                .is_none_or(|id| crate::sessions::JobId::parse(id).is_some())
            && self.destination_revision > 0
            && crate::sessions::JobId::parse(&self.destination_job).is_some()
            && crate::conversations::normalise_message(&self.prompt)
                .is_ok_and(|text| text == self.prompt)
    }
}

impl WorkflowRun {
    pub(crate) fn handoff_settings(&self) -> Vec<crate::execution::ExecutionSettings> {
        self.model_phases()
            .filter_map(|phase| phase.settings.clone())
            .collect()
    }

    pub(crate) fn decision_revision(
        &self,
        gate: &super::gates::HumanGateRecord,
    ) -> super::gates::GateRevision {
        // Ownership changes invalidate open decision forms without changing the pinned gate or its evidence.
        super::gates::GateRevision::new(
            gate.revision.get() + ((self.ownership_history.len() as u64) << 32),
        )
        .expect("bounded gate and ownership history")
    }

    pub(crate) fn handoff_fingerprint(&self) -> String {
        super::artefacts::ObjectHash::of(&serde_json::to_vec(&self.to_file()).expect("run record"))
            .as_str()
    }

    pub(crate) fn transferable(&self) -> bool {
        self.recoverable_gate()
            && self.pending_handoff.is_none()
            && self.ownership_history.len() < 64
    }

    pub(crate) fn recoverable_gate(&self) -> bool {
        self.conversation_id.is_some()
            && self.agent_id.is_none()
            && self.directory_settings().is_some()
            && matches!(self.state, super::run::RunState::AwaitingHuman { .. })
            && matches!(self.source, super::RunSource::Captured { .. })
            && self
                .attempts
                .iter()
                .all(|attempt| attempt.cleanup == super::run::AttemptCleanupRecord::Complete)
            && !self.apply_is_uncertain()
            && !self.apply_is_known_partial()
    }

    pub(crate) fn transfer(
        &mut self,
        expected: &str,
        source: ConversationId,
        destination: ConversationId,
    ) -> Result<(), super::run::TransitionError> {
        if !self.transferable()
            || self.conversation_id != Some(source)
            || source == destination
            || self.ownership_history.contains(&destination)
            || self.handoff_fingerprint() != expected
        {
            return Err(super::run::TransitionError::Invalid);
        }
        // This run record is the sole ownership authority. Candidates and their original source remain unchanged.
        self.ownership_history.push(source);
        self.conversation_id = Some(destination);
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests;
