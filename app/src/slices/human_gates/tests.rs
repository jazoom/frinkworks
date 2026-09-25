use crate::{sessions, state::AppState, workflows};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    middleware::from_fn_with_state,
};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

fn app(state: &AppState) -> axum::Router {
    crate::slices::router()
        .layer(from_fn_with_state(
            state.clone(),
            crate::sessions::resolve_session,
        ))
        .layer(axum::middleware::from_fn(hypergraft::middleware::classify))
        .with_state(state.clone())
}

pub(crate) fn conversation_at_gate() -> (
    AppState,
    String,
    sessions::SessionId,
    workflows::WorkflowRun,
) {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let token = sessions::generate_session_token().unwrap();
    state.sessions.insert(token.id());
    let connection = crate::providers::ProviderConnection::with_key(
        crate::providers::ProviderKind::Xai,
        "test-key",
        "grok-4.6",
    );
    state.vault.put(connection.clone()).unwrap();
    let mut settings = workflows::tests::settings();
    settings.model = crate::providers::ModelSelection::new(
        crate::providers::ProviderKind::Xai,
        "grok-4.6".into(),
        state
            .models_dev
            .effective_effort(crate::providers::ProviderKind::Xai, "grok-4.6", None),
    )
    .unwrap();
    let conversation = state
        .conversations
        .create_saved(
            crate::conversations::ConversationId::generate().unwrap(),
            Some("Plan review".into()),
            Some(crate::conversations::ConversationModelConfiguration {
                settings,
                preset: None,
            }),
            vec![],
        )
        .unwrap();
    let (run, directory) = workflows::handoff::tests::prepared_run(&state, &conversation);
    let settings = run.directory_settings().unwrap();
    let current = state
        .conversations
        .update_execution_settings(&conversation.id, conversation.revision, settings.clone())
        .unwrap();
    state.workflow_runs.create(run.clone()).unwrap();
    state.keep_temp_dir(directory);
    state
        .access_consent
        .approve_handoff(run.id, token.id(), conversation.id, vec![settings.clone()])
        .unwrap();
    let job = state
        .sessions
        .begin_conversation_job(&token.id(), conversation.id)
        .unwrap();
    state
        .conversations
        .begin_message(
            &conversation.id,
            current.revision,
            settings.model.clone(),
            job.id(),
            "Plan the task".into(),
        )
        .unwrap();
    let authority =
        crate::execution::ProjectFreeAuthority::from_settings(current.revision, &settings).unwrap();
    job.set_awaiting_decision();
    state.gate_continuations.insert(workflows::WorkflowJob {
        run_id: run.id,
        session_id: token.id(),
        agent_id: None,
        agent_revision: current.revision,
        conversation_id: Some(conversation.id),
        authority: None,
        project_free_authority: Some(authority.clone()),
        connection,
        phase_providers: Vec::new(),
        active_connection: Arc::new(Mutex::new(None)),
        host_policy: authority.policy,
        turns: Vec::new(),
        job,
        eligible_reply: Arc::new(Mutex::new("Plan ready".into())),
    });
    (state, token.raw().as_str().to_owned(), token.id(), run)
}

#[tokio::test]
async fn plan_gate_supports_document_and_navigation_but_not_targeted_get() {
    let (state, token, _, run) = conversation_at_gate();
    let path = format!("/runs/{}/gates/{}", run.id, run.gates[0].id);
    for kind in [None, Some("navigation"), Some("patch")] {
        let mut request = Request::builder()
            .uri(&path)
            .header(header::COOKIE, format!("frinkworks_session={token}"));
        if let Some(kind) = kind {
            request = request
                .header("Graft-Request", kind)
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE);
        }
        let response = app(&state)
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if kind == Some("patch") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::OK
            }
        );
        if kind != Some("patch") {
            let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let body = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(body.contains(&run.gates[0].candidate.artefact_hash.as_str()));
            for form in body
                .split("<form")
                .skip(1)
                .filter_map(|part| part.split_once("</form>").map(|(form, _)| form))
            {
                if !form.contains("name=\"plan\"") {
                    continue;
                }
                let requires_note = !form.contains("/approve");
                let mut pairs = vec![
                    (
                        "gate-revision".into(),
                        run.gates[0].revision.get().to_string(),
                    ),
                    (
                        "plan".into(),
                        run.gates[0].candidate.artefact_hash.as_str().to_owned(),
                    ),
                ];
                if form.contains("name=\"note\"") {
                    pairs.push(("note".into(), "Fix the plan".into()));
                }
                assert!(super::forms::DecisionForm::parse(pairs, requires_note).is_ok());
            }
        }
    }
}

#[tokio::test]
async fn stale_plan_decision_does_not_advance_the_run() {
    let (state, token, _, run) = conversation_at_gate();
    let path = format!("/runs/{}/gates/{}/approve", run.id, run.gates[0].id);
    let response = app(&state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(header::COOKIE, format!("frinkworks_session={token}"))
                .header("Graft-Request", "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("gate-revision=999&plan=sha256%3Ainvalid"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(state.workflow_runs.get(&run.id).unwrap(), run);
}

#[test]
fn decision_forms_reject_duplicate_and_unknown_fields() {
    for pairs in [
        vec![("unknown".into(), "x".into())],
        vec![
            ("gate-revision".into(), "1".into()),
            ("candidate".into(), "obsolete".into()),
        ],
        vec![
            ("gate-revision".into(), "1".into()),
            ("gate-revision".into(), "2".into()),
        ],
    ] {
        assert!(super::forms::DecisionForm::parse(pairs.clone(), false).is_err());
        assert!(super::forms::EnvironmentSwitchDecisionForm::parse(pairs).is_err());
    }
}
