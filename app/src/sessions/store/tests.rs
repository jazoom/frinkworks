use super::*;

impl super::SessionStore {
    pub(crate) fn purge_expired(&self) {
        let now = self.clock.now();
        self.lock().retain(|_, session| session.expires_at > now);
    }

    pub(crate) fn advance_clock(&self, duration: Duration) {
        self.clock.advance(duration);
    }

    pub(crate) fn contains(&self, id: &super::SessionId) -> bool {
        self.lock().contains_key(id)
    }
}

impl super::Clock {
    fn advance(&self, duration: Duration) {
        let millis = u64::try_from(duration.as_millis()).expect("duration fits u64");
        self.offset_ms.fetch_add(millis, Ordering::SeqCst);
    }
}

use std::time::Duration;

use crate::agents::AgentId;
use crate::providers::ChatTurn;
use crate::sessions::{self, ConversationKey, SESSION_LIFETIME};
use crate::workflows::RunId;

fn conversation() -> ConversationKey {
    ConversationKey {
        agent_id: AgentId::generate().expect("agent"),
    }
}

fn run() -> RunId {
    RunId::generate().expect("run")
}

#[test]
fn parallel_commands_cannot_overwrite_a_completed_turn() {
    let store = super::SessionStore::new();
    let token = sessions::generate_session_token().expect("token");
    let id = token.id();
    let key = conversation();
    store.insert(id);

    let first = store
        .begin_turn(&id, key, run(), "First".to_owned())
        .expect("first");
    assert!(matches!(
        store.begin_turn(&id, key, run(), "Second".to_owned()),
        Err(BeginTurnError::Conflict)
    ));
    assert_eq!(
        store.snapshot(&id, &key).expect("session").turns,
        [ChatTurn::user("First".to_owned())]
    );

    assert!(store.finish_turn(&id, &key, &first.job.id(), "Done".to_owned()));

    let second = store
        .begin_turn(&id, key, run(), "Third".to_owned())
        .expect("third");
    assert!(!store.finish_turn(&id, &key, &first.job.id(), "Late".to_owned()));

    let snapshot = store.snapshot(&id, &key).expect("session");
    assert_eq!(
        snapshot.turns,
        [
            ChatTurn::user("First".to_owned()),
            ChatTurn::assistant("Done".into()),
            ChatTurn::user("Third".to_owned()),
        ]
    );

    assert!(store.fail_turn(&id, &key, &second.job.id(), "partial".to_owned()));
    let snapshot = store.snapshot(&id, &key).expect("session");
    assert_eq!(
        snapshot.turns.last().map(|turn| turn.text.as_str()),
        Some("partial")
    );
}

#[test]
fn one_session_job_blocks_another_conversation() {
    let store = super::SessionStore::new();
    let token = sessions::generate_session_token().expect("token");
    let id = token.id();
    let first = conversation();
    let second = conversation();
    store.insert(id);
    store
        .begin_turn(&id, first, run(), "First".to_owned())
        .expect("first");
    assert!(matches!(
        store.begin_turn(&id, second, run(), "Second".to_owned()),
        Err(BeginTurnError::Conflict)
    ));
    assert!(
        store
            .snapshot(&id, &second)
            .expect("session")
            .turns
            .is_empty()
    );
    assert!(store.snapshot(&id, &first).expect("session").session_busy);
}

#[test]
fn whitespace_assistant_output_does_not_add_a_turn() {
    let store = super::SessionStore::new();
    let token = sessions::generate_session_token().expect("token");
    let id = token.id();
    let key = conversation();
    store.insert(id);
    let begun = store
        .begin_turn(&id, key, run(), "Hello".to_owned())
        .expect("begin");

    assert!(store.fail_turn(&id, &key, &begun.job.id(), " \n ".to_owned()));
    assert_eq!(
        store.snapshot(&id, &key).expect("session").turns,
        [ChatTurn::user("Hello".to_owned())]
    );
}

#[test]
fn transcripts_are_independent_per_conversation() {
    let store = super::SessionStore::new();
    let token = sessions::generate_session_token().expect("token");
    let id = token.id();
    let first = conversation();
    let second = ConversationKey {
        agent_id: AgentId::generate().expect("agent"),
    };
    store.insert(id);
    let begun = store
        .begin_turn(&id, first, run(), "Hello".to_owned())
        .expect("begin");
    assert!(store.finish_turn(&id, &first, &begun.job.id(), "Hi".to_owned()));
    assert_eq!(
        store
            .snapshot(&id, &first)
            .expect("first")
            .turns
            .iter()
            .map(|turn| turn.text.as_str())
            .collect::<Vec<_>>(),
        ["Hello", "Hi"]
    );
    assert!(
        store
            .snapshot(&id, &second)
            .expect("second")
            .turns
            .is_empty()
    );
}

