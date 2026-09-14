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
    local_data::{CatalogueResetConflict, HOST_PATH_RESET_PENDING},
    preferences::Theme,
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

fn git_worktree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("dir");
    git_init(dir.path());
    dir
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

fn encoded_path(path: &Path) -> String {
    path.to_string_lossy().replace(' ', "%20")
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
async fn reset_conflicts_when_a_project_path_sits_inside_the_data_root() {
    let state = owned_state();
    let token = connected(&state);
    let nested = git_worktree_under(state.local_data.root(), "nested-project");
    state
        .projects
        .create("Inside".to_owned(), nested.clone())
        .expect("project");
    let response = app(&state)
        .oneshot(reset_request(&token, "confirmation=reset"))
        .await
        .expect("conflict");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let text = body_text(response).await;
    assert!(text.contains("target=\"local-data-reset\""));
    assert!(text.contains(CatalogueResetConflict::Project.message()));
    assert_no_paths(&text, &[state.local_data.root(), nested.as_path()]);
    assert!(!state.local_data.is_pending());
    assert_eq!(state.projects.list().len(), 1);
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

#[tokio::test]
async fn host_path_mutations_conflict_after_reset_is_pending() {
    let state = owned_state();
    let token = connected(&state);
    let project_dir = git_worktree();
    let agent_dir = git_worktree();
    let extra_dir = git_worktree();
    let project = state
        .projects
        .create("Desk".to_owned(), project_dir.path().to_path_buf())
        .expect("project");
    let agent = create_agent(&state, "Other", agent_dir.path());
    let configured = create_agent(&state, "Configured", extra_dir.path());
    state.keep_temp_dir(project_dir);
    state.keep_temp_dir(agent_dir);
    state.keep_temp_dir(extra_dir);

    let recorded = app(&state)
        .oneshot(reset_request(&token, "confirmation=reset"))
        .await
        .expect("reset");
    assert_eq!(recorded.status(), StatusCode::OK);
    assert!(state.local_data.is_pending());

    let create_dir = git_worktree();
    let create_path = encoded_path(&create_dir.path().canonicalize().expect("canonical"));
    state.keep_temp_dir(create_dir);
    let created = app(&state)
        .oneshot(patch_form(
            &token,
            "/projects",
            format!("name=Desk&path={create_path}"),
        ))
        .await
        .expect("create");
    assert_eq!(created.status(), StatusCode::CONFLICT);
    let created_text = body_text(created).await;
    assert!(created_text.contains("target=\"project-form\""));
    assert!(created_text.contains(HOST_PATH_RESET_PENDING));

    let starter = app(&state)
        .oneshot(patch_form(
            &token,
            &format!("/projects/{}/agents/starter", project.id.as_hex()),
            String::new(),
        ))
        .await
        .expect("starter");
    assert_eq!(starter.status(), StatusCode::CONFLICT);
    let starter_text = body_text(starter).await;
    assert!(starter_text.contains("target=\"chat-main\""));
    assert!(starter_text.contains(HOST_PATH_RESET_PENDING));
    assert_eq!(state.agents.list().len(), 2);

    let grant = app(&state)
        .oneshot(patch_form(
            &token,
            &format!("/projects/{}/agents/grant", project.id.as_hex()),
            format!(
                "agent_id={}&revision={}&alias=code&access=read-write",
                agent.id.as_hex(),
                agent.revision
            ),
        ))
        .await
        .expect("grant");
    assert_eq!(grant.status(), StatusCode::CONFLICT);
    let grant_text = body_text(grant).await;
    assert!(grant_text.contains("target=\"chat-main\""));
    assert!(grant_text.contains(HOST_PATH_RESET_PENDING));
    assert_eq!(
        state
            .agents
            .get(&agent.id)
            .expect("agent")
            .directories
            .len(),
        1
    );

    let agent_dir = git_worktree();
    let agent_path = encoded_path(&agent_dir.path().canonicalize().expect("canonical"));
    state.keep_temp_dir(agent_dir);
    let agent_create = app(&state)
        .oneshot(patch_form(
            &token,
            "/agents",
            format!(
                "intent=save&name=Reader&instructions=&primary=project&tool_list=on&alias_0=project&path_0={agent_path}&access_0=read-write"
            ),
        ))
        .await
        .expect("agent create");
    assert_eq!(agent_create.status(), StatusCode::CONFLICT);
    let agent_create_text = body_text(agent_create).await;
    assert!(agent_create_text.contains("target=\"agent-form\""));
    assert!(agent_create_text.contains(HOST_PATH_RESET_PENDING));
    assert_eq!(state.agents.list().len(), 2);

    let updated = app(&state)
        .oneshot(patch_form(
            &token,
            &format!("/agents/{}/configuration", configured.id.as_hex()),
            format!(
                "intent=save&name=After&instructions=&primary=project&tool_list=on&alias_0=project&path_0={}&access_0=read-write&revision={}",
                encoded_path(&configured.directories[0].host_path),
                configured.revision
            ),
        ))
        .await
        .expect("agent update");
    assert_eq!(updated.status(), StatusCode::CONFLICT);
    let updated_text = body_text(updated).await;
    assert!(updated_text.contains("target=\"agent-form\""));
    assert!(updated_text.contains(HOST_PATH_RESET_PENDING));
    assert_eq!(
        state.agents.get(&configured.id).expect("configured").name,
        "Configured"
    );

    let deleted = app(&state)
        .oneshot(patch_form(
            &token,
            &format!("/agents/{}/delete", configured.id.as_hex()),
            format!("revision={}", configured.revision),
        ))
        .await
        .expect("agent delete");
    assert_eq!(deleted.status(), StatusCode::CONFLICT);
    let deleted_text = body_text(deleted).await;
    assert!(deleted_text.contains("target=\"agent-form\""));
    assert!(deleted_text.contains(HOST_PATH_RESET_PENDING));
    assert!(state.agents.get(&configured.id).is_some());
}

#[tokio::test]
async fn reset_and_a_project_under_the_data_root_cannot_both_succeed() {
    let state = owned_state();
    let token = connected(&state);
    let nested = git_worktree_under(state.local_data.root(), "race-project");
    let encoded = encoded_path(&nested);
    let router = app(&state);
    let (reset, create) = tokio::join!(
        router
            .clone()
            .oneshot(reset_request(&token, "confirmation=reset")),
        router.oneshot(patch_form(
            &token,
            "/projects",
            format!("name=Nested&path={encoded}"),
        )),
    );
    let reset = reset.expect("reset");
    let create = create.expect("create");
    let reset_text = body_text(reset).await;
    let create_text = body_text(create).await;
    let reset_recorded = reset_text.contains("Stop and restart Power Plant to finish the reset.");
    let create_recorded = create_text.contains("navigate=\"/projects/");
    assert!(
        reset_recorded ^ create_recorded,
        "reset={reset_text}\ncreate={create_text}"
    );
    if reset_recorded {
        assert!(state.local_data.is_pending());
        assert!(state.projects.list().is_empty());
        assert!(create_text.contains(HOST_PATH_RESET_PENDING));
    } else {
        assert!(!state.local_data.is_pending());
        assert_eq!(state.projects.list().len(), 1);
        assert!(reset_text.contains(CatalogueResetConflict::Project.message()));
        assert_no_paths(&reset_text, &[state.local_data.root(), nested.as_path()]);
    }
}
