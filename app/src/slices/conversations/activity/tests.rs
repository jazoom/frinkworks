use axum::{body::Body, http::Request};
use tower::ServiceExt;

use super::super::tests::{
    app, command, connected, document, navigation, session_id, test_state, text,
};

fn host_settings(state: &crate::state::AppState) -> crate::execution::ExecutionSettings {
    let effort = state
        .models_dev
        .effective_effort(crate::providers::ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(
            crate::providers::ProviderKind::Xai,
            "grok-4.6".to_owned(),
            Some(effort),
        )
        .unwrap(),
        String::new(),
        vec![crate::agents::ToolId::Run],
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_location(crate::execution::ToolLocation::Host)
}

#[tokio::test]
async fn activity_supports_document_navigation_and_targeted_updates() {
    let state = test_state();
    let token = connected(&state);
    let record = state.conversations.create("Activity".to_owned()).unwrap();
    let path = format!("/conversations/{}/activity", record.id);

    let page = app(&state).oneshot(document(&path, &token)).await.unwrap();
    assert_eq!(page.status(), axum::http::StatusCode::OK);
    let body = text(page).await;
    assert!(body.contains("id=\"conversation-detail\""));
    assert!(body.contains("id=\"activity-detail\""));
    assert!(body.contains(&format!("href=\"/conversations/{}\"", record.id)));

    let enhanced = app(&state)
        .oneshot(navigation(&path, &token))
        .await
        .unwrap();
    assert_eq!(enhanced.status(), axum::http::StatusCode::OK);
    assert!(text(enhanced).await.contains("target=\"chat-main\""));

    let patch = Request::builder()
        .uri(&path)
        .header("Cookie", format!("powerplant_session={token}"))
        .header(hypergraft::GRAFT_REQUEST, "patch")
        .header("Accept", hypergraft::MEDIA_TYPE)
        .body(Body::empty())
        .unwrap();
    let fragment = app(&state).oneshot(patch).await.unwrap();
    assert_eq!(fragment.status(), axum::http::StatusCode::OK);
    let fragment = text(fragment).await;
    assert!(fragment.contains("target=\"conversation-detail\""));
    assert!(fragment.contains("id=\"activity-detail\""));

    let unknown = app(&state)
        .oneshot(document(
            "/conversations/00000000000000000000000000000000/activity",
            &token,
        ))
        .await
        .unwrap();
    assert!(unknown.status().is_redirection());
}

#[tokio::test]
async fn host_approval_shows_actual_command_and_location_before_decision() {
    let state = test_state();
    let token = connected(&state);
    let session = session_id(&token);
    let record = state.conversations.create("Host".to_owned()).unwrap();
    let settings = host_settings(&state);
    let record = state
        .conversations
        .update_execution_settings(&record.id, record.revision, settings.clone())
        .unwrap();
    state
        .access_consent
        .approve_host_conversation(
            &state
                .access_consent
                .request_host_conversation(session, record.id, &settings)
                .unwrap(),
            session,
            record.id,
            &settings,
        )
        .unwrap();
    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .unwrap();
    let record = state
        .conversations
        .begin_message_with_model(
            &record.id,
            record.revision,
            None,
            job.id(),
            "Run the checks.".to_owned(),
        )
        .unwrap();
    let directory = std::env::current_dir().unwrap();
    let request = state
        .host_approvals
        .submit(crate::execution::HostCommandRequest {
            token: String::new(),
            session,
            job: job.id(),
            conversation: record.id,
            execution_revision: record.revision,
            command: "printf hi".to_owned(),
            directory: directory.clone(),
            explanation: "Print a greeting".to_owned(),
            run: None,
            step: None,
            attempt: None,
        })
        .unwrap();
    job.set_awaiting_decision();

    let page = app(&state)
        .oneshot(document(&format!("/conversations/{}", record.id), &token))
        .await
        .unwrap();
    assert_eq!(page.status(), axum::http::StatusCode::OK);
    let body = text(page).await;
    assert!(body.contains("printf hi"));
    assert!(body.contains(&directory.display().to_string()));
    assert!(body.contains("Print a greeting"));
    assert!(body.contains("Unrestricted host access"));

    let path = format!("/conversations/{}/host-command/reject", record.id);
    let rejection = format!(
        "revision={}&job={}&request={}&command=%22printf+hi%22",
        record.revision,
        job.id(),
        request
    );
    let decided = app(&state)
        .oneshot(command(&path, &token, &rejection))
        .await
        .unwrap();
    assert_eq!(decided.status(), axum::http::StatusCode::OK);
    assert!(
        state
            .host_approvals
            .pending_for(record.id, job.id())
            .is_none()
    );
}
