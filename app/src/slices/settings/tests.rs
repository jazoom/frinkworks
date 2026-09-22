use std::path::{Path, PathBuf};
use std::process::Command;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
    middleware::from_fn_with_state,
};
use tower::ServiceExt;

use crate::{
    agents::{AccessMode, AgentDraft, DirectoryGrant, ToolId},
    config::{RuntimeConfig, StartupConfig},
    local_data::CatalogueResetConflict,
    preferences::{CompactionPreference, Theme},
    providers::{ProviderConnection, ProviderKind},
    sessions,
    state::AppState,
};

use super::page;

fn test_state() -> AppState {
    crate::tests::test_state(RuntimeConfig::development())
}

fn app(state: &AppState) -> axum::Router {
    crate::slices::router()
        .layer(from_fn_with_state(
            state.clone(),
            crate::sessions::resolve_session,
        ))
        .layer(axum::middleware::from_fn(hypergraft::middleware::classify))
        .with_state(state.clone())
}

fn connected(state: &AppState) -> String {
    let token = sessions::generate_session_token().expect("session token");
    state
        .vault
        .put(ProviderConnection::with_key(
            ProviderKind::Xai,
            "test-key",
            "grok-4.6",
        ))
        .expect("vault");
    state.sessions.insert(token.id());
    token.raw().as_str().to_owned()
}

fn cookie(token: &str) -> String {
    format!("powerplant_session={token}")
}