#[test]
fn expired_sessions_cannot_be_resolved() {
    let store = super::SessionStore::new();
    let token = sessions::generate_session_token().expect("token");
    let id = token.id();
    let key = conversation();
    store.insert(id);
    assert!(store.snapshot(&id, &key).is_some());

    store.advance_clock(SESSION_LIFETIME + Duration::from_secs(1));
    assert!(store.snapshot(&id, &key).is_none());
    assert!(store.contains(&id));
    assert!(store.contains_expired(&id));
}

#[test]
fn purge_removes_expired_sessions_without_a_lookup() {
    let store = super::SessionStore::new();
    let token = sessions::generate_session_token().expect("token");
    let id = token.id();
    store.insert(id);

    store.advance_clock(SESSION_LIFETIME + Duration::from_secs(1));
    assert!(store.contains(&id));
    store.purge_expired();
    assert!(!store.contains(&id));
    assert!(!store.contains_live(&id));
}

#[test]
fn live_sessions_survive_purge() {
    let store = super::SessionStore::new();
    let token = sessions::generate_session_token().expect("token");
    let id = token.id();
    let key = conversation();
    store.insert(id);

    store.advance_clock(SESSION_LIFETIME.saturating_sub(Duration::from_secs(1)));
    store.purge_expired();
    assert!(store.snapshot(&id, &key).is_some());
}

#[test]
fn remove_cancels_the_active_job() {
    let store = super::SessionStore::new();
    let token = sessions::generate_session_token().expect("token");
    let id = token.id();
    let key = conversation();
    store.insert(id);
    let begun = store
        .begin_turn(&id, key, run(), "Hello".to_owned())
        .expect("begin");
    store.remove(&id);
    assert!(begun.job.cancel_requested());
    assert!(store.snapshot(&id, &key).is_none());
}

#[test]
fn conversation_jobs_run_independently_of_each_other() {
    let store = super::SessionStore::new();
    let token = sessions::generate_session_token().expect("token");
    let id = token.id();
    let first = crate::conversations::ConversationId::generate().expect("first conversation");
    let second = crate::conversations::ConversationId::generate().expect("second conversation");
    store.insert(id);
    let first_job = store.begin_conversation_job(&id, first).expect("first job");
    let second_job = store
        .begin_conversation_job(&id, second)
        .expect("second job");
    assert!(!store.busy(&id));
    assert!(store.conversation_reserved(first));
    assert!(store.conversation_reserved(second));
    assert!(store.begin_conversation_job(&id, first).is_err());
    assert!(store.owns_conversation_job(&id, first, first_job.id()));
    assert!(!store.owns_conversation_job(&id, first, second_job.id()));
    assert!(!store.owns_conversation_job(&id, second, first_job.id()));
    let other_session = sessions::generate_session_token()
        .expect("other session")
        .id();
    store.insert(other_session);
    assert!(!store.owns_conversation_job(&other_session, first, first_job.id()));
    assert!(store.begin_conversation_job(&other_session, first).is_err());
    assert!(store.finish_conversation_job(&id, first, first_job.id()));
    assert!(store.conversation_reserved(second));
    assert!(!store.conversation_reserved(first));
    assert!(store.finish_conversation_job(&id, second, second_job.id()));
    assert!(!store.busy(&id));
}

#[test]
fn command_reservations_are_local_to_the_conversation() {
    let store = Arc::new(super::SessionStore::new());
    let session = sessions::generate_session_token().expect("session").id();
    store.insert(session);
    let first = crate::conversations::ConversationId::generate().expect("first");
    let second = crate::conversations::ConversationId::generate().expect("second");
    let first_command = store
        .reserve_command(session, first)
        .expect("first command");
    let second_command = store
        .reserve_command(session, second)
        .expect("second command");
    assert!(store.reserve_command(session, first).is_err());
    assert!(store.conversation_reserved(first));
    assert!(store.begin_conversation_job(&session, first).is_err());
    let other_session = sessions::generate_session_token()
        .expect("other session")
        .id();
    store.insert(other_session);
    assert!(store.reserve_command(other_session, first).is_err());
    assert!(store.begin_conversation_job(&other_session, first).is_err());
    drop(first_command);
    assert!(!store.command_reserved(&session, &first));
    assert!(!store.conversation_reserved(first));
    let job = store
        .begin_conversation_job(&session, first)
        .expect("job after handoff");
    assert!(store.reserve_command(session, first).is_err());
    assert!(store.reserve_command(other_session, first).is_err());
    assert!(job.set_awaiting_decision().is_some());
    let gate_handoff = store
        .reserve_command(session, first)
        .expect("handoff at a safe gate");
    drop(gate_handoff);
    assert!(store.conversation_reserved(first));
    assert!(store.finish_conversation_job(&session, first, job.id()));
    assert!(store.command_reserved(&session, &second));
    drop(second_command);
    assert!(!store.command_reserved(&session, &second));
}

