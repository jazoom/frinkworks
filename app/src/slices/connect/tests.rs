use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    middleware::from_fn_with_state,
};
use tower::ServiceExt;

use crate::{
    config::RuntimeConfig,
    plan_login::PendingPlan,
    providers::{
        ChatBackend, ProviderConnection, ProviderError, ProviderKind, tests::ScriptedBackend,
    },
    sessions::{self, SESSION_LIFETIME, SessionId, ValidatedToken},
    state::AppState,
    vault::ProviderVault,
};

const SECRET_KEY: &str = "sk-test-secret-key-do-not-echo";

fn test_state() -> AppState {
    crate::tests::test_state(RuntimeConfig::development())
}

fn app(state: AppState) -> axum::Router {
    crate::slices::router()
        .layer(from_fn_with_state(
            state.clone(),
            crate::sessions::resolve_session,
        ))
        .layer(axum::middleware::from_fn(hypergraft::middleware::classify))
        .with_state(state)
}

fn state_with_backend(backend: ScriptedBackend) -> AppState {
    let mut state = test_state();
    state.chat = Arc::new(ChatBackend::Scripted(backend));
    state
}

fn store_provider(state: &AppState, key: &str) {
    state
        .vault
        .put(ProviderConnection::with_key(
            ProviderKind::Xai,
            key,
            "grok-4.6",
        ))
        .expect("vault");
}

fn connected(state: &AppState) -> String {
    state.environments.apply_production_seeds();
    store_provider(state, SECRET_KEY);
    let token = sessions::generate_session_token().expect("session token");
    state.sessions.insert(token.id());
    token.raw().as_str().to_owned()
}

fn cookie(token: &str) -> String {
    format!("powerplant_session={token}")
}

fn session_id(token: &str) -> SessionId {
    SessionId::from_validated(&ValidatedToken::parse(token).expect("token"))
}

fn set_cookies(response: &axum::http::Response<Body>) -> Vec<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().expect("cookie utf8").to_owned())
        .collect()
}

async fn wait_flag(flag: &AtomicBool, message: &str) {
    for _ in 0..2_000 {
        if flag.load(Ordering::SeqCst) {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("{message}");
}

#[tokio::test]
async fn an_expired_cookie_without_a_vault_cannot_resolve_a_session() {
    let state = test_state();
    let token = sessions::generate_session_token().expect("session token");
    state.sessions.insert(token.id());
    let token = token.raw().as_str().to_owned();
    let id = session_id(&token);
    state
        .sessions
        .advance_clock(SESSION_LIFETIME + Duration::from_secs(1));

    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .uri("/")
                .header(header::COOKIE, cookie(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("chat");

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get(header::LOCATION).unwrap(),
        "/connect"
    );
    let cookies = set_cookies(&response);
    assert!(
        cookies
            .iter()
            .any(|value| value.contains("powerplant_session="))
    );
    assert!(cookies.iter().all(|value| !value.contains(&token)));
    assert!(cookies.iter().all(|value| !value.contains(SECRET_KEY)));
    assert!(!state.sessions.contains(&id));
}

#[tokio::test]
async fn forget_of_the_last_provider_stops_an_active_stream() {
    let started = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let state = state_with_backend(ScriptedBackend::hang_watched(
        started.clone(),
        dropped.clone(),
    ));
    let token = connected(&state);
    let id = session_id(&token);
    let conversation = state
        .conversations
        .create("Provider cancellation".to_owned())
        .expect("conversation");

    let send = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/conversations/{}/messages", conversation.id))
                .header(header::COOKIE, cookie(&token))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from(format!(
                    "message=Hello&revision={}",
                    conversation.revision
                )))
                .unwrap(),
        )
        .await
        .expect("chat send");
    assert_eq!(send.status(), StatusCode::OK);
    wait_flag(&started, "provider stream did not start").await;

    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/connect/forget")
                .header(header::COOKIE, cookie(&token))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from("provider=xai"))
                .unwrap(),
        )
        .await
        .expect("forget");

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(header::LOCATION).is_none());
    let cookies = set_cookies(&response);
    assert!(
        cookies
            .iter()
            .any(|value| value.contains("powerplant_session="))
    );
    assert!(cookies.iter().all(|value| !value.contains(&token)));
    assert!(cookies.iter().all(|value| !value.contains(SECRET_KEY)));
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains(r#"navigate="/connect""#));
    assert!(!text.contains(SECRET_KEY));
    assert!(!state.sessions.contains(&id));
    assert!(!state.vault.has_providers());
    wait_flag(&dropped, "provider stream was not dropped").await;
}

