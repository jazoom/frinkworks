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
    providers::{ChatBackend, ProviderKind},
    state::AppState,
    vault::ProviderVault,
    workflows::{
        CommitJournals, WorkflowArtefactRepository, WorkflowCatalogue, WorkflowRunStore,
        workspace::WorkflowWorkspaces,
    },
};

pub(super) fn file_outcome_run(
    conversation: crate::conversations::ConversationId,
    outcomes: &[crate::workflows::apply::ApplyRootOutcome],
) -> crate::workflows::WorkflowRun {
    use crate::workflows::{
        AttemptId, RunId, RunKind, WorkflowRun,
        apply::{ApplyRoot, ApplyTransaction, ApplyTransactionState},
        artefacts::{ArtefactHash, ArtefactReference, CandidateHash},
        definition::{ArtefactKind, PinnedWorkflowDefinition},
        run::{AttemptCleanupRecord, AttemptSandboxKind, AttemptSandboxRecord},
    };
    let definition = crate::tests::test_named_definition("Recorded recovery");
    let environments = crate::tests::test_environment_set(&definition);
    let mut run = WorkflowRun::create(
        RunId::generate().unwrap(),
        1,
        None,
        RunKind::Configured,
        PinnedWorkflowDefinition::pin(None, definition),
        environments,
    );
    run.conversation_id = Some(conversation);
    let attempt = AttemptId::generate().unwrap();
    run.start_attempt(
        attempt,
        Vec::new(),
        crate::tests::test_agent_capabilities(),
        AttemptSandboxRecord {
            kind: AttemptSandboxKind::IsolatedAttempt,
            snapshot_digest: run.environments.steps[0].snapshot_digest.clone(),
        },
        2,
    )
    .unwrap();
    let reference = |kind, bytes: &[u8]| ArtefactReference {
        id: crate::workflows::ArtefactId::generate().unwrap(),
        kind,
        artefact_hash: ArtefactHash::of(b"fixture", bytes),
    };
    run.record_apply_transaction(
        attempt,
        ApplyTransaction {
            state: ApplyTransactionState::Recovered,
            roots: outcomes
                .iter()
                .enumerate()
                .map(|(index, outcome)| ApplyRoot {
                    grant_id: crate::execution::DirectoryGrantId::generate().unwrap(),
                    alias: format!("directory-{index}"),
                    host_path: format!("/synthetic/directory-{index}").into(),
                    identity: crate::execution::CanonicalDirectoryIdentity {
                        device: 1,
                        inode: index as u64 + 1,
                    },
                    baseline_candidate: CandidateHash::of(b"baseline"),
                    candidate_hash: CandidateHash::of(b"candidate"),
                    exclusions: Vec::new(),
                    outcome: *outcome,
                })
                .collect(),
            baseline: reference(ArtefactKind::CandidateRevision, b"baseline"),
            candidate: reference(ArtefactKind::CandidateRevision, b"candidate"),
            approval: reference(ArtefactKind::HumanDecision, b"approval"),
        },
    )
    .unwrap();
    run.record_cleanup(attempt, AttemptCleanupRecord::Complete)
        .unwrap();
    run
}

const EXAMPLE: &str = "Explain how this project is structured.";
const USEFUL_REPLY: &str = "Hello from Frinkworks.";

