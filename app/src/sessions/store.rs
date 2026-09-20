use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::{
    agents::AgentId,
    conversations::ConversationId,
    providers::{AssistantReply, ChatTurn},
    sessions::{
        SESSION_LIFETIME,
        job::{Job, JobId},
        tokens::SessionId,
    },
};

#[cfg(test)]
use crate::sessions::JobSnapshot;

#[cfg(test)]
mod tests;

pub(crate) struct SessionStore {
    sessions: Mutex<HashMap<SessionId, StoredSession>>,
    conversation_jobs: Mutex<HashMap<ConversationId, ConversationJob>>,
    clock: Clock,
}

pub(crate) struct CommandReservation {
    store: Arc<SessionStore>,
    session: SessionId,
    conversation: ConversationId,
    token: JobId,
}

impl Drop for CommandReservation {
    fn drop(&mut self) {
        if let Some(session) = self.store.lock().get_mut(&self.session)
            && session.commands.get(&self.conversation) == Some(&self.token)
        {
            session.commands.remove(&self.conversation);
        }
    }
}

struct ConversationJob {
    job: Arc<Job>,
    session: SessionId,
}

struct Clock {
    offset_ms: AtomicU64,
}

impl Clock {
    fn real() -> Self {
        Self {
            offset_ms: AtomicU64::new(0),
        }
    }

    fn now(&self) -> Instant {
        Instant::now() + Duration::from_millis(self.offset_ms.load(Ordering::SeqCst))
    }
}

struct Conversation {
    turns: Vec<ChatTurn>,
    job: Option<Arc<Job>>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ConversationKey {
    pub(crate) agent_id: AgentId,
}

struct StoredSession {
    conversations: HashMap<ConversationKey, Conversation>,
    language: Option<super::BrowserLanguage>,
    // Legacy desk jobs use this token. Saved conversations do not share it.
    active: Option<JobId>,
    commands: HashMap<ConversationId, JobId>,
    expires_at: Instant,
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct SessionSnapshot {
    pub(crate) turns: Vec<ChatTurn>,
    pub(crate) job: Option<JobSnapshot>,
    pub(crate) session_busy: bool,
}

#[derive(Debug)]
pub(crate) enum BeginTurnError {
    MissingSession,
    Conflict,
    JobId,
}

impl SessionStore {
    pub(crate) fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            conversation_jobs: Mutex::new(HashMap::new()),
            clock: Clock::real(),
        }
    }

    pub(crate) fn insert(&self, id: SessionId) {
        let expires_at = self.clock.now() + SESSION_LIFETIME;
        self.lock().insert(
            id,
            StoredSession {
                conversations: HashMap::new(),
                language: None,
                active: None,
                commands: HashMap::new(),
                expires_at,
            },
        );
    }

    pub(crate) fn set_language(&self, id: &SessionId, language: super::BrowserLanguage) {
        if let Some(session) = live_mut(&mut self.lock(), id, self.clock.now()) {
            session.language = Some(language);
        }
    }

    pub(crate) fn language(&self, id: &SessionId) -> Option<super::BrowserLanguage> {
        live(&mut self.lock(), id, self.clock.now()).and_then(|session| session.language.clone())
    }

    pub(crate) fn contains_live(&self, id: &SessionId) -> bool {
        let mut sessions = self.lock();
        live(&mut sessions, id, self.clock.now()).is_some()
    }

    pub(crate) fn contains_expired(&self, id: &SessionId) -> bool {
        self.lock()
            .get(id)
            .is_some_and(|session| session.expires_at <= self.clock.now())
    }

    #[cfg(test)]
    pub(crate) fn busy(&self, id: &SessionId) -> bool {
        let mut sessions = self.lock();
        live(&mut sessions, id, self.clock.now()).is_some_and(|session| session.active.is_some())
    }

    pub(crate) fn command_reserved(&self, id: &SessionId, conversation: &ConversationId) -> bool {
        let mut sessions = self.lock();
        let now = self.clock.now();
        live(&mut sessions, id, now).is_some() && command_held(&sessions, conversation, now)
    }

    pub(crate) fn reserve_command(
        self: &Arc<Self>,
        id: SessionId,
        conversation: ConversationId,
    ) -> Result<CommandReservation, BeginTurnError> {
        let token = JobId::generate().map_err(|_| BeginTurnError::JobId)?;
        let mut sessions = self.lock();
        let now = self.clock.now();
        live_mut(&mut sessions, &id, now).ok_or(BeginTurnError::MissingSession)?;
        if command_held(&sessions, &conversation, now)
            || self
                .conversation_jobs()
                .get(&conversation)
                .is_some_and(|entry| {
                    // Handoff can use a prepared candidate while its gate retains ownership.
                    entry.job.snapshot().status != super::JobStatus::AwaitingDecision
                })
        {
            return Err(BeginTurnError::Conflict);
        }
        sessions
            .get_mut(&id)
            .expect("live session")
            .commands
            .insert(conversation, token);
        Ok(CommandReservation {
            store: self.clone(),
            session: id,
            conversation,
            token,
        })
    }

    #[cfg(test)]
    pub(crate) fn snapshot(
        &self,
        id: &SessionId,
        key: &ConversationKey,
    ) -> Option<SessionSnapshot> {
        let mut sessions = self.lock();
        live(&mut sessions, id, self.clock.now()).map(|session| snapshot_session(key, session))
    }

    pub(crate) fn finish_turn(
        &self,
        id: &SessionId,
        key: &ConversationKey,
        job_id: &JobId,
        reply: impl Into<AssistantReply>,
    ) -> bool {
        self.complete_turn(id, key, job_id, reply.into())
    }

