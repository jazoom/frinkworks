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
    workflows::seeds::one_agent_definition,
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

async fn seed_ready_environment(state: &AppState) -> crate::environments::EnvironmentId {
    let (environment, preparation) = state
        .environments
        .create(crate::environments::EnvironmentDraft {
            name: "Alpine Git".to_owned(),
            oci_image: "alpine/git".to_owned(),
            setup_script: String::new(),
        })
        .expect("environment");
    state.environments.claim_oldest_queued().expect("claim");
    let snapshot = crate::tests::sample_snapshot(preparation.id);
    state.environment_snapshots.mark(
        snapshot.artifact_key.clone(),
        crate::environments::SnapshotAvailability::Available,
    );
    state
        .environments
        .finish_ready(&preparation.id, snapshot, preparation.log)
        .expect("ready");
    environment.id
}

fn create_body(environment_id: crate::environments::EnvironmentId) -> String {
    let environment = format!("default-environment={}", environment_id.as_hex());
    [
        "intent=save",
        "name=One+step",
        environment.as_str(),
        "role_0_key=coding-agent",
        "role_0_name=Coding+agent",
        "role_0_expertise=",
        "role_0_prompt=",
        "step_0_key=work-on-task",
        "step_0_name=Work+on+task",
        "step_0_action=agent",
        "step_0_review-policy=none",
        "step_0_role=coding-agent",
        "step_0_candidate-access=edit-candidate",
        "step_0_tool_list=on",
        "step_0_input_0_key=candidate",
        "step_0_input_0_kind=candidate-revision",
        "step_0_input_0_source=run-initial-candidate",
        "step_0_output_0_key=assistant-reply",
        "step_0_output_0_kind=assistant-reply",
        "step_0_output_1_key=candidate",
        "step_0_output_1_kind=candidate-revision",
    ]
    .join("&")
}

fn review_body(
    environment_id: crate::environments::EnvironmentId,
    revision: Option<u64>,
) -> String {
    let environment = format!("default-environment={}", environment_id.as_hex());
    let mut fields = vec![
        "intent=save".to_owned(),
        "name=Review+loop".to_owned(),
        environment,
        "role_0_key=coding-agent".to_owned(),
        "role_0_name=Coding+agent".to_owned(),
        "role_0_expertise=".to_owned(),
        "role_0_prompt=".to_owned(),
        "role_1_key=reviewer".to_owned(),
        "role_1_name=Reviewer".to_owned(),
        "role_1_expertise=".to_owned(),
        "role_1_prompt=".to_owned(),
        "step_0_key=work-on-task".to_owned(),
        "step_0_name=Work+on+task".to_owned(),
        "step_0_action=agent".to_owned(),
        "step_0_review-policy=none".to_owned(),
        "step_0_role=coding-agent".to_owned(),
        "step_0_candidate-access=edit-candidate".to_owned(),
        "step_0_tool_list=on".to_owned(),
        "step_0_input_0_key=candidate".to_owned(),
        "step_0_input_0_kind=candidate-revision".to_owned(),
        "step_0_input_0_source=run-current-candidate".to_owned(),
        "step_0_output_0_key=assistant-reply".to_owned(),
        "step_0_output_0_kind=assistant-reply".to_owned(),
        "step_0_output_1_key=candidate".to_owned(),
        "step_0_output_1_kind=candidate-revision".to_owned(),
        "step_1_key=review".to_owned(),
        "step_1_name=Review".to_owned(),
        "step_1_action=agent".to_owned(),
        "step_1_review-policy=review-verdict".to_owned(),
        "step_1_report-output=review".to_owned(),
        "step_1_revision-target=work-on-task".to_owned(),
        "step_1_attempt-limit=3".to_owned(),
        "step_1_role=reviewer".to_owned(),
        "step_1_candidate-access=read-only".to_owned(),
        "step_1_tool_list=on".to_owned(),
        "step_1_input_0_key=candidate".to_owned(),
        "step_1_input_0_kind=candidate-revision".to_owned(),
        "step_1_input_0_source=run-current-candidate".to_owned(),
        "step_1_output_0_key=assistant-reply".to_owned(),
        "step_1_output_0_kind=assistant-reply".to_owned(),
        "step_1_output_1_key=review".to_owned(),
        "step_1_output_1_kind=review-report".to_owned(),
    ];
    if let Some(revision) = revision {
        fields.push(format!("revision={revision}"));
    }
    fields.join("&")
}

