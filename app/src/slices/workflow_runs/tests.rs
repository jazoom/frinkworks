use askama::Template;
use axum::{
    body::{Body, to_bytes},
    http::{Request, header},
    middleware::from_fn_with_state,
};
use tower::ServiceExt;

use crate::{
    config::RuntimeConfig,
    providers::{ProviderConnection, ProviderKind},
    sessions,
    state::AppState,
    workflows::{
        RunId, WorkflowRun, definition::PinnedWorkflowDefinition, seeds::one_agent_definition,
    },
};

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

fn cookie(token: &str) -> String {
    format!("powerplant_session={token}")
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

fn stored_run(state: &AppState) -> RunId {
    let definition = one_agent_definition(crate::tests::test_environment_id());
    let environments = crate::tests::test_environment_set(&definition);
    let run = WorkflowRun::create(
        RunId::generate().expect("run"),
        1,
        Some(crate::agents::AgentId::generate().expect("agent")),
        crate::workflows::RunKind::Configured,
        PinnedWorkflowDefinition::pin(None, definition),
        environments,
    );
    let id = run.id;
    state.workflow_runs.create(run).expect("store");
    id
}

#[tokio::test]
async fn a_runs_document_uses_chat_main() {
    let state = test_state();
    let token = connected(&state);
    stored_run(&state);
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/runs")
                .header(header::COOKIE, cookie(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("index");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("<!doctype html>"));
    assert_eq!(text.matches("id=\"chat-main\"").count(), 1);
    assert!(text.contains("href=\"/runs/"));
    assert!(text.contains("data-graft"));
}

#[tokio::test]
async fn a_runs_navigation_patches_chat_main() {
    let state = test_state();
    let token = connected(&state);
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/runs")
                .header(header::COOKIE, cookie(&token))
                .header(hypergraft::GRAFT_REQUEST, "navigation")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("navigation");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(!text.contains("<!doctype html>"));
    assert!(text.contains("operation=\"children\" target=\"chat-main\""));
}

#[tokio::test]
async fn a_runs_patch_is_rejected() {
    let state = test_state();
    let token = connected(&state);
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/runs")
                .header(header::COOKIE, cookie(&token))
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("patch");
    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_detail_document_uses_chat_main() {
    let state = test_state();
    let token = connected(&state);
    let id = stored_run(&state);
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/runs/{}", id.as_hex()))
                .header(header::COOKIE, cookie(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("detail");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("<!doctype html>"));
    assert_eq!(text.matches("id=\"run-detail\"").count(), 1);
    assert!(text.contains("Refresh"));
}

#[tokio::test]
async fn a_detail_navigation_patches_chat_main() {
    let state = test_state();
    let token = connected(&state);
    let id = stored_run(&state);
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/runs/{}", id.as_hex()))
                .header(header::COOKIE, cookie(&token))
                .header(hypergraft::GRAFT_REQUEST, "navigation")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("navigation");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("operation=\"children\" target=\"chat-main\""));
}

#[tokio::test]
async fn a_detail_patch_targets_run_detail() {
    let state = test_state();
    let token = connected(&state);
    let id = stored_run(&state);
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/runs/{}", id.as_hex()))
                .header(header::COOKIE, cookie(&token))
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("patch");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("operation=\"children\" target=\"run-detail\""));
    assert!(!text.contains("id=\"run-detail\""));
}

#[test]
fn review_verdict_skips_candidate_outputs_from_fixing_reviews() {
    let candidate = crate::workflows::artefacts::ArtefactSummary::Candidate {
        candidate: crate::workflows::artefacts::CandidateHash::of(b"candidate"),
        entries: 1,
        bytes: 1,
        disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
    };
    let review = crate::workflows::artefacts::ArtefactSummary::Review {
        candidate: crate::workflows::artefacts::CandidateHash::of(b"candidate"),
        verdict: crate::workflows::artefacts::ReviewVerdict::Approved,
    };

    assert_eq!(
        super::page::review_verdict_label([&candidate, &review].into_iter()),
        "Approved"
    );
}

fn context_packet(prompt: String) -> crate::workflows::input_context::AttemptContextPacket {
    let mut packet = crate::workflows::input_context::AttemptContextPacket {
        prompt,
        messages: vec![crate::workflows::input_context::ContextMessage::User {
            text: "Execute the assigned task.".to_owned(),
        }],
        tools: vec![crate::workflows::input_context::ContextTool {
            name: "read".to_owned(),
            description: "Read a granted file.".to_owned(),
            parameters: serde_json::json!({"type": "object"}),
        }],
        source_available: "Candidate files are available through tools.".to_owned(),
        excluded_context: "Conversation and worker transcripts are excluded.".to_owned(),
        project_instructions: crate::workflows::input_context::ProjectInstructionSnapshot {
            candidate: None,
            guest_path: "AGENTS.md".to_owned(),
            state: crate::workflows::input_context::ProjectInstructionState::Present {
                text: "Use the test command.".to_owned(),
                content_hash: crate::workflows::artefacts::ObjectHash::of(b"Use the test command.")
                    .as_str(),
            },
        },
        budget: crate::workflows::input_context::ContextBudget {
            packet_bytes: 42,
            reserved_output_bytes: 128,
            reserved_tool_bytes: 256,
            total_bytes: 426,
            estimated_input_tokens: 11,
            estimated_total_tokens: 107,
            model_context_limit: None,
        },
    };
    packet.budget.packet_bytes = packet.byte_len() as u64;
    packet.budget.reserved_output_bytes =
        crate::workflows::input_context::RESERVED_MODEL_OUTPUT_BYTES as u64;
    packet.budget.reserved_tool_bytes =
        crate::workflows::input_context::RESERVED_TOOL_WORK_BYTES as u64;
    packet.budget.total_bytes = packet.budget.packet_bytes
        + packet.budget.reserved_output_bytes
        + packet.budget.reserved_tool_bytes;
    packet.budget.estimated_input_tokens = packet.budget.packet_bytes;
    packet.budget.estimated_total_tokens = packet.budget.total_bytes;
    packet
}

#[test]
fn context_inspection_preserves_text_across_bounded_escaped_pages() {
    let packet = context_packet("<script>é&".repeat(8000));
    let mut collected = String::new();
    loop {
        let context = super::page::initial_context_view(
            &packet,
            "/runs/run",
            "/runs/run/attempts/attempt/context",
            0,
            collected.len(),
        )
        .expect("context");
        let rendered = context.render().expect("render");
        assert!(!rendered.contains("<script>"));
        hypergraft::outcome::page_patch("Initial context", "chat-main", &context)
            .expect("bounded navigation envelope");
        collected.push_str(&context.prompt);
        if collected.len() == packet.prompt.len() {
            assert!(context.next_href.contains("part=1"));
            break;
        }
        assert!(
            context
                .next_href
                .ends_with(&format!("offset={}", collected.len()))
        );
    }
    assert_eq!(collected, packet.prompt);
    let messages =
        super::page::initial_context_view(&packet, "/runs/run", "/context", 1, 0).expect("message");
    assert_eq!(messages.prompt, packet.request_messages()[0].text);
    let tools =
        super::page::initial_context_view(&packet, "/runs/run", "/context", 2, 0).expect("tools");
    let tools: serde_json::Value = serde_json::from_str(&tools.prompt).expect("tool JSON");
    assert_eq!(tools[0]["parameters"], packet.request_tools()[0].parameters);
    assert!(
        super::page::initial_context_view(&packet, "/runs/run", "/context", usize::MAX, 0)
            .is_none()
    );
    assert!(
        super::page::initial_context_view(&packet, "/runs/run", "/context", 0, usize::MAX)
            .is_none()
    );
}

#[tokio::test]
async fn context_routes_reject_cross_run_attempts_and_unsupported_patches() {
    let state = test_state();
    let token = connected(&state);
    let run_id = stored_run(&state);
    let other_run = stored_run(&state);
    let attempt_id = crate::workflows::AttemptId::generate().expect("attempt");
    state
        .workflow_runs
        .mutate(&run_id, |run| {
            let sandbox = crate::workflows::run::AttemptSandboxRecord {
                kind: crate::workflows::run::AttemptSandboxKind::IsolatedAttempt,
                snapshot_digest: run.environments.steps[0].snapshot_digest.clone(),
            };
            run.start_attempt(
                attempt_id,
                vec![],
                crate::tests::test_agent_capabilities(),
                sandbox,
                2,
            )?;
            run.record_initial_context(
                attempt_id,
                context_packet("Private attempt direction".to_owned()),
            )
        })
        .expect("attempt");
    for (owner, representation, status) in [
        (run_id, None, 200),
        (run_id, Some("navigation"), 200),
        (run_id, Some("patch"), 400),
        (other_run, None, 303),
    ] {
        let mut request = Request::builder()
            .uri(format!(
                "/runs/{}/attempts/{}/context",
                owner.as_hex(),
                attempt_id.as_hex()
            ))
            .header(header::COOKIE, cookie(&token));
        if let Some(representation) = representation {
            request = request
                .header(hypergraft::GRAFT_REQUEST, representation)
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE);
        }
        let response = app(&state)
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .expect("context response");
        assert_eq!(response.status().as_u16(), status);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(text.contains("Private attempt direction"), status == 200);
    }
}

#[tokio::test]
async fn evidence_routes_reject_cross_run_attempts_and_unsupported_patches() {
    let state = test_state();
    let token = connected(&state);
    let run_id = stored_run(&state);
    let other_run = stored_run(&state);
    let attempt_id = crate::workflows::AttemptId::generate().expect("attempt");
    state
        .workflow_runs
        .mutate(&run_id, |run| {
            let sandbox = crate::workflows::run::AttemptSandboxRecord {
                kind: crate::workflows::run::AttemptSandboxKind::IsolatedAttempt,
                snapshot_digest: run.environments.steps[0].snapshot_digest.clone(),
            };
            run.start_attempt(
                attempt_id,
                vec![],
                crate::tests::test_agent_capabilities(),
                sandbox,
                2,
            )
        })
        .expect("attempt");
    let phase = state.workflow_runs.get(&run_id).unwrap().attempts[0]
        .step
        .as_str()
        .to_owned();
    let evidence = crate::workflows::AttemptEvidenceContext::new(
        state.workflow_evidence.clone(),
        run_id,
        attempt_id,
        phase,
    );
    for _ in 0..crate::workflows::evidence::MAXIMUM_ACTIVITY_EVENTS {
        evidence.response(&"<&'\"".repeat(64), None);
    }
    let mut reply = crate::providers::AssistantReply::from("<&'\"".repeat(16 * 1024));
    reply.thinking = reply.text.clone();
    reply.tools = vec![
        crate::providers::ToolOutput {
            label: reply.text.clone(),
            output: reply.text.clone(),
            command: None,
        };
        8
    ];
    evidence.terminal(
        crate::workflows::evidence::TerminalState::Completed,
        &reply,
        Some(&reply.text),
        None,
    );
    for view in ["activity", "changes", "result"] {
        for (owner, representation, status) in [
            (run_id, None, 200),
            (run_id, Some("navigation"), 200),
            (run_id, Some("patch"), 400),
            (other_run, None, 303),
        ] {
            let mut request = Request::builder()
                .uri(format!(
                    "/runs/{}/attempts/{}/{view}",
                    owner.as_hex(),
                    attempt_id.as_hex()
                ))
                .header(header::COOKIE, cookie(&token));
            if let Some(representation) = representation {
                request = request
                    .header(hypergraft::GRAFT_REQUEST, representation)
                    .header(header::ACCEPT, hypergraft::MEDIA_TYPE);
            }
            let response = app(&state)
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .expect("evidence response");
            assert_eq!(response.status().as_u16(), status, "{view}");
            if status == 303 {
                assert_eq!(
                    response.headers()[header::LOCATION],
                    format!("/runs/{}", other_run.as_hex())
                );
            }
        }
    }
}

#[tokio::test]
async fn an_unknown_run_redirects_to_the_index() {
    let state = test_state();
    let token = connected(&state);
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/runs/{}", "a".repeat(32)))
                .header(header::COOKIE, cookie(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("missing");
    assert_eq!(response.status(), axum::http::StatusCode::SEE_OTHER);
    assert_eq!(response.headers().get(header::LOCATION).unwrap(), "/runs");
}

#[tokio::test]
async fn an_unknown_artefact_redirects_to_the_run() {
    let state = test_state();
    let token = connected(&state);
    let id = stored_run(&state);
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/runs/{}/artefacts/{}",
                    id.as_hex(),
                    "a".repeat(32)
                ))
                .header(header::COOKIE, cookie(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("missing artefact");
    assert_eq!(response.status(), axum::http::StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get(header::LOCATION).unwrap(),
        format!("/runs/{}", id.as_hex()).as_str()
    );
}

#[tokio::test]
async fn anonymous_run_requests_redirect_to_connect() {
    let state = test_state();
    let detail = format!("/runs/{}", "0".repeat(32));
    let cases = [
        ("GET", "/runs", None, false),
        ("GET", "/runs", Some("navigation"), true),
        ("GET", detail.as_str(), Some("patch"), true),
    ];
    for (method, uri, graft, enhanced) in cases {
        assert_connect_redirect(&state, method, uri, graft, enhanced).await;
    }
}

async fn assert_connect_redirect(
    state: &AppState,
    method: &str,
    uri: &str,
    graft: Option<&str>,
    enhanced: bool,
) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(graft) = graft {
        builder = builder
            .header(hypergraft::GRAFT_REQUEST, graft)
            .header(header::ACCEPT, hypergraft::MEDIA_TYPE);
    }
    let response = app(state)
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .expect("anonymous");
    if enhanced {
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            hypergraft::MEDIA_TYPE
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            text.contains("navigate=\"/connect\""),
            "{method} {uri} {graft:?}: {text}"
        );
    } else {
        assert_eq!(response.status(), axum::http::StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "/connect"
        );
    }
}

#[tokio::test]
async fn history_directory_filter_matches_stored_identities_before_the_limit() {
    let state = test_state();
    let token = connected(&state);
    let root = tempfile::tempdir().expect("filter directories");
    let first = root.path().join("first");
    let second = root.path().join("second");
    std::fs::create_dir_all(&first).expect("first");
    std::fs::create_dir_all(&second).expect("second");
    let first_grant =
        crate::execution::DirectoryGrant::from_selected(&first, &[]).expect("first grant");
    let second_grant =
        crate::execution::DirectoryGrant::from_selected(&second, &[]).expect("second grant");
    let first_key = super::page::run_directory_key(&first_grant);
    let older = stored_run(&state);
    let newer = stored_run(&state);
    for (id, grant) in [(older, first_grant.clone()), (newer, second_grant.clone())] {
        state
            .workflow_runs
            .mutate(&id, |run| {
                let step = run
                    .pinned
                    .definition
                    .steps()
                    .iter()
                    .find(|step| {
                        matches!(
                            step.action,
                            crate::workflows::definition::StepAction::Agent(_)
                        )
                    })
                    .expect("model phase")
                    .key
                    .clone();
                let selection = crate::providers::ModelSelection::new(
                    ProviderKind::Xai,
                    "grok-4.6".to_owned(),
                    None,
                )
                .expect("model");
                let mut settings = crate::execution::ExecutionSettings::new(
                    selection.clone(),
                    String::new(),
                    Vec::new(),
                    crate::tests::test_environment_id(),
                )
                .expect("settings");
                settings.directories = vec![grant];
                run.phase_models = vec![crate::workflows::PhaseModelSelection {
                    step,
                    selection,
                    instructions: String::new(),
                    preset: None,
                    settings: Some(settings),
                }];
                Ok(())
            })
            .expect("pin directories");
    }
    let view = super::page::RunIndexView::filtered(&state, &first_key, "");
    assert_eq!(view.runs.len(), 1);
    assert_eq!(view.runs[0].id, older.as_hex());
    assert_eq!(view.directories.len(), 2);

    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/runs?directory={first_key}"))
                .header(header::COOKIE, cookie(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("filtered document");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains(&older.as_hex()));
    assert!(!text.contains(&newer.as_hex()));
    assert!(text.contains("id=\"run-directory-filter\""));
    assert!(text.contains(&first.to_string_lossy().into_owned()));
    assert!(text.contains("data-graft-submit-on=\"change\""));

    let navigation = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/runs?directory={first_key}"))
                .header(header::COOKIE, cookie(&token))
                .header(hypergraft::GRAFT_REQUEST, "navigation")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("filtered navigation");
    assert_eq!(navigation.status(), axum::http::StatusCode::OK);

    let patch = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/runs?directory={first_key}"))
                .header(header::COOKIE, cookie(&token))
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("filtered patch");
    assert_eq!(patch.status(), axum::http::StatusCode::BAD_REQUEST);

    let unknown = app(&state)
        .oneshot(
            Request::builder()
                .uri("/runs?directory=0000000000000000-0000000000000000")
                .header(header::COOKIE, cookie(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("unknown directory");
    assert_eq!(
        unknown.status(),
        axum::http::StatusCode::UNPROCESSABLE_ENTITY
    );
    let body = to_bytes(unknown.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("Choose a directory from run history"));
    assert!(text.contains("No runs match this filter"));

    let malformed = app(&state)
        .oneshot(
            Request::builder()
                .uri("/runs?project=abc")
                .header(header::COOKIE, cookie(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("malformed filter");
    assert_eq!(malformed.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn history_filter_applies_before_limit_newest_first() {
    let state = test_state();
    let root = tempfile::tempdir().expect("filter directories");
    let first = root.path().join("first");
    let second = root.path().join("second");
    std::fs::create_dir_all(&first).expect("first");
    std::fs::create_dir_all(&second).expect("second");
    let first_grant =
        crate::execution::DirectoryGrant::from_selected(&first, &[]).expect("first grant");
    let second_grant =
        crate::execution::DirectoryGrant::from_selected(&second, &[]).expect("second grant");
    let first_key = super::page::run_directory_key(&first_grant);
    let definition = one_agent_definition(crate::tests::test_environment_id());
    let environments = crate::tests::test_environment_set(&definition);
    let step = definition
        .steps()
        .iter()
        .find(|step| {
            matches!(
                step.action,
                crate::workflows::definition::StepAction::Agent(_)
            )
        })
        .expect("model phase")
        .key
        .clone();
    let mut oldest = None;
    let mut newest = None;
    for index in 0..55 {
        let id = RunId::generate().expect("run id");
        let grant = if index == 0 || index == 54 {
            first_grant.clone()
        } else {
            second_grant.clone()
        };
        let selection =
            crate::providers::ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None)
                .expect("model");
        let mut settings = crate::execution::ExecutionSettings::new(
            selection.clone(),
            String::new(),
            Vec::new(),
            crate::tests::test_environment_id(),
        )
        .expect("settings");
        settings.directories = vec![grant];
        let mut run = WorkflowRun::create(
            id,
            (index + 1) as u64,
            None,
            crate::workflows::RunKind::Configured,
            PinnedWorkflowDefinition::pin(None, definition.clone()),
            environments.clone(),
        );
        run.phase_models = vec![crate::workflows::PhaseModelSelection {
            step: step.clone(),
            selection,
            instructions: String::new(),
            preset: None,
            settings: Some(settings),
        }];
        state.workflow_runs.create(run).expect("store run");
        if index == 0 {
            oldest = Some(id);
        }
        if index == 54 {
            newest = Some(id);
        }
    }
    let (oldest, newest) = (oldest.expect("oldest"), newest.expect("newest"));
    let filtered = super::page::RunIndexView::filtered(&state, &first_key, "");
    assert_eq!(filtered.runs.len(), 2);
    assert_eq!(filtered.runs[0].id, newest.as_hex());
    assert_eq!(filtered.runs[1].id, oldest.as_hex());
    let unfiltered = super::page::RunIndexView::filtered(&state, "", "");
    assert_eq!(unfiltered.runs.len(), 50);
    assert!(
        !unfiltered.runs.iter().any(|row| row.id == oldest.as_hex()),
        "oldest match stays available through the filtered view"
    );
    std::fs::rename(&first, root.path().join("old-first")).expect("rename");
    std::fs::create_dir(&first).expect("recreate");
    let unavailable = super::page::RunIndexView::filtered(&state, &first_key, "");
    assert_eq!(unavailable.runs.len(), 2);
    assert!(
        unavailable
            .directories
            .iter()
            .any(|option| option.id == first_key && option.name.contains("Unavailable")),
        "moved directories keep an unavailable label"
    );
}