    pub(crate) fn fail_turn(
        &self,
        id: &SessionId,
        key: &ConversationKey,
        job_id: &JobId,
        partial: impl Into<AssistantReply>,
    ) -> bool {
        self.complete_turn(id, key, job_id, partial.into())
    }

    pub(crate) fn begin_conversation_job(
        &self,
        id: &SessionId,
        conversation_id: ConversationId,
    ) -> Result<Arc<Job>, BeginTurnError> {
        let job_id = JobId::generate().map_err(|_| BeginTurnError::JobId)?;
        let mut sessions = self.lock();
        live_mut(&mut sessions, id, self.clock.now()).ok_or(BeginTurnError::MissingSession)?;
        let mut jobs = self.conversation_jobs();
        if jobs.contains_key(&conversation_id)
            || command_held(&sessions, &conversation_id, self.clock.now())
        {
            return Err(BeginTurnError::Conflict);
        }
        let job = Job::for_conversation(job_id, conversation_id);
        jobs.insert(
            conversation_id,
            ConversationJob {
                job: job.clone(),
                session: *id,
            },
        );
        Ok(job)
    }

    pub(crate) fn conversation_reserved(&self, conversation_id: ConversationId) -> bool {
        let sessions = self.lock();
        command_held(&sessions, &conversation_id, self.clock.now())
            || self.conversation_jobs().contains_key(&conversation_id)
    }

    pub(crate) fn conversation_job(
        &self,
        conversation_id: ConversationId,
        job_id: JobId,
    ) -> Option<Arc<Job>> {
        self.conversation_jobs()
            .get(&conversation_id)
            .filter(|entry| entry.job.id() == job_id)
            .map(|entry| entry.job.clone())
    }

    pub(crate) fn owns_conversation_job(
        &self,
        id: &SessionId,
        conversation_id: ConversationId,
        job_id: JobId,
    ) -> bool {
        let mut sessions = self.lock();
        live(&mut sessions, id, self.clock.now()).is_some()
            && self
                .conversation_jobs()
                .get(&conversation_id)
                .is_some_and(|entry| entry.session == *id && entry.job.id() == job_id)
    }

    pub(crate) fn finish_conversation_job(
        &self,
        id: &SessionId,
        conversation_id: ConversationId,
        job_id: JobId,
    ) -> bool {
        let mut jobs = self.conversation_jobs();
        if !jobs
            .get(&conversation_id)
            .is_some_and(|entry| entry.job.id() == job_id && entry.session == *id)
        {
            return false;
        }
        jobs.remove(&conversation_id);
        true
    }

    pub(crate) fn remove(&self, id: &SessionId) {
        let mut sessions = self.lock();
        cancel_and_remove(&mut sessions, id);
        for entry in self
            .conversation_jobs()
            .values()
            .filter(|entry| entry.session == *id)
        {
            entry.job.request_cancel();
        }
    }

    pub(crate) fn expired_ids(&self) -> Vec<SessionId> {
        let now = self.clock.now();
        self.lock()
            .iter()
            .filter(|(_, session)| session.expires_at <= now)
            .map(|(id, _)| *id)
            .collect()
    }

    // Only the active job can complete the turn. A stale writer cannot overwrite a later command.
    fn complete_turn(
        &self,
        id: &SessionId,
        key: &ConversationKey,
        job_id: &JobId,
        reply: AssistantReply,
    ) -> bool {
        let mut sessions = self.lock();
        let Some(session) = live_mut(&mut sessions, id, self.clock.now()) else {
            return false;
        };
        if session.active != Some(*job_id) {
            return false;
        }
        if let Some(conversation) = session.conversations.get_mut(key)
            && !reply.is_empty()
        {
            conversation.turns.push(ChatTurn::assistant(reply));
        }
        session.active = None;
        true
    }

    fn conversation_jobs(&self) -> MutexGuard<'_, HashMap<ConversationId, ConversationJob>> {
        self.conversation_jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<SessionId, StoredSession>> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn command_held(
    sessions: &HashMap<SessionId, StoredSession>,
    conversation: &ConversationId,
    now: Instant,
) -> bool {
    sessions
        .values()
        .any(|session| session.expires_at > now && session.commands.contains_key(conversation))
}

#[cfg(test)]
fn snapshot_session(key: &ConversationKey, session: &StoredSession) -> SessionSnapshot {
    let conversation = session.conversations.get(key);
    SessionSnapshot {
        turns: conversation
            .map(|conversation| conversation.turns.clone())
            .unwrap_or_default(),
        job: conversation
            .and_then(|conversation| conversation.job.as_ref().map(|job| job.snapshot())),
        session_busy: session.active.is_some(),
    }
}

fn live<'a>(
    sessions: &'a mut HashMap<SessionId, StoredSession>,
    id: &SessionId,
    now: Instant,
) -> Option<&'a StoredSession> {
    sessions.get(id).filter(|session| session.expires_at > now)
}

fn live_mut<'a>(
    sessions: &'a mut HashMap<SessionId, StoredSession>,
    id: &SessionId,
    now: Instant,
) -> Option<&'a mut StoredSession> {
    if sessions
        .get(id)
        .is_some_and(|session| session.expires_at <= now)
    {
        return None;
    }
    sessions.get_mut(id)
}

fn cancel_and_remove(sessions: &mut HashMap<SessionId, StoredSession>, id: &SessionId) {
    if let Some(session) = sessions.get(id) {
        cancel_jobs(session);
    }
    sessions.remove(id);
}

fn cancel_jobs(session: &StoredSession) {
    for conversation in session.conversations.values() {
        if let Some(job) = &conversation.job {
            job.request_cancel();
        }
    }
}