#[tokio::test]
async fn a_catalogue_document_uses_chat_main() {
    let state = test_state();
    let token = connected(&state);
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/workflows")
                .header(header::COOKIE, cookie(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("catalogue");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("<!doctype html>"));
    assert_eq!(text.matches("id=\"chat-main\"").count(), 1);
    assert!(text.contains("href=\"/workflows/new\""));
    assert!(text.contains("data-graft"));
}

#[tokio::test]
async fn a_catalogue_navigation_patches_chat_main() {
    let state = test_state();
    let token = connected(&state);
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/workflows")
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
async fn a_catalogue_patch_is_rejected() {
    let state = test_state();
    let token = connected(&state);
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/workflows")
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
async fn create_redirects_to_configuration() {
    let state = test_state();
    let token = connected(&state);
    let environment = seed_ready_environment(&state).await;
    let response = app(&state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/workflows")
                .header(header::COOKIE, cookie(&token))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from(create_body(environment)))
                .unwrap(),
        )
        .await
        .expect("create");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("navigate=\"/workflows/"));
    assert!(text.contains("/configuration\""));
    assert_eq!(state.workflows.list()[0].definition.name(), "One step");
}

#[tokio::test]
async fn create_rejects_a_malformed_review_policy() {
    let state = test_state();
    let token = connected(&state);
    let environment = seed_ready_environment(&state).await;
    let body = create_body(environment)
        .replace("step_0_review-policy=none", "step_0_review-policy=normal");
    let response = app(&state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/workflows")
                .header(header::COOKIE, cookie(&token))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("malformed policy");
    assert_eq!(
        response.status(),
        axum::http::StatusCode::UNPROCESSABLE_ENTITY
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("target=\"workflow-form\""));
    assert!(text.contains("That review policy is not valid."));
    assert!(state.workflows.list().is_empty());
}

#[tokio::test]
async fn create_rejects_an_unready_environment() {
    let state = test_state();
    let token = connected(&state);
    let response = app(&state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/workflows")
                .header(header::COOKIE, cookie(&token))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from(create_body(crate::tests::test_environment_id())))
                .unwrap(),
        )
        .await
        .expect("unready");
    assert_eq!(
        response.status(),
        axum::http::StatusCode::UNPROCESSABLE_ENTITY
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("target=\"workflow-form\""));
    assert!(state.workflows.list().is_empty());
}

#[tokio::test]
async fn create_validation_returns_unprocessable() {
    let state = test_state();
    let token = connected(&state);
    let response = app(&state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/workflows")
                .header(header::COOKIE, cookie(&token))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from("intent=save&name="))
                .unwrap(),
        )
        .await
        .expect("invalid");
    assert_eq!(
        response.status(),
        axum::http::StatusCode::UNPROCESSABLE_ENTITY
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("target=\"workflow-form\""));
}

#[tokio::test]
async fn stale_updates_return_conflict() {
    let state = test_state();
    let token = connected(&state);
    let environment = seed_ready_environment(&state).await;
    let record = state
        .workflows
        .create(one_agent_definition(environment))
        .expect("create");
    state
        .workflows
        .update(
            &record.id,
            record.revision,
            crate::workflows::definition::WorkflowDefinition::from_parts(
                "Edited".to_owned(),
                environment,
                record.definition.roles().to_vec(),
                record.definition.steps().to_vec(),
            )
            .expect("edited"),
        )
        .expect("update");
    let response = app(&state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/workflows/{}/configuration", record.id.as_hex()))
                .header(header::COOKIE, cookie(&token))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from(format!(
                    "{}&revision={}",
                    create_body(environment),
                    record.revision
                )))
                .unwrap(),
        )
        .await
        .expect("stale");
    assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
}

#[tokio::test]
async fn edit_saves_a_review_policy() {
    let state = test_state();
    let token = connected(&state);
    let environment = seed_ready_environment(&state).await;
    let record = state
        .workflows
        .create(one_agent_definition(environment))
        .expect("create");
    let response = app(&state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/workflows/{}/configuration", record.id.as_hex()))
                .header(header::COOKIE, cookie(&token))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from(review_body(environment, Some(record.revision))))
                .unwrap(),
        )
        .await
        .expect("edit");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("target=\"workflow-form\""));
    let updated = state.workflows.get(&record.id).expect("updated");
    assert_eq!(updated.definition.name(), "Review loop");
    assert_eq!(updated.definition.steps().len(), 2);
    assert!(updated.definition.steps()[0].review.is_none());
    let policy = updated.definition.steps()[1]
        .review
        .as_ref()
        .expect("policy");
    assert_eq!(policy.report_output.as_str(), "review");
    assert_eq!(policy.revision_target.as_str(), "work-on-task");
    assert_eq!(policy.attempt_limit, 3);
}