#[tokio::test]
async fn forget_of_the_last_provider_clears_the_session_despite_preference_failure() {
    let mut state = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.preferences = Arc::new(crate::preferences::Preferences::open(
        dir.path().to_path_buf(),
    ));
    let token = connected(&state);
    let id = session_id(&token);

    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/connect/forget")
                .header(header::COOKIE, cookie(&token))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from("provider=xai"))
                .unwrap(),
        )
        .await
        .expect("forget");

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(header::LOCATION).is_none());
    let cookies = set_cookies(&response);
    assert!(
        cookies
            .iter()
            .any(|value| value.contains("powerplant_session="))
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains(r#"navigate="/connect""#));
    assert!(!text.contains(&token));
    assert!(!text.contains(SECRET_KEY));
    assert!(!state.sessions.contains(&id));
    assert!(!state.vault.has_providers());
}

#[tokio::test]
async fn forget_of_one_provider_keeps_the_rest_despite_preference_failure() {
    let mut state = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.preferences = Arc::new(crate::preferences::Preferences::open(
        dir.path().to_path_buf(),
    ));
    store_provider(&state, SECRET_KEY);
    state
        .vault
        .put(ProviderConnection::with_key(
            ProviderKind::Synthetic,
            "sk-second-key",
            "hf:custom",
        ))
        .expect("vault");
    let token = sessions::generate_session_token().expect("session token");
    state.sessions.insert(token.id());
    let token = token.raw().as_str().to_owned();
    let id = session_id(&token);

    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/connect/forget")
                .header(header::COOKIE, cookie(&token))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from("provider=xai"))
                .unwrap(),
        )
        .await
        .expect("forget");

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(header::LOCATION).is_none());
    assert!(state.sessions.contains(&id));
    assert!(!state.vault.contains(ProviderKind::Xai));
    assert!(state.vault.contains(ProviderKind::Synthetic));
    let cookies = set_cookies(&response);
    assert!(cookies.iter().all(|value| !value.contains(SECRET_KEY)));
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("Synthetic"));
    assert!(!text.contains(SECRET_KEY));
}

#[tokio::test]
async fn a_failed_plan_forget_restores_the_provider_and_session() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("providers.json");
    let staged = dir.path().join("staged-xai.json");
    let plan_path = dir.path().join("xai-auth.json");
    std::fs::write(&staged, br#"{"access_token":"plan-secret"}"#).unwrap();
    let mut state = test_state();
    state.vault = Arc::new(ProviderVault::open(path).expect("vault"));
    state
        .vault
        .install_plan(ProviderKind::Xai, &staged)
        .expect("plan");
    let token = sessions::generate_session_token().expect("session token");
    state.sessions.insert(token.id());
    let token = token.raw().as_str().to_owned();
    let id = session_id(&token);
    state.vault.fail_next_marker_remove();

    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/connect/forget")
                .header(header::COOKIE, cookie(&token))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from("provider=xai"))
                .unwrap(),
        )
        .await
        .expect("forget");

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(response.headers().get(header::LOCATION).is_none());
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(text, "Internal error");
    assert!(!text.contains("plan-secret"));
    assert!(state.vault.contains(ProviderKind::Xai));
    assert!(state.sessions.contains(&id));
    assert!(plan_path.exists());
}

fn connect_form() -> &'static str {
    "provider=xai&api_key=sk-test-key"
}

async fn connect_patch(state: AppState) -> axum::http::Response<Body> {
    app(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/connect")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from(connect_form()))
                .unwrap(),
        )
        .await
        .expect("connect")
}

#[tokio::test]
async fn a_rejected_key_returns_an_unauthorised_result() {
    let mut backend = ScriptedBackend::accept();
    backend.verify_result = Err(ProviderError::Rejected);
    let response = connect_patch(state_with_backend(backend)).await;
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(text.contains("That key was rejected"));
    assert_field_target(&text, "connect-key", "sk-test-key");
}

