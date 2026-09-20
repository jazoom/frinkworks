use std::path::PathBuf;
use std::sync::Arc;

use crate::sessions::{Job, JobId};

use super::{ApprovalError, HostApprovalStore, HostCommandDecision, HostCommandRequest};

fn request(
    session: crate::sessions::SessionId,
    job: JobId,
    conversation: crate::conversations::ConversationId,
    revision: u32,
    command: &str,
    explanation: &str,
) -> HostCommandRequest {
    HostCommandRequest {
        token: String::new(),
        session,
        job,
        conversation,
        execution_revision: revision,
        command: command.to_owned(),
        directory: PathBuf::from("/tmp"),
        explanation: explanation.to_owned(),
        run: None,
        step: None,
        attempt: None,
    }
}

fn session() -> crate::sessions::SessionId {
    crate::sessions::generate_session_token().unwrap().id()
}

fn job() -> (JobId, Arc<Job>) {
    let id = JobId::generate().unwrap();
    let job = Job::for_conversation(
        id,
        crate::conversations::ConversationId::generate().unwrap(),
    );
    (id, job)
}

#[tokio::test]
async fn approval_binds_session_job_revision_and_command() {
    let store = HostApprovalStore::new();
    let owner = session();
    let other = session();
    let conversation = crate::conversations::ConversationId::generate().unwrap();
    let (job_id, job) = job();
    let mut submitted = request(owner, job_id, conversation, 3, "printf hi", "List files");
    submitted.token = store.submit(submitted.clone()).unwrap();

    let mut other_session = submitted.clone();
    other_session.session = other;
    assert_eq!(
        store.decide(&other_session, HostCommandDecision::Approved),
        Err(ApprovalError::Invalid)
    );
    let mut stale = submitted.clone();
    stale.execution_revision = 4;
    assert_eq!(
        store.decide(&stale, HostCommandDecision::Approved),
        Err(ApprovalError::Invalid)
    );
    let mut tampered = submitted.clone();
    tampered.command = "rm -rf /".to_owned();
    assert_eq!(
        store.decide(&tampered, HostCommandDecision::Approved),
        Err(ApprovalError::Invalid)
    );
    let mut other_directory = submitted.clone();
    other_directory.directory = PathBuf::from("/");
    assert_eq!(
        store.decide(&other_directory, HostCommandDecision::Approved),
        Err(ApprovalError::Invalid)
    );
    let mut other_explanation = submitted.clone();
    other_explanation.explanation = "Different explanation".to_owned();
    assert_eq!(
        store.decide(&other_explanation, HostCommandDecision::Approved),
        Err(ApprovalError::Invalid)
    );
    store
        .decide(&submitted, HostCommandDecision::Approved)
        .unwrap();
    assert_eq!(
        store.decide(&submitted, HostCommandDecision::Approved),
        Err(ApprovalError::Duplicate)
    );
    assert_eq!(
        store.wait(&submitted.token, &job).await.unwrap(),
        HostCommandDecision::Approved
    );
}

#[tokio::test]
async fn only_one_waiter_can_consume_an_approval() {
    let store = HostApprovalStore::new();
    let (job_id, job) = job();
    let conversation = crate::conversations::ConversationId::generate().unwrap();
    let mut submitted = request(session(), job_id, conversation, 1, "true", "");
    submitted.token = store.submit(submitted.clone()).unwrap();
    let first = store.wait(&submitted.token, &job);
    tokio::pin!(first);
    assert!(futures_util::poll!(&mut first).is_pending());
    assert_eq!(
        store.wait(&submitted.token, &job).await,
        Err(ApprovalError::Duplicate)
    );
    store
        .decide(&submitted, HostCommandDecision::Approved)
        .unwrap();
    assert_eq!(first.await.unwrap(), HostCommandDecision::Approved);
    assert_eq!(
        store.wait(&submitted.token, &job).await,
        Err(ApprovalError::Invalid)
    );
}