#[tokio::test]
async fn edit_rejects_a_malformed_review_policy() {
    let state = test_state();
    let token = connected(&state);
    let environment = seed_ready_environment(&state).await;
    let record = state
        .workflows
        .create(one_agent_definition(environment))
        .expect("create");
    let body = format!(
        "{}&revision={}",
        create_body(environment).replace(
            "step_0_review-policy=none",
            "step_0_review-policy=conditional",
        ),
        record.revision
    );
    let response = app(&state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/workflows/{}/configuration", record.id.as_hex()))
                .header(header::COOKIE, cookie(&token))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("malformed policy");
    assert_eq!(
        response.status(),
        axum::http::StatusCode::UNPROCESSABLE_ENTITY
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("target=\"workflow-form\""));
    assert!(text.contains("That review policy is not valid."));
    let stored = state.workflows.get(&record.id).expect("unchanged");
    assert_eq!(stored.definition.name(), "One agent");
    assert_eq!(stored.revision, record.revision);
}

#[tokio::test]
async fn delete_redirects_to_the_catalogue() {
    let state = test_state();
    let token = connected(&state);
    let record = state
        .workflows
        .create(one_agent_definition(crate::tests::test_environment_id()))
        .expect("create");
    let response = app(&state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/workflows/{}/delete", record.id.as_hex()))
                .header(header::COOKIE, cookie(&token))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from(format!(
                    "revision={}&confirm=on",
                    record.revision
                )))
                .unwrap(),
        )
        .await
        .expect("delete");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("navigate=\"/workflows\""));
    assert!(state.workflows.list().is_empty());
}

#[tokio::test]
async fn oversized_forms_are_rejected_before_row_allocation() {
    let state = test_state();
    let token = connected(&state);
    let oversized = format!("intent=save&name={}&pad={}", "a", "b".repeat(800_000));
    let response = app(&state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/workflows")
                .header(header::COOKIE, cookie(&token))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from(oversized))
                .unwrap(),
        )
        .await
        .expect("oversize");
    assert_eq!(response.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn anonymous_workflow_authoring_requests_redirect_to_connect() {
    let state = test_state();
    let cases = [
        ("GET", "/workflows/new", None, false),
        ("GET", "/workflows/new", Some("navigation"), true),
        ("POST", "/workflows", None, false),
        ("POST", "/workflows", Some("patch"), true),
    ];
    for (method, uri, graft, enhanced) in cases {
        assert_connect_redirect(&state, method, uri, graft, enhanced).await;
    }
}

#[tokio::test]
async fn workflow_selection_is_non_mutating_and_validates_context_without_a_provider() {
    let state = test_state();
    let conversation = state
        .conversations
        .create("Destination <script>alert(1)</script>".to_owned())
        .unwrap();
    let definition = one_agent_definition(crate::tests::test_environment_id());
    let definition = crate::workflows::definition::WorkflowDefinition::from_parts(
        "Workflow <script>alert(2)</script>".to_owned(),
        definition.default_environment(),
        definition.roles().to_vec(),
        definition.steps().to_vec(),
    )
    .unwrap();
    let workflow = state.workflows.create(definition).unwrap();
    let selection = crate::workflows::WorkflowSelection {
        workflow_id: workflow.id,
        definition_version: workflow.definition_version,
    }
    .as_token();
    let unknown = "0123456789abcdef0123456789abcdef";
    for (context, valid) in [
        (conversation.id.as_hex(), true),
        (String::new(), false),
        (unknown.to_owned(), false),
        ("https%3A%2F%2Fexample.com%2Fevil".to_owned(), false),
    ] {
        for kind in [None, Some("navigation"), Some("patch")] {
            let mut request = Request::builder().uri(format!(
                "/workflows?conversation={context}&workflow={selection}"
            ));
            if let Some(kind) = kind {
                request = request
                    .header(hypergraft::GRAFT_REQUEST, kind)
                    .header(header::ACCEPT, hypergraft::MEDIA_TYPE);
            }
            let response = app(&state)
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            if kind == Some("patch") {
                assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
                continue;
            }
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let body = String::from_utf8(body.to_vec()).unwrap();
            assert_eq!(body.contains(">Back to conversation<"), valid);
            assert!(body.contains(&format!(
                "/conversations/{}/workflow?workflow={selection}",
                conversation.id.as_hex()
            )));
            assert!(!body.contains("example.com"));
            assert!(body.contains("Destination &#60;script&#62;alert(1)&#60;/script&#62;"));
            assert!(body.contains("Workflow &#60;script&#62;alert(2)&#60;/script&#62;"));
            assert!(!body.contains("<script>alert("));
        }
    }
    state
        .workflows
        .update(
            &workflow.id,
            workflow.revision,
            one_agent_definition(crate::tests::test_environment_id()),
        )
        .unwrap();
    for token in ["invalid".to_owned(), format!("{unknown}:1"), selection] {
        let response = app(&state)
            .oneshot(
                Request::builder()
                    .uri(format!("/workflows?workflow={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("That workflow is no longer available."));
        assert!(!body.contains("/workflow?workflow="));
    }
    assert_eq!(state.conversations.list().len(), 1);
    assert_eq!(state.workflows.list().len(), 1);
    assert!(state.workflow_runs.summaries().is_empty());
    assert_eq!(
        state.conversations.get(&conversation.id).unwrap(),
        conversation
    );
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
    if method == "POST" && graft.is_none() {
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    } else if enhanced {
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