#[tokio::test]
async fn an_outage_does_not_tell_the_user_to_replace_a_valid_key() {
    let mut backend = ScriptedBackend::accept();
    backend.verify_result = Err(ProviderError::Unreachable);
    let response = connect_patch(state_with_backend(backend)).await;
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(text.contains("The provider could not be reached"));
    assert!(!text.contains("key was rejected"));
    assert_field_target(&text, "connect-provider", "sk-test-key");
}

#[tokio::test]
async fn a_rate_limit_returns_a_typed_retry_interval() {
    let mut backend = ScriptedBackend::accept();
    backend.verify_result = Err(ProviderError::RateLimited {
        retry_after: hypergraft::RetryAfter::seconds(7),
    });
    let response = connect_patch(state_with_backend(backend)).await;
    let status = response.status();
    let retry_after = response
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(retry_after.as_deref(), Some("7"));
    assert!(text.contains("rate-limited"));
    assert_field_target(&text, "connect-provider", "sk-test-key");
}

async fn connect_submit(
    state: AppState,
    body: String,
    enhanced: bool,
) -> axum::http::Response<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/connect")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if enhanced {
        request = request
            .header(hypergraft::GRAFT_REQUEST, "patch")
            .header(header::ACCEPT, hypergraft::MEDIA_TYPE);
    }
    app(state)
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .expect("connect")
}

fn assert_field_target(text: &str, target: &str, secret: &str) {
    assert!(text.contains(&format!("href=\"#{target}\"")));
    assert_eq!(text.matches("aria-invalid").count(), 1);
    assert_eq!(text.matches("connect-errors").count(), 2);
    assert_eq!(aria_invalid_control(text), target);
    assert!(!text.contains(secret));
}

fn has_checked_control(text: &str) -> bool {
    text.replace("has-checked", "").contains("checked")
}

fn aria_invalid_control(text: &str) -> &str {
    let aria = text.find("aria-invalid").expect("aria-invalid");
    let start = text[..aria].rfind('<').expect("tag open");
    let tag = &text[start..aria];
    for id in ["connect-provider", "connect-key", "connect-plan"] {
        if tag.contains(id) {
            return id;
        }
    }
    panic!("aria-invalid on unknown control: {tag}");
}