fn activation_state() -> AppState {
    let scratch = tempfile::tempdir().expect("data");
    let (config, local_data) = crate::local_data::prepare(StartupConfig {
        bind_address: "localhost:4000".to_owned(),
        runtime: RuntimeConfig::development(),
        static_dir: PathBuf::from("/tmp/frinkworks-static"),
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

fn cookie(token: &str) -> String {
    format!("frinkworks_session={token}")
}

fn session_cookie(headers: &HeaderMap) -> String {
    let header = headers
        .get(header::SET_COOKIE)
        .expect("session cookie")
        .to_str()
        .expect("cookie utf8");
    let start =
        header.find("frinkworks_session=").expect("session name") + "frinkworks_session=".len();
    let rest = &header[start..];
    rest[..rest.find(';').unwrap_or(rest.len())].to_owned()
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

    let (status, _, _) = send(&state, document("/conversations/new", Some(&token))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(state.conversations.list().is_empty());
    let (status, _, _) = send(
        &state,
        patch(
            "/conversations/new",
            Some(&token),
            &format!(
                "action=send&provider=xai&model=grok-4.6&thinking={}&message=Hello",
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
    ready_alpine_git(&state);
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

#[tokio::test]
async fn recovery_settlement_revalidates_identity_and_never_reapplies_files() {
    use crate::workflows::{apply::ApplyRootOutcome, run::AttemptCleanupRecord};
    let state = crate::tests::test_state(RuntimeConfig::development());
    state
        .vault
        .put(crate::providers::ProviderConnection::with_key(
            ProviderKind::Xai,
            "test-key",
            "grok-4.6",
        ))
        .unwrap();
    let token = crate::sessions::generate_session_token().unwrap();
    state.sessions.insert(token.id());
    let token = token.raw().as_str();
    let owner = state.conversations.create("Recovery".to_owned()).unwrap();
    let other = state.conversations.create("Other".to_owned()).unwrap();
    let baseline = tempfile::tempdir().unwrap();
    let file = baseline.path().join("retained.txt");
    std::fs::write(&file, "Do not change").unwrap();

    for case in ["partial", "unchanged", "uncertain", "cleanup", "terminal"] {
        let outcome = match case {
            "unchanged" => ApplyRootOutcome::Unchanged,
            "uncertain" => ApplyRootOutcome::Uncertain,
            _ => ApplyRootOutcome::Applied,
        };
        let mut run = file_outcome_run(owner.id, &[outcome, ApplyRootOutcome::Conflicted]);
        let attempt = run.attempts[0].id;
        run.attempts[0].apply_transaction.as_mut().unwrap().roots[0].host_path =
            baseline.path().to_owned();
        if case == "cleanup" {
            run.attempts[0].cleanup = AttemptCleanupRecord::Pending;
        } else if case == "terminal" {
            run.settle_known_partial(3).unwrap();
        }
        state.workflow_runs.create(run.clone()).unwrap();
        let path = format!("/conversations/{}/runs/{}/settle-partial", owner.id, run.id);
        let valid = format!("attempt={attempt}&state=recovered");
        for (uri, form) in [
            (
                path.clone(),
                format!(
                    "attempt={}&state=recovered",
                    crate::workflows::AttemptId::generate().unwrap()
                ),
            ),
            (path.clone(), format!("attempt={attempt}&state=verified")),
            (
                format!("/conversations/{}/runs/{}/settle-partial", other.id, run.id),
                valid.clone(),
            ),
        ] {
            let (status, _, body) = send(&state, patch(&uri, Some(token), &form)).await;
            assert_eq!(status, StatusCode::CONFLICT);
            assert!(body.contains("target=\"conversation-detail\""));
            assert_eq!(state.workflow_runs.get(&run.id).unwrap().state, run.state);
        }
        let (status, _, body) = send(&state, patch(&path, Some(token), &valid)).await;
        let allowed = matches!(case, "partial" | "unchanged");
        assert_eq!(
            status,
            if allowed {
                StatusCode::OK
            } else {
                StatusCode::CONFLICT
            }
        );
        assert!(body.contains("target=\"conversation-detail\""));
        assert!(!body.contains("navigate="));
        let stored = state.workflow_runs.get(&run.id).unwrap();
        assert_eq!(
            stored.attempts[0].apply_transaction,
            run.attempts[0].apply_transaction
        );
        assert_eq!(
            stored.state,
            if allowed {
                crate::workflows::run::RunState::Cancelled
            } else {
                run.state
            }
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "Do not change");
    }
}

#[tokio::test]
async fn recovery_evidence_preserves_attempt_identity_and_escapes_directory_text() {
    use crate::workflows::{
        CommitTransactionState,
        apply::ApplyRootOutcome,
        tests::{CommitRoot, CommitTransaction},
    };
    let state = crate::tests::test_state(RuntimeConfig::development());
    state
        .vault
        .put(crate::providers::ProviderConnection::with_key(
            ProviderKind::Xai,
            "test-key",
            "grok-4.6",
        ))
        .unwrap();
    let token = crate::sessions::generate_session_token().unwrap();
    state.sessions.insert(token.id());
    let token = token.raw().as_str();
    let owner = state
        .conversations
        .create("Recovery evidence".to_owned())
        .unwrap();
    let mut run = file_outcome_run(owner.id, &[ApplyRootOutcome::Uncertain]);
    let attempt = run.attempts[0].id;
    let transaction = run.attempts[0].apply_transaction.as_mut().unwrap();
    transaction.roots[0].alias = "<script>hostile</script>".to_owned();
    transaction.roots[0].host_path = "/synthetic/<img src=x onerror=alert(1)>".into();
    let root = &transaction.roots[0];
    run.attempts[0].commit_transaction = Some(CommitTransaction {
        candidate: transaction.candidate.clone(),
        reviews: Vec::new(),
        approval: None,
        roots: [
            CommitTransactionState::Verified {
                commit: "recorded-commit".to_owned(),
            },
            CommitTransactionState::ReferenceUpdated {
                commit: "unverified-commit".to_owned(),
            },
        ]
        .into_iter()
        .map(|state| CommitRoot {
            grant: crate::execution::DirectoryGrant {
                id: root.grant_id,
                host_path: root.host_path.clone(),
                identity: root.identity,
                alias: root.alias.clone(),
                access: crate::execution::DirectoryAccess::ReviewBeforeApply,
            },
            state,
            expected_reference: "refs/heads/main".to_owned(),
            old_object: None,
            target_tree: None,
            expected_commit: None,
            timestamp: String::new(),
        })
        .collect(),
    });
    run.attempts[0].state = crate::workflows::run::AttemptState::Failed;
    run.state = crate::workflows::run::RunState::Failed;
    state.workflow_runs.create(run.clone()).unwrap();
    let evidence = format!("/runs/{}/attempts/{attempt}/changes", run.id);
    for path in [
        format!("/conversations/{}", owner.id),
        format!("/conversations/{}/activity", owner.id),
        evidence.clone(),
    ] {
        let (status, _, body) = send(&state, document(&path, Some(token))).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Directory results"));
        assert!(body.contains("Repository results"));
        assert!(body.contains("recorded-commit"));
        assert!(!body.contains("unverified-commit"));
        assert!(!body.contains("<script>hostile"));
        assert!(!body.contains("<img src=x"));
        assert!(body.contains("hostile"));
        assert!(!body.contains("/settle-partial"));
        assert!(!body.contains("data-continue-conversation"));
        if path != evidence {
            assert!(body.contains(&format!("href=\"{evidence}\"")));
        }
    }
    assert_eq!(state.workflow_runs.get(&run.id).unwrap(), run);
}
