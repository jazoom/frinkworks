use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode, header},
    middleware::from_fn_with_state,
};
use tower::ServiceExt;

use crate::{
    agents::AgentStore,
    config::{RuntimeConfig, StartupConfig},
    environments::{
        EnvironmentCatalogue, EnvironmentPreparationScheduler, EnvironmentSnapshotRepository,
        PreparationState, SnapshotAvailability,
    },
    preferences::Preferences,
    projects::ProjectStore,
    providers::{ChatBackend, ProviderKind},
    state::AppState,
    vault::ProviderVault,
    workflows::{
        CommitJournals, WorkflowArtefactRepository, WorkflowCatalogue, WorkflowRunStore,
        workspace::WorkflowWorkspaces,
    },
};

const EXAMPLE: &str = "Explain how this project is structured.";
const USEFUL_REPLY: &str = "Hello from Power Plant.";

fn activation_state() -> AppState {
    let scratch = tempfile::tempdir().expect("data");
    let (config, local_data) = crate::local_data::prepare(StartupConfig {
        bind_address: "localhost:4000".to_owned(),
        runtime: RuntimeConfig::development(),
        static_dir: PathBuf::from("/tmp/powerplant-static"),
        data_dir: scratch.path().join("data"),
        protected_user_roots: Vec::new(),
    })
    .expect("owned root");
    let root = local_data.root();
    let environments = Arc::new(
        EnvironmentCatalogue::open(
            root.join("environments.json"),
            root.join("environment-preparation-logs"),
        )
        .expect("environments"),
    );
    let snapshots = Arc::new(
        EnvironmentSnapshotRepository::open(root.join("environment-snapshots")).expect("snapshots"),
    );
    let alpine = crate::workflows::alpine_git_id(&environments).expect("alpine-git");
    let workflows = WorkflowCatalogue::open_with_seeds(
        root.join("workflows.json"),
        &crate::workflows::seeds::production_seeds(alpine),
    )
    .expect("workflows");
    let mut state = crate::tests::test_state(config.runtime);
    state.chat = Arc::new(ChatBackend::Scripted(
        crate::tests::ScriptedBackend::chunks([Ok(USEFUL_REPLY.to_owned())]),
    ));
    state.vault = Arc::new(ProviderVault::open(root.join("providers.json")).expect("providers"));
    state.preferences = Arc::new(Preferences::open(root.join("preferences.json")));
    state.agents = Arc::new(AgentStore::open(root.join("agents")).expect("agents"));
    state.projects = Arc::new(ProjectStore::open(root.join("projects.json")).expect("projects"));
    state.workflows = Arc::new(workflows);
    state.workflow_runs =
        Arc::new(WorkflowRunStore::open(root.join("workflow-runs")).expect("workflow runs"));
    state.workflow_artefacts = Arc::new(
        WorkflowArtefactRepository::open(root.join("workflow-artefacts")).expect("artefacts"),
    );
    state.workflow_workspaces =
        Arc::new(WorkflowWorkspaces::open(root.join("workflow-workspaces")).expect("workspaces"));
    state.commit_journals =
        Arc::new(CommitJournals::open(root.join("workflow-commit-journals")).expect("journals"));
    state.local_data = local_data;
    state.environments = environments.clone();
    state.environment_snapshots = snapshots.clone();
    state.environment_preparations = EnvironmentPreparationScheduler::idle(environments, snapshots);
    state.keep_temp_dir(scratch);
    state
}

fn app(state: &AppState) -> axum::Router {
    crate::slices::router()
        .layer(from_fn_with_state(
            state.clone(),
            crate::sessions::resolve_session,
        ))
        .layer(from_fn_with_state(
            state.clone(),
            crate::security::enforce_origin,
        ))
        .layer(axum::middleware::from_fn(hypergraft::middleware::classify))
        .with_state(state.clone())
}

fn git_worktree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("worktree");
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .expect("git")
            .success()
    );
    dir
}

fn cookie(token: &str) -> String {
    format!("powerplant_session={token}")
}

fn session_cookie(headers: &HeaderMap) -> String {
    let header = headers
        .get(header::SET_COOKIE)
        .expect("session cookie")
        .to_str()
        .expect("cookie utf8");
    let start =
        header.find("powerplant_session=").expect("session name") + "powerplant_session=".len();
    let rest = &header[start..];
    rest[..rest.find(';').unwrap_or(rest.len())].to_owned()
}

fn navigate_target(text: &str) -> String {
    let marker = "navigate=\"";
    let start = text.find(marker).expect("navigate") + marker.len();
    let end = text[start..].find('"').expect("navigate end") + start;
    text[start..end].to_owned()
}

fn location(headers: &HeaderMap) -> String {
    headers
        .get(header::LOCATION)
        .expect("location")
        .to_str()
        .expect("location utf8")
        .to_owned()
}

fn form_value(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            b' ' => encoded.push('+'),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

async fn send(state: &AppState, request: Request<Body>) -> (StatusCode, HeaderMap, String) {
    let response = app(state).oneshot(request).await.expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, headers, String::from_utf8(body.to_vec()).unwrap())
}

fn document(uri: &str, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::COOKIE, cookie(token));
    }
    builder.body(Body::empty()).unwrap()
}