#[tokio::test]
async fn completed_requests_do_not_exhaust_the_pending_capacity() {
    let store = HostApprovalStore::new();
    let (job_id, job) = job();
    let owner = session();
    let conversation = crate::conversations::ConversationId::generate().unwrap();
    let mut first = None;
    for _ in 0..=super::MAXIMUM_PENDING {
        let mut submitted = request(owner, job_id, conversation, 1, "true", "");
        submitted.token = store.submit(submitted.clone()).unwrap();
        first.get_or_insert_with(|| submitted.clone());
        store
            .decide(&submitted, HostCommandDecision::Rejected)
            .unwrap();
        assert_eq!(
            store.wait(&submitted.token, &job).await.unwrap(),
            HostCommandDecision::Rejected
        );
    }
    assert!(
        store
            .decide(&first.unwrap(), HostCommandDecision::Approved)
            .is_err()
    );
}

#[tokio::test]
async fn rejection_and_cancellation_are_terminal() {
    let store = HostApprovalStore::new();
    let owner = session();
    let conversation = crate::conversations::ConversationId::generate().unwrap();
    let (job_id, job) = job();
    let mut submitted = request(owner, job_id, conversation, 1, "true", "");
    submitted.token = store.submit(submitted.clone()).unwrap();
    store
        .decide(&submitted, HostCommandDecision::Rejected)
        .unwrap();
    assert_eq!(
        store.wait(&submitted.token, &job).await.unwrap(),
        HostCommandDecision::Rejected
    );

    let mut submitted = request(owner, job_id, conversation, 1, "true", "");
    submitted.token = store.submit(submitted.clone()).unwrap();
    job.request_cancel();
    assert_eq!(
        store.wait(&submitted.token, &job).await.unwrap_err(),
        ApprovalError::Cancelled
    );
    assert!(store.pending_for(conversation, job_id).is_none());
}

#[tokio::test]
async fn session_expiry_wakes_pending_waiters_and_restart_rejects_replay() {
    let store = HostApprovalStore::new();
    let owner = session();
    let conversation = crate::conversations::ConversationId::generate().unwrap();
    let (job_id, job) = job();
    let mut submitted = request(owner, job_id, conversation, 1, "true", "");
    submitted.token = store.submit(submitted.clone()).unwrap();
    let waiting = store.wait(&submitted.token, &job);
    tokio::pin!(waiting);
    assert!(futures_util::poll!(&mut waiting).is_pending());
    store.retain_sessions(|_| false);
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
            .await
            .unwrap(),
        Err(ApprovalError::Cancelled)
    );
    assert!(store.pending_for(conversation, job_id).is_none());
    let restarted = HostApprovalStore::new();
    assert_eq!(
        restarted.decide(&submitted, HostCommandDecision::Approved),
        Err(ApprovalError::Invalid)
    );
}

#[test]
fn workflow_request_identity_includes_run_step_and_attempt() {
    let store = HostApprovalStore::new();
    let owner = session();
    let conversation = crate::conversations::ConversationId::generate().unwrap();
    let (job_id, _) = job();
    let mut bound = request(owner, job_id, conversation, 1, "true", "diagnose");
    bound.run = Some("a".repeat(32));
    bound.step = Some("diagnose".to_owned());
    bound.attempt = Some("b".repeat(32));
    bound.token = store.submit(bound.clone()).unwrap();
    for field in ["run", "step", "attempt"] {
        let mut tampered = bound.clone();
        match field {
            "run" => tampered.run = Some("c".repeat(32)),
            "step" => tampered.step = Some("other".to_owned()),
            _ => tampered.attempt = Some("d".repeat(32)),
        }
        assert_eq!(
            store.decide(&tampered, HostCommandDecision::Approved),
            Err(ApprovalError::Invalid)
        );
    }
    store.decide(&bound, HostCommandDecision::Approved).unwrap();
}

#[test]
fn conversation_invalidation_drops_pending_host_commands() {
    let store = HostApprovalStore::new();
    let owner = session();
    let conversation = crate::conversations::ConversationId::generate().unwrap();
    let other = crate::conversations::ConversationId::generate().unwrap();
    let (job_id, _) = job();
    let (other_job, _) = job();
    let mut kept = request(owner, other_job, other, 1, "true", "");
    kept.token = store.submit(kept.clone()).unwrap();
    let mut dropped = request(owner, job_id, conversation, 1, "true", "");
    dropped.token = store.submit(dropped.clone()).unwrap();
    store.invalidate_conversation(conversation);
    assert!(store.pending_for(conversation, job_id).is_none());
    assert!(store.pending_for(other, other_job).is_some());
}
