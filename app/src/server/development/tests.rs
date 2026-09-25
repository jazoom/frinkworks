use super::*;
use axum::{body::Body, http::Request};
use tower::ServiceExt;

async fn idle_status(state: &AppState, token: Option<&str>) -> StatusCode {
    let mut request = Request::builder().uri("/_dev/supervised/idle");
    if let Some(token) = token {
        request = request.header("x-frinkworks-supervisor", token);
    }
    with_supervisor(Router::new(), state.clone(), "private-token".to_owned())
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn idle_probe_requires_the_launcher_token() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    assert_eq!(idle_status(&state, None).await, StatusCode::FORBIDDEN);
    assert_eq!(
        idle_status(&state, Some("other-token")).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        idle_status(&state, Some("private-token")).await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn restart_refuses_work_from_any_session_including_waiting_jobs_and_commands() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let session = crate::sessions::generate_session_token().unwrap().id();
    state.sessions.insert(session);
    let conversation = crate::conversations::ConversationId::generate().unwrap();
    let job = state
        .sessions
        .begin_conversation_job(&session, conversation)
        .unwrap();
    assert_eq!(
        idle_status(&state, Some("private-token")).await,
        StatusCode::CONFLICT
    );
    job.set_awaiting_decision();
    assert_eq!(
        idle_status(&state, Some("private-token")).await,
        StatusCode::CONFLICT
    );
    job.resume();
    job.set_awaiting_question();
    assert_eq!(
        idle_status(&state, Some("private-token")).await,
        StatusCode::CONFLICT
    );
    state
        .sessions
        .finish_conversation_job(&session, conversation, job.id());
    let reservation = state
        .sessions
        .reserve_command(session, conversation)
        .unwrap();
    assert_eq!(
        idle_status(&state, Some("private-token")).await,
        StatusCode::CONFLICT
    );
    drop(reservation);
    assert_eq!(
        idle_status(&state, Some("private-token")).await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn recovery_reservations_permit_restart_only_after_other_work_and_cleanup_finish() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let session = crate::sessions::generate_session_token().unwrap().id();
    state.sessions.insert(session);
    let conversation = crate::conversations::ConversationId::generate().unwrap();
    let job = state
        .sessions
        .begin_conversation_job(&session, conversation)
        .unwrap();
    let guard = state.workflow_execution.acquire().unwrap();
    let cleanup = state.workflow_execution.acquire().unwrap();
    job.finish(crate::sessions::JobStatus::Failed, None);
    guard.require_recovery();
    guard.require_recovery();
    assert!(state.sessions.conversation_reserved(conversation));
    assert_eq!(
        idle_status(&state, Some("private-token")).await,
        StatusCode::CONFLICT
    );
    drop(cleanup);
    let other = crate::conversations::ConversationId::generate().unwrap();
    let active = state
        .sessions
        .begin_conversation_job(&session, other)
        .unwrap();
    assert_eq!(
        idle_status(&state, Some("private-token")).await,
        StatusCode::CONFLICT
    );
    active.finish(crate::sessions::JobStatus::Completed, None);
    state
        .sessions
        .finish_conversation_job(&session, other, active.id());
    assert_eq!(
        idle_status(&state, Some("private-token")).await,
        StatusCode::NO_CONTENT
    );
    drop(guard);
    assert_eq!(
        idle_status(&state, Some("private-token")).await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn queued_environment_preparation_blocks_restart() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    state.environments.apply_production_seeds();
    assert_eq!(
        idle_status(&state, Some("private-token")).await,
        StatusCode::CONFLICT
    );
}