#[tokio::test]
async fn settings_supports_documents_and_navigation_only() {
    let state = test_state();
    let token = connected(&state);

    let document = app(&state)
        .oneshot(
            Request::builder()
                .uri("/settings")
                .header(header::COOKIE, cookie(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("document");
    assert_eq!(document.status(), StatusCode::OK);
    let body = to_bytes(document.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(text.matches("id=\"chat-main\"").count(), 1);

    let navigation = app(&state)
        .oneshot(
            Request::builder()
                .uri("/settings")
                .header(header::COOKIE, cookie(&token))
                .header(hypergraft::GRAFT_REQUEST, "navigation")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("navigation");
    assert_eq!(navigation.status(), StatusCode::OK);
    let body = to_bytes(navigation.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("operation=\"children\" target=\"chat-main\""));

    let patch = app(&state)
        .oneshot(
            Request::builder()
                .uri("/settings")
                .header(header::COOKIE, cookie(&token))
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("patch");
    assert_eq!(patch.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_initial_document_renders_the_saved_theme_and_selection() {
    let state = test_state();
    state
        .preferences
        .set_theme(Theme::EvergreenTerrace)
        .expect("theme");
    let token = connected(&state);

    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/settings")
                .header(header::COOKIE, cookie(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("settings");
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();

    let root = text
        .split_once("<html")
        .unwrap()
        .1
        .split_once('>')
        .unwrap()
        .0;
    assert!(root.contains("data-theme=\"evergreen-terrace\""));
    assert!(text.contains("data-active-theme=\"evergreen-terrace\""));
    assert_eq!(text.matches("selected").count(), 1);
}

#[tokio::test]
async fn a_theme_patch_persists_and_returns_the_authoritative_selector() {
    let state = test_state();
    let token = connected(&state);

    let response = app(&state)
        .oneshot(theme_request(&token, "sector-7-g"))
        .await
        .expect("theme patch");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.preferences.theme(), Theme::Sector7G);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("operation=\"children\" target=\"theme-setting\""));
    assert!(text.contains("data-active-theme=\"sector-7-g\""));
    assert_eq!(text.matches("selected").count(), 1);
}

#[tokio::test]
async fn an_unknown_theme_is_rejected_without_changing_the_preference() {
    let state = test_state();
    let token = connected(&state);

    let response = app(&state)
        .oneshot(theme_request(&token, "unknown"))
        .await
        .expect("theme patch");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(state.preferences.theme(), Theme::System);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("Choose a listed theme."));
    assert!(text.contains("data-active-theme=\"system\""));
}

#[tokio::test]
async fn clearing_conversation_defaults_requires_a_command_and_keeps_the_theme() {
    let state = test_state();
    state.preferences.set_theme(Theme::Sector7G).expect("theme");
    state
        .preferences
        .set_conversation_defaults(saved_defaults())
        .expect("defaults");
    let token = connected(&state);

    let mut native = patch_form(
        &token,
        "/settings/conversation-defaults/clear",
        String::new(),
    );
    native.headers_mut().remove(hypergraft::GRAFT_REQUEST);
    assert_eq!(
        app(&state).oneshot(native).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    assert!(state.preferences.conversation_defaults().is_some());

    let response = app(&state)
        .oneshot(patch_form(
            &token,
            "/settings/conversation-defaults/clear",
            String::new(),
        ))
        .await
        .expect("clear");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(state.preferences.conversation_defaults().is_none());
    assert_eq!(state.preferences.theme(), Theme::Sector7G);
    let text = body_text(response).await;
    assert!(text.contains("target=\"conversation-defaults-setting\""));
}

fn saved_defaults() -> crate::execution::ExecutionSettings {
    crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None)
            .unwrap(),
        "Use concise explanations.".to_owned(),
        vec![crate::agents::ToolId::Read],
        crate::tests::test_environment_id(),
    )
    .unwrap()
}

fn theme_request(token: &str, theme: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/settings/theme")
        .header(header::COOKIE, cookie(token))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(hypergraft::GRAFT_REQUEST, "patch")
        .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
        .body(Body::from(format!("theme={theme}")))
        .unwrap()
}

fn owned_state() -> AppState {
    let dir = tempfile::tempdir().expect("dir");
    let config = StartupConfig {
        bind_address: "localhost:4000".to_owned(),
        runtime: RuntimeConfig::development(),
        static_dir: PathBuf::from("/tmp/powerplant-static"),
        data_dir: dir.path().join("data"),
        protected_user_roots: Vec::new(),
    };
    let (_, local_data) = crate::local_data::prepare(config).expect("prepare");
    let mut state = test_state();
    state.local_data = local_data;
    state.keep_temp_dir(dir);
    state
}

fn git_init(path: &Path) {
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(path)
            .status()
            .expect("git")
            .success()
    );
}

fn git_worktree_under(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::create_dir_all(&path).expect("dir");
    git_init(&path);
    path.canonicalize().expect("canonical")
}

fn create_agent(state: &AppState, name: &str, path: &Path) -> crate::agents::AgentRecord {
    state
        .agents
        .create(AgentDraft {
            name: name.to_owned(),
            instructions: String::new(),
            selection: None,
            tools: ToolId::ALL.to_vec(),
            network: crate::agents::NetworkAccess::None,
            directories: vec![DirectoryGrant {
                alias: "project".to_owned(),
                host_path: path.to_path_buf(),
                access: AccessMode::ReadWrite,
            }],
            primary_directory: "project".to_owned(),
        })
        .expect("agent")
}

fn patch_form(token: &str, uri: &str, body: String) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::COOKIE, cookie(token))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(hypergraft::GRAFT_REQUEST, "patch")
        .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
        .body(Body::from(body))
        .unwrap()
}

fn reset_request(token: &str, body: &str) -> Request<Body> {
    patch_form(token, "/settings/local-data/reset", body.to_owned())
}

fn compaction_request(token: &str, body: &str) -> Request<Body> {
    patch_form(token, "/settings/compaction", body.to_owned())
}

async fn body_text(response: axum::http::Response<Body>) -> String {
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}

fn assert_no_paths(text: &str, paths: &[&Path]) {
    for path in paths {
        let displayed = path.to_string_lossy();
        if displayed.is_empty() {
            continue;
        }
        assert!(
            !text.contains(displayed.as_ref()),
            "response included path {displayed}: {text}"
        );
    }
}

#[tokio::test]
async fn compaction_patch_persists_valid_values_and_returns_the_authoritative_setting() {
    let state = test_state();
    let token = connected(&state);

    let response = app(&state)
        .oneshot(compaction_request(&token, "enabled=on&threshold=80"))
        .await
        .expect("compaction");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        state.preferences.compaction(),
        CompactionPreference {
            enabled: true,
            threshold: 80,
        }
    );
    let text = body_text(response).await;
    assert!(text.contains("target=\"compaction-setting\""));
    assert!(text.contains("value=\"80\""));
    assert!(text.contains("checked"));
}

#[tokio::test]
async fn compaction_rejects_non_command_representations_without_a_mutation() {
    let state = test_state();
    let token = connected(&state);
    for representation in [None, Some("navigation")] {
        let mut request = compaction_request(&token, "threshold=1");
        request.headers_mut().remove(hypergraft::GRAFT_REQUEST);
        if let Some(value) = representation {
            request
                .headers_mut()
                .insert(hypergraft::GRAFT_REQUEST, value.parse().unwrap());
        }
        let response = app(&state).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            state.preferences.compaction(),
            CompactionPreference::default()
        );
    }
}

#[tokio::test]
async fn compaction_patch_accepts_a_disabled_policy() {
    let state = test_state();
    let token = connected(&state);

    let response = app(&state)
        .oneshot(compaction_request(&token, "threshold=95"))
        .await
        .expect("compaction");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        state.preferences.compaction(),
        CompactionPreference {
            enabled: false,
            threshold: 95,
        }
    );
    let text = body_text(response).await;
    assert!(!text.contains("checked"));
}

#[tokio::test]
async fn an_out_of_range_compaction_percentage_is_rejected() {
    let state = test_state();
    let token = connected(&state);
    for body in [
        "threshold=0",
        "threshold=101",
        "threshold=nine",
        "threshold=95.5",
    ] {
        let response = app(&state)
            .oneshot(compaction_request(&token, body))
            .await
            .expect("compaction");
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body}"
        );
        let text = body_text(response).await;
        assert!(text.contains("target=\"compaction-setting\""), "{body}");
        assert!(text.contains(page::COMPACTION_RANGE), "{body}");
        assert_eq!(
            state.preferences.compaction(),
            CompactionPreference::default(),
            "{body}"
        );
    }
}