fn patch(uri: &str, token: Option<&str>, body: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::ORIGIN, "http://localhost:4000")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(hypergraft::GRAFT_REQUEST, "patch")
        .header(header::ACCEPT, hypergraft::MEDIA_TYPE);
    if let Some(token) = token {
        builder = builder.header(header::COOKIE, cookie(token));
    }
    builder.body(Body::from(body.to_owned())).unwrap()
}

fn ready_alpine_git(state: &AppState) {
    let preparation = state
        .environments
        .claim_oldest_queued()
        .expect("claim")
        .expect("queued");
    assert_eq!(preparation.state, PreparationState::Preparing);
    let snapshot = crate::tests::sample_snapshot(preparation.id);
    state.environment_snapshots.mark(
        snapshot.artifact_key.clone(),
        SnapshotAvailability::Available,
    );
    state
        .environments
        .finish_ready(&preparation.id, snapshot, preparation.log)
        .expect("ready");
}

#[tokio::test]
async fn tool_free_activation_reaches_useful_chat_without_a_runtime() {
    let state = activation_state();
    assert!(!state.vault.has_providers());
    assert!(state.projects.list().is_empty());
    assert!(state.agents.list().is_empty());
    assert!(!state.workflows.list().is_empty());
    let alpine = crate::workflows::alpine_git_id(&state.environments).expect("alpine-git");
    assert!(
        state
            .environments
            .get(&alpine)
            .expect("environment")
            .ready_preparation
            .is_none()
    );
    let (status, headers, _) = send(&state, document("/", None)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location(&headers), "/connect");

    let (status, _, text) = send(&state, document("/connect", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(text.contains("href=\"/connect?provider=xai\""));
    let (status, _, text) = send(&state, document("/connect?provider=xai", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(text.contains("action=\"/connect\""));
    assert!(text.contains("Connect to xAI (Grok)"));

    let (status, headers, text) = send(
        &state,
        patch("/connect", None, "provider=xai&api_key=sk-test-key"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(text.contains(r#"navigate="/conversations""#));
    assert!(!text.contains("sk-test-key"));
    assert!(state.vault.contains(ProviderKind::Xai));
    assert!(headers.get(header::SET_COOKIE).is_none());

    let (status, headers, _) = send(&state, document("/", None)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location(&headers), "/conversations");
    let token = session_cookie(&headers);

    let (status, _, text) = send(&state, document("/projects/new", Some(&token))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(text.contains("Choose folder"));
    assert!(text.contains("Git project folder"));
    assert!(text.contains("formaction=\"/projects/folder\""));
    assert!(text.contains("action=\"/projects\""));

    let worktree = git_worktree();
    let path = worktree.path().canonicalize().expect("canonical");
    state.keep_temp_dir(worktree);
    let create_body = format!("name=Desk&path={}", form_value(&path.to_string_lossy()));
    let (status, _, text) = send(&state, patch("/projects", Some(&token), &create_body)).await;
    assert_eq!(status, StatusCode::OK);
    let project = &state.projects.list()[0];
    assert_eq!(project.name, "Desk");
    assert_eq!(project.host_path, path);
    assert_eq!(
        navigate_target(&text),
        format!("/projects/{}", project.id.as_hex())
    );

    let project_path = format!("/projects/{}", project.id.as_hex());
    let (status, _, text) = send(&state, document(&project_path, Some(&token))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(text.contains("New conversation"));
    assert!(text.contains(&format!(
        "href=\"/conversations/new?project={}\"",
        project.id.as_hex()
    )));

    let (status, _, text) = send(
        &state,
        patch(
            "/conversations",
            Some(&token),
            &format!("project={}", project.id),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        navigate_target(&text),
        format!("/conversations/new?project={}", project.id)
    );
    assert!(state.conversations.list().is_empty());
    let (status, _, _) = send(
        &state,
        patch(
            "/conversations/new",
            Some(&token),
            &format!(
                "action=send&project={}&provider=xai&model=grok-4.6&thinking={}&message=Hello",
                project.id,
                state
                    .models_dev
                    .effective_effort(ProviderKind::Xai, "grok-4.6", None)
                    .unwrap()
                    .as_str()
            ),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while state
            .conversations
            .list()
            .iter()
            .any(|record| record.active_job.is_some())
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let conversation = state
        .conversations
        .list()
        .pop()
        .expect("saved conversation");
    let conversation_path = format!("/conversations/{}", conversation.id);
    assert!(state.agents.list().is_empty());
    assert!(conversation.grants.is_empty());
    ready_alpine_git(&state);
    let (status, _, _) = send(
        &state,
        patch(
            &format!("{conversation_path}/access"),
            Some(&token),
            &format!(
                "revision={}&project={}&access=read-write",
                conversation.revision, project.id
            ),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let conversation = state
        .conversations
        .get(&conversation.id)
        .expect("conversation");
    let (status, _, _) = send(
        &state,
        patch(
            &format!("{conversation_path}/messages"),
            Some(&token),
            &format!(
                "revision={}&message={}",
                conversation.revision,
                form_value(EXAMPLE)
            ),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while state
            .conversations
            .get(&conversation.id)
            .expect("conversation")
            .active_job
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("settlement");
    let (status, _, text) = send(&state, document(&conversation_path, Some(&token))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(text.contains(USEFUL_REPLY));
    assert!(text.contains(&format!("href=\"{conversation_path}/workflow\"")));
    assert!(state.workflow_runs.summaries().is_empty());
}
