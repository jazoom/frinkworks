use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
    middleware::from_fn_with_state,
    routing::get,
};
use cookie::Cookie;
use hypergraft::live::LiveGuard;
use tower::ServiceExt;

use crate::agents::{AgentId, DirectoryPolicy};
use crate::providers::{ProviderConnection, ProviderKind};
use crate::sessions::{Job, JobId, SESSION_LIFETIME, SessionId, ValidatedToken};
use crate::state::AppState;
use crate::workflows::RunId;

fn state_with_gate(name: &str) -> (AppState, ValidatedToken, SessionId, RunId) {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    state
        .vault
        .put(ProviderConnection::with_key(
            ProviderKind::Xai,
            "key",
            "model",
        ))
        .expect("provider");
    let token = super::generate_session_token().expect("token");
    let session = token.id();
    let raw = token.raw().clone();
    state.sessions.insert(session);

    let definition = crate::tests::test_named_definition(name);
    let environments = crate::tests::test_environment_set(&definition);
    let mut run = crate::workflows::WorkflowRun::configured(
        RunId::generate().expect("run"),
        1,
        AgentId::generate().expect("agent"),
        crate::workflows::definition::PinnedWorkflowDefinition::pin(None, definition),
        environments,
    );
    run.state = crate::workflows::run::RunState::InitialisingSource;
    let run_id = run.id;
    state.workflow_runs.create(run).expect("store run");
    let continuation = crate::workflows::WorkflowJob {
        run_id,
        session_id: session,
        agent_id: Some(AgentId::generate().expect("agent")),
        agent_revision: 1,
        conversation_id: None,
        authority: None,
        project_free_authority: None,
        grant_alias: "project".to_owned(),
        connection: ProviderConnection::with_key(ProviderKind::Xai, "key", "model"),
        phase_providers: Vec::new(),
        active_connection: Arc::new(std::sync::Mutex::new(None)),
        host_policy: DirectoryPolicy::from_grants(Vec::new(), "project".to_owned()),
        turns: Vec::new(),
        job: Job::new(JobId::generate().expect("job"), run_id, 0),
        eligible_reply: Arc::new(std::sync::Mutex::new(String::new())),
    };
    assert!(state.gate_continuations.insert(continuation));
    (state, raw, session, run_id)
}

#[tokio::test]
async fn browser_language_belongs_to_the_resolved_live_session() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    state
        .vault
        .put(ProviderConnection::with_key(
            ProviderKind::Xai,
            "key",
            "model",
        ))
        .unwrap();
    let token = super::generate_session_token().unwrap();
    let other = super::generate_session_token().unwrap();
    state.sessions.insert(token.id());
    state.sessions.insert(other.id());
    let app = axum::Router::new()
        .route("/private", get(|| async { StatusCode::OK }))
        .layer(from_fn_with_state(state.clone(), super::resolve_session))
        .with_state(state.clone());
    let request = Request::builder()
        .uri("/private")
        .header(
            header::COOKIE,
            format!("powerplant_session={}", token.raw().as_str()),
        )
        .header(header::ACCEPT_LANGUAGE, "en-GB,en;q=0.9")
        .body(Body::empty())
        .unwrap();
    let language = super::BrowserLanguage::from_headers(request.headers()).unwrap();
    assert_eq!(app.oneshot(request).await.unwrap().status(), StatusCode::OK);
    assert_eq!(state.sessions.language(&token.id()), Some(language.clone()));
    assert_eq!(state.sessions.language(&other.id()), None);
    state
        .sessions
        .advance_clock(SESSION_LIFETIME + std::time::Duration::from_secs(1));
    state.sessions.set_language(&token.id(), language);
    assert_eq!(state.sessions.language(&token.id()), None);
}

#[tokio::test]
async fn the_live_guard_rejects_anonymous_and_removed_sessions() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    state
        .vault
        .put(ProviderConnection::with_key(
            ProviderKind::Xai,
            "key",
            "model",
        ))
        .expect("provider");
    let token = super::generate_session_token().expect("token");
    let session = token.id();
    state.sessions.insert(session);
    let guard = super::LiveSessionGuard::new(state.sessions.clone(), state.vault.clone());

    assert!(matches!(
        guard.bind(&axum::http::Extensions::new()).await,
        Err(hypergraft::live::GuardFailure::Terminal)
    ));

    let mut extensions = axum::http::Extensions::new();
    extensions.insert(super::ResolvedSession::Present(session));
    let connection = guard.bind(&extensions).await.expect("live connection");
    assert_eq!(guard.revalidate(&connection).await, Ok(session));

    state.sessions.remove(&session);
    assert_eq!(
        guard.revalidate(&connection).await,
        Err(hypergraft::live::GuardFailure::Terminal)
    );
}

#[test]
fn request_time_expiry_interrupts_a_gate_before_session_restore() {
    let (state, token, session, run_id) = state_with_gate("Expiry");
    state
        .sessions
        .advance_clock(SESSION_LIFETIME + std::time::Duration::from_secs(1));

    assert!(matches!(
        super::existing_or_restore(&state, &token),
        super::ResolvedSession::Present(id) if id == session
    ));

    assert!(!state.gate_continuations.available(&run_id, &session));
    assert_eq!(
        state.workflow_runs.get(&run_id).expect("run").state,
        crate::workflows::run::RunState::Interrupted
    );
    assert!(state.sessions.contains_live(&session));
}

#[tokio::test]
async fn a_live_cookie_after_final_provider_removal_revokes_the_session() {
    let (state, token, session, run_id) = state_with_gate("Forget");
    state.vault.forget(ProviderKind::Xai).expect("forget");
    assert!(!state.vault.has_providers());
    assert!(state.sessions.contains_live(&session));

    let app = axum::Router::new()
        .route(
            "/private",
            get(|_session: super::RequiredSession| async { StatusCode::NO_CONTENT }),
        )
        .layer(from_fn_with_state(state.clone(), super::resolve_session))
        .layer(axum::middleware::from_fn(hypergraft::middleware::classify))
        .with_state(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/private")
                .header(
                    header::COOKIE,
                    format!("powerplant_session={}", token.as_str()),
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get(header::LOCATION).unwrap(),
        "/connect"
    );
    let deletion = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| Cookie::parse(value.to_owned()).ok())
        .find(|cookie| cookie.name() == "powerplant_session")
        .expect("session deletion cookie");
    assert!(deletion.value().is_empty());
    assert_eq!(deletion.max_age(), Some(cookie::time::Duration::ZERO));
    assert!(!state.gate_continuations.available(&run_id, &session));
    assert_eq!(
        state.workflow_runs.get(&run_id).expect("run").state,
        crate::workflows::run::RunState::Interrupted
    );
    assert!(!state.sessions.contains(&session));
}