#[tokio::test]
async fn a_malformed_compaction_form_is_rejected_without_changing_the_preference() {
    let state = test_state();
    let token = connected(&state);

    for body in [
        "",
        "enabled=on",
        "threshold=95&threshold=1",
        "threshold=95&enabled=on&enabled=on",
    ] {
        let response = app(&state)
            .oneshot(compaction_request(&token, body))
            .await
            .expect("compaction");
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body}"
        );
        let text = body_text(response).await;
        assert!(text.contains("target=\"compaction-setting\""), "{body}");
        assert!(text.contains(page::COMPACTION_MALFORMED), "{body}");
        assert_eq!(
            state.preferences.compaction(),
            CompactionPreference::default(),
            "{body}"
        );
    }
}

#[tokio::test]
async fn reset_rejects_absent_duplicated_and_malformed_confirmation() {
    let state = owned_state();
    let token = connected(&state);
    for (body, copy) in [
        ("", page::CONFIRMATION_ABSENT),
        (
            "confirmation=reset&confirmation=reset",
            page::CONFIRMATION_DUPLICATED,
        ),
        ("confirmation=yes", page::CONFIRMATION_MALFORMED),
        ("other=reset", page::CONFIRMATION_MALFORMED),
    ] {
        let response = app(&state)
            .oneshot(reset_request(&token, body))
            .await
            .expect("confirmation");
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let text = body_text(response).await;
        assert!(text.contains("target=\"local-data-reset\""), "{copy}");
        assert!(text.contains(copy));
        assert!(!state.local_data.is_pending());
    }
}

#[tokio::test]
async fn reset_rejects_native_and_navigation_use_before_recording() {
    let state = owned_state();
    let token = connected(&state);
    for graft in [None, Some("navigation")] {
        let mut request = Request::builder()
            .method(Method::POST)
            .uri("/settings/local-data/reset")
            .header(header::COOKIE, cookie(&token))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some(graft) = graft {
            request = request
                .header(hypergraft::GRAFT_REQUEST, graft)
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE);
        }
        let response = app(&state)
            .oneshot(request.body(Body::from("confirmation=reset")).unwrap())
            .await
            .expect("rejected");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(!state.local_data.is_pending());
    }
}

#[tokio::test]
async fn reset_conflicts_when_a_workflow_owns_the_executor() {
    let state = owned_state();
    let token = connected(&state);
    let _guard = state.workflow_execution.acquire().expect("execution");
    let response = app(&state)
        .oneshot(reset_request(&token, "confirmation=reset"))
        .await
        .expect("busy");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let text = body_text(response).await;
    assert!(text.contains("target=\"local-data-reset\""));
    assert!(text.contains(page::WORKFLOW_BUSY));
    assert!(!state.local_data.is_pending());
}

#[tokio::test]
async fn reset_conflicts_when_an_agent_grant_sits_inside_the_data_root() {
    let state = owned_state();
    let token = connected(&state);
    let nested = git_worktree_under(state.local_data.root(), "nested-grant");
    create_agent(&state, "Inside", &nested);
    let response = app(&state)
        .oneshot(reset_request(&token, "confirmation=reset"))
        .await
        .expect("conflict");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let text = body_text(response).await;
    assert!(text.contains("target=\"local-data-reset\""));
    assert!(text.contains(CatalogueResetConflict::AgentGrant.message()));
    assert_no_paths(&text, &[state.local_data.root(), nested.as_path()]);
    assert!(!state.local_data.is_pending());
    assert_eq!(state.agents.list().len(), 1);
}

#[tokio::test]
async fn a_confirmed_reset_records_the_request_and_keeps_the_theme() {
    let state = owned_state();
    state.preferences.set_theme(Theme::Sector7G).expect("theme");
    let token = connected(&state);
    let response = app(&state)
        .oneshot(reset_request(&token, "confirmation=reset"))
        .await
        .expect("reset");
    assert_eq!(response.status(), StatusCode::OK);
    let text = body_text(response).await;
    assert!(text.contains("target=\"chat-main\""));
    assert!(text.contains("Stop and restart Power Plant to finish the reset."));
    assert!(text.contains("The next start removes local data before normal store initialisation."));
    assert!(state.local_data.is_pending());
    assert_eq!(state.preferences.theme(), Theme::Sector7G);
    assert!(state.workflow_execution.acquire().is_err());

    let document = app(&state)
        .oneshot(
            Request::builder()
                .uri("/settings")
                .header(header::COOKIE, cookie(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("pending document");
    let document_text = body_text(document).await;
    assert!(document_text.contains("Stop and restart Power Plant to finish the reset."));
    assert!(!document_text.contains("id=\"local-data-reset\""));

    let repeated = app(&state)
        .oneshot(reset_request(&token, "confirmation=reset"))
        .await
        .expect("repeated");
    assert_eq!(repeated.status(), StatusCode::OK);
    let repeated_text = body_text(repeated).await;
    assert!(repeated_text.contains("target=\"chat-main\""));
    assert!(repeated_text.contains("Stop and restart Power Plant to finish the reset."));
}