#[tokio::test]
async fn a_provider_error_targets_the_provider_fieldset() {
    let response = connect_submit(
        test_state(),
        "provider=openai&api_key=sk-secret-key".to_owned(),
        true,
    )
    .await;
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(text.contains(r#"operation="children" target="connect-card""#));
    assert!(text.contains("Choose a provider."));
    assert_field_target(&text, "connect-provider", "sk-secret-key");
    assert!(!has_checked_control(&text));
}

#[tokio::test]
async fn an_api_key_error_targets_the_key_control() {
    let response = connect_submit(test_state(), "provider=xai&api_key=".to_owned(), true).await;
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(text.contains("Enter an API key."));
    assert_field_target(&text, "connect-key", "sk-secret-key");
    assert!(text.contains(r#"value="xai""#));
    assert!(!has_checked_control(&text));
    assert!(text.contains("Connect to xAI (Grok)"));
}

#[tokio::test]
async fn a_native_connect_is_rejected_before_validation() {
    let response = connect_submit(
        test_state(),
        "provider=openai&api_key=sk-secret-key".to_owned(),
        false,
    )
    .await;
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(text, "Bad request");
    assert!(!text.contains("sk-secret-key"));
}

#[tokio::test]
async fn a_successful_connect_stores_the_provider_without_echoing_the_key() {
    let state = test_state();
    let response = connect_submit(
        state.clone(),
        "provider=xai&api_key=sk-test-key".to_owned(),
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains(r#"navigate="/conversations""#));
    assert!(!text.contains("Continue to projects"));
    assert!(!text.contains("sk-test-key"));
    assert!(state.vault.contains(ProviderKind::Xai));
}

#[tokio::test]
async fn connect_stays_available_when_providers_are_stored() {
    let state = test_state();
    let token = connected(&state);
    let response = app(state)
        .oneshot(
            Request::builder()
                .uri("/connect")
                .header(header::COOKIE, cookie(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("connect");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("xAI (Grok)"));
    assert!(text.contains("action=\"/connect/forget\""));
    assert!(text.contains("id=\"chat-main\""));
    assert!(text.contains("data-section=\"providers\""));
    assert!(!text.contains("Back to the local desk"));
    assert!(!text.contains("class=\"connect-frame\""));
    assert!(!text.contains(SECRET_KEY));
}

#[tokio::test]
async fn a_new_process_restores_a_session_from_the_vault_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("providers.json");
    let mut writer = test_state();
    writer.vault = Arc::new(ProviderVault::open(path.clone()).expect("vault"));
    store_provider(&writer, SECRET_KEY);

    let mut reader = test_state();
    reader.vault = Arc::new(ProviderVault::open(path).expect("vault"));
    let response = app(reader)
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .expect("chat");

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get(header::LOCATION).unwrap(),
        "/conversations"
    );
    let cookies = set_cookies(&response);
    assert!(
        cookies
            .iter()
            .any(|value| value.contains("powerplant_session="))
    );
    assert!(cookies.iter().all(|value| !value.contains(SECRET_KEY)));
}

#[tokio::test]
async fn adding_a_second_provider_in_setup_keeps_the_first() {
    let state = test_state();
    store_provider(&state, SECRET_KEY);
    let response = connect_submit(
        state.clone(),
        "provider=synthetic&api_key=sk-second-key".to_owned(),
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains(r#"navigate="/conversations""#));
    assert!(state.vault.contains(ProviderKind::Xai));
    assert!(state.vault.contains(ProviderKind::Synthetic));
}

const PLAN_TOKEN: &str = "xai-plan-access-do-not-echo";
const PLAN_URI: &str = "https://accounts.x.ai/connect";
const PLAN_CODE: &str = "ABCD-EFGH";

fn pending_state() -> AppState {
    let state = test_state();
    state.plan_login.force_pending(PendingPlan {
        kind: ProviderKind::Xai,
        verification_uri: PLAN_URI.to_owned(),
        user_code: PLAN_CODE.to_owned(),
        error: None,
    });
    state
}

async fn connect_get(state: AppState, kind: Option<&str>) -> axum::http::Response<Body> {
    connect_get_at(state, "/connect", kind).await
}

async fn connect_get_at(
    state: AppState,
    uri: &str,
    kind: Option<&str>,
) -> axum::http::Response<Body> {
    let mut request = Request::builder().uri(uri);
    if let Some(kind) = kind {
        request = request
            .header(hypergraft::GRAFT_REQUEST, kind)
            .header(header::ACCEPT, hypergraft::MEDIA_TYPE);
    }
    app(state)
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .expect("connect")
}

#[tokio::test]
async fn pending_login_document_shows_the_url_and_code() {
    let response = connect_get(pending_state(), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("<!doctype html>"));
    assert!(text.contains(PLAN_URI));
    assert!(text.contains(PLAN_CODE));
}

#[tokio::test]
async fn pending_login_navigation_patches_the_page() {
    let response = connect_get(pending_state(), Some("navigation")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains(r#"operation="children" target="connect-main""#));
    assert!(text.contains(PLAN_URI));
    assert!(text.contains(PLAN_CODE));
}

#[tokio::test]
async fn stored_provider_navigation_patches_the_app_shell() {
    let state = test_state();
    store_provider(&state, SECRET_KEY);
    let response = connect_get(state, Some("navigation")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains(r#"operation="children" target="chat-main""#));
    assert!(text.contains("data-section=\"providers\""));
    assert!(!text.contains("<!doctype html>"));
    assert!(!text.contains(SECRET_KEY));
}

#[tokio::test]
async fn pending_login_patch_updates_the_card() {
    let response = connect_get(pending_state(), Some("patch")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains(r#"operation="children" target="connect-card""#));
    assert!(text.contains(PLAN_URI));
    assert!(text.contains(PLAN_CODE));
    assert!(!text.contains("<!doctype html>"));
}

#[tokio::test]
async fn a_stored_plan_token_is_not_echoed_in_html_or_cookies() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("providers.json");
    let staged = dir.path().join("staged-xai.json");
    std::fs::write(&staged, format!(r#"{{"access_token":"{PLAN_TOKEN}"}}"#)).unwrap();
    let mut state = test_state();
    state.vault = Arc::new(ProviderVault::open(path).expect("vault"));
    state
        .vault
        .install_plan(ProviderKind::Xai, &staged)
        .unwrap();

    let response = connect_get(state, None).await;
    let cookies = set_cookies(&response);
    assert!(cookies.iter().all(|value| !value.contains(PLAN_TOKEN)));
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(!text.contains(PLAN_TOKEN));
}

#[tokio::test]
async fn an_unknown_plan_provider_is_rejected() {
    let response = app(test_state())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/connect/plan")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from("provider=synthetic"))
                .unwrap(),
        )
        .await
        .expect("plan");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(text.contains("Choose ChatGPT or SuperGrok."));
    assert!(text.contains("href=\"#connect-plan\""));
}

#[tokio::test]
async fn empty_vault_offers_both_plan_logins_and_every_api_key_provider() {
    let response = connect_get(test_state(), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(text.contains("Which provider will you use?"));
    assert!(text.contains("ChatGPT plan or API key"));
    assert!(text.contains("SuperGrok plan or API key"));
    for kind in ProviderKind::ALL {
        assert!(text.contains(&format!("href=\"/connect?provider={}\"", kind.as_str())));
    }
    assert!(!text.contains(r#"name="api_key""#));
    assert!(!text.contains("Connected providers"));
    assert!(!text.contains("Continue to conversations"));
}

#[tokio::test]
async fn provider_selection_supports_documents_navigation_and_patches() {
    for (kind, target) in [
        (None, None),
        (Some("navigation"), Some("connect-main")),
        (Some("patch"), Some("connect-card")),
    ] {
        let response = connect_get_at(test_state(), "/connect?provider=openai-codex", kind).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        if let Some(target) = target {
            assert!(text.contains(&format!("target=\"{target}\"")));
        } else {
            assert!(text.contains("<!doctype html>"));
        }
        assert_eq!(
            text.matches(r#"name="provider" value="openai-codex""#)
                .count(),
            2
        );
        assert!(!text.contains(r#"name="provider" value="xai""#));
    }
}

#[tokio::test]
async fn malformed_provider_queries_fail_without_echoing_input() {
    for query in [
        "provider=unknown-secret",
        "provider=xai&provider=deepseek",
        "plan=unknown-secret",
    ] {
        for kind in [None, Some("navigation"), Some("patch")] {
            let response = connect_get_at(test_state(), &format!("/connect?{query}"), kind).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            assert_eq!(body, "Bad request");
        }
    }
}

#[tokio::test]
async fn a_plan_poll_preserves_the_selected_provider() {
    let response = connect_get_at(
        pending_state(),
        "/connect?provider=deepseek&plan=xai",
        Some("patch"),
    )
    .await;
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains(r#"name="provider" value="deepseek""#));
    assert!(!text.contains(r#"name="provider" value="xai""#));
    assert!(text.contains(PLAN_CODE));
}

#[tokio::test]
async fn a_completed_plan_poll_navigates_to_conversations() {
    let state = test_state();
    store_provider(&state, SECRET_KEY);
    let response = connect_get_at(state, "/connect?provider=xai&plan=xai", Some("patch")).await;
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains(r#"navigate="/conversations""#));
    assert!(!text.contains(SECRET_KEY));
}

#[tokio::test]
async fn stored_provider_hides_its_own_plan_login_and_key_option() {
    let state = test_state();
    store_provider(&state, SECRET_KEY);
    let response = connect_get(state, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(text.contains(r#"action="/connect/forget""#));
    for provider in ["openai-codex", "synthetic", "openrouter", "deepseek"] {
        assert!(text.contains(&format!(r#"href="/connect?provider={provider}""#)));
    }
    assert!(!text.contains(r#"href="/connect?provider=xai""#));
    assert_eq!(text.matches(r#"name="provider" value="xai""#).count(), 1);
    assert!(!text.contains(SECRET_KEY));
}