#[test]
fn job_lookup_requires_the_conversation_identity() {
    let store = super::SessionStore::new();
    let token = sessions::generate_session_token().expect("token");
    let id = token.id();
    let first = conversation();
    let second = conversation();
    store.insert(id);
    let begun = store
        .begin_turn(&id, first, run(), "Hello".to_owned())
        .expect("begin");
    assert!(store.job(&id, &first, &begun.job.id()).is_some());
    assert!(store.job(&id, &second, &begun.job.id()).is_none());
}

#[test]
fn rollback_removes_the_reserved_turn() {
    let store = super::SessionStore::new();
    let token = sessions::generate_session_token().expect("token");
    let id = token.id();
    let key = conversation();
    store.insert(id);
    let begun = store
        .begin_turn(&id, key, run(), "Hello".to_owned())
        .expect("begin");
    assert!(store.rollback_turn(&id, &key, &begun.job.id()));
    let snapshot = store.snapshot(&id, &key).expect("session");
    assert!(snapshot.turns.is_empty());
    assert!(!snapshot.session_busy);
    assert!(snapshot.job.is_none());
}

#[test]
fn a_stale_rollback_cannot_remove_a_later_turn() {
    let store = super::SessionStore::new();
    let token = sessions::generate_session_token().expect("token");
    let id = token.id();
    let key = conversation();
    store.insert(id);
    let first = store
        .begin_turn(&id, key, run(), "First".to_owned())
        .expect("first");
    assert!(store.rollback_turn(&id, &key, &first.job.id()));
    let second = store
        .begin_turn(&id, key, run(), "Second".to_owned())
        .expect("second");
    assert!(!store.rollback_turn(&id, &key, &first.job.id()));
    let snapshot = store.snapshot(&id, &key).expect("session");
    assert_eq!(snapshot.turns, [ChatTurn::user("Second".to_owned())]);
    assert_eq!(
        snapshot.job.as_ref().map(|job| job.id),
        Some(second.job.id())
    );
}

impl super::SessionStore {
    pub(crate) fn begin_turn(
        &self,
        id: &SessionId,
        key: ConversationKey,
        run_id: RunId,
        message: String,
    ) -> Result<BegunTurn, BeginTurnError> {
        let mut sessions = self.lock();
        let session =
            live_mut(&mut sessions, id, self.clock.now()).ok_or(BeginTurnError::MissingSession)?;
        if session.active.is_some() {
            return Err(BeginTurnError::Conflict);
        }
        let job_id = JobId::generate().map_err(|_| BeginTurnError::JobId)?;
        let conversation = session
            .conversations
            .entry(key)
            .or_insert_with(|| Conversation {
                turns: Vec::new(),
                job: None,
            });
        conversation.turns.push(ChatTurn::user(message));
        let job = Job::new(job_id, run_id, conversation.turns.len());
        conversation.job = Some(job.clone());
        session.active = Some(job_id);
        Ok(BegunTurn { job })
    }
    pub(crate) fn rollback_turn(
        &self,
        id: &SessionId,
        key: &ConversationKey,
        job_id: &JobId,
    ) -> bool {
        let mut sessions = self.lock();
        let Some(session) = live_mut(&mut sessions, id, self.clock.now()) else {
            return false;
        };
        if session.active != Some(*job_id) {
            return false;
        }
        if let Some(conversation) = session.conversations.get_mut(key) {
            conversation.turns.pop();
            if conversation
                .job
                .as_ref()
                .is_some_and(|job| job.id() == *job_id)
            {
                conversation.job = None;
            }
        }
        session.active = None;
        true
    }
    pub(crate) fn job(
        &self,
        id: &SessionId,
        key: &ConversationKey,
        job_id: &JobId,
    ) -> Option<Arc<Job>> {
        let mut sessions = self.lock();
        live(&mut sessions, id, self.clock.now()).and_then(|session| {
            session
                .conversations
                .get(key)
                .and_then(|conversation| conversation.job.as_ref())
                .filter(|job| job.id() == *job_id)
                .cloned()
        })
    }
}

pub(crate) struct BegunTurn {
    pub(crate) job: Arc<Job>,
}
