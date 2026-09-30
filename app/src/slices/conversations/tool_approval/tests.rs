use axum::http::StatusCode;
use tower::ServiceExt;

use crate::{
    agents::ToolId,
    execution::ToolLocation,
    providers::ProviderKind,
    slices::conversations::tests::{
        app, command, connected, document, session_id, test_state, text,
    },
};

fn host_settings(state: &crate::state::AppState) -> crate::execution::ExecutionSettings {
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(
            ProviderKind::Xai,
            "grok-4.6".to_owned(),
            Some(effort),
        )
        .unwrap(),
        String::new(),
        vec![ToolId::Run],
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_location(ToolLocation::Host)
}

fn pending_command(
    state: &crate::state::AppState,
    token: &str,
    location: ToolLocation,
) -> (
    crate::conversations::ConversationRecord,
    std::sync::Arc<crate::sessions::Job>,
    crate::execution::HostCommandRequest,
) {
    let session = session_id(token);
    let record = state
        .conversations
        .create("Command approval".to_owned())
        .unwrap();
    // A workflow phase can differ from the conversation's default location.
    let settings = host_settings(state).with_location(match location {
        ToolLocation::Host => ToolLocation::Sandbox,
        ToolLocation::Sandbox => ToolLocation::Host,
    });
    let record = state
        .conversations
        .update_execution_settings(&record.id, record.revision, settings)
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
            record.model.clone(),
            job.id(),
            "Inspect the directory.".to_owned(),
        )
        .unwrap();
    job.start_tool(
        "call-run".to_owned(),
        "run".to_owned(),
        serde_json::json!({"command": "printf hi"}),
    );
    state
        .host_approvals
        .submit(crate::execution::HostCommandRequest {
            token: String::new(),
            session,
            job: job.id(),
            conversation: record.id,
            execution_revision: record.revision,
            location,
            command: "printf hi".to_owned(),
            directory: if location == ToolLocation::Sandbox {
                "/scratch"
            } else {
                "/tmp"
            }
            .into(),
            explanation: "Print a greeting".to_owned(),
            run: None,
            step: None,
            attempt: None,
        })
        .unwrap();
    job.set_awaiting_decision();
    let request = state
        .host_approvals
        .pending_for(record.id, job.id())
        .unwrap();
    (record, job, request)
}

#[tokio::test]
async fn command_approval_discloses_the_request_location_without_granting_authority() {
    for location in [ToolLocation::Sandbox, ToolLocation::Host] {
        let state = test_state();
        let token = connected(&state);
        let (record, job, request) = pending_command(&state, &token, location);
        let path = format!("/conversations/{}?work=true", record.id);
        let response = app(&state).oneshot(document(&path, &token)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = text(response).await;
        let approval = body
            .split("id=\"conversation-host-command\"")
            .nth(1)
            .unwrap()
            .split("</section>")
            .next()
            .unwrap();
        assert_eq!(
            approval.contains("Unrestricted host access"),
            location == ToolLocation::Host
        );
        assert_eq!(
            approval.contains("Sandbox access"),
            location == ToolLocation::Sandbox
        );
        assert!(approval.contains(&request.directory.display().to_string()));
        assert_eq!(body.matches("id=\"conversation-stop\"").count(), 1);
        assert!(body.contains("Awaiting command approval"));
        assert_eq!(
            state.host_approvals.pending_for(record.id, job.id()),
            Some(request)
        );
        assert_eq!(
            job.snapshot().status,
            crate::sessions::JobStatus::AwaitingDecision
        );
        assert!(
            state
                .conversations
                .get(&record.id)
                .unwrap()
                .directory_approvals
                .is_empty()
        );
    }
}

#[tokio::test]
async fn stop_during_command_approval_revokes_the_request_and_keeps_observation() {
    let state = test_state();
    let token = connected(&state);
    let (record, job, request) = pending_command(&state, &token, ToolLocation::Sandbox);
    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/cancel", record.id),
            &token,
            &format!("job={}", job.id()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("target=\"conversation-detail\""));
    assert!(body.contains("data-observe-active=\"true\""));
    assert!(job.cancel_requested());
    assert_eq!(
        state.conversations.get(&record.id).unwrap().active_job,
        Some(job.id())
    );
    assert!(
        state
            .host_approvals
            .pending_for(record.id, job.id())
            .is_none()
    );
    assert_eq!(
        state
            .host_approvals
            .decide(&request, crate::execution::HostCommandDecision::Approved),
        Err(crate::execution::ApprovalError::Invalid)
    );
}

#[tokio::test]
async fn host_command_approval_rejects_tampering_duplicates_and_stale_jobs() {
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
    let token_request = state
        .host_approvals
        .submit(crate::execution::HostCommandRequest {
            token: String::new(),
            session,
            job: job.id(),
            conversation: record.id,
            execution_revision: record.revision,
            command: "printf hi".to_owned(),
            directory: std::env::current_dir().unwrap(),
            explanation: "Print a greeting".to_owned(),
            location: ToolLocation::Host,
            run: None,
            step: None,
            attempt: None,
        })
        .unwrap();
    job.set_awaiting_decision();

    let path = format!("/conversations/{}/host-command/approve", record.id);
    let tampered = format!(
        "revision={}&job={}&request={}&command=rm+-rf+/",
        record.revision,
        job.id(),
        token_request
    );
    let response = app(&state)
        .oneshot(command(&path, &token, &tampered))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let valid = format!(
        "revision={}&job={}&request={}&command=%22printf+hi%22",
        record.revision,
        job.id(),
        token_request
    );
    let response = app(&state)
        .oneshot(command(&path, &token, &valid))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        text(response).await
    );

    let response = app(&state)
        .oneshot(command(&path, &token, &valid))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn host_command_approval_rejects_stale_revision_and_foreign_session() {
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
    let token_request = state
        .host_approvals
        .submit(crate::execution::HostCommandRequest {
            token: String::new(),
            session,
            job: job.id(),
            conversation: record.id,
            execution_revision: record.revision,
            command: "printf hi".to_owned(),
            directory: std::env::current_dir().unwrap(),
            explanation: "Print a greeting".to_owned(),
            location: ToolLocation::Host,
            run: None,
            step: None,
            attempt: None,
        })
        .unwrap();
    job.set_awaiting_decision();
    let path = format!("/conversations/{}/host-command/approve", record.id);
    let stale = format!(
        "revision={}&job={}&request={}&command=%22printf+hi%22",
        record.revision + 1,
        job.id(),
        token_request
    );
    let response = app(&state)
        .oneshot(command(&path, &token, &stale))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        state
            .host_approvals
            .pending_for(record.id, job.id())
            .is_some()
    );

    let other = crate::sessions::generate_session_token().unwrap();
    state.sessions.insert(other.id());
    let foreign = format!(
        "revision={}&job={}&request={}&command=%22printf+hi%22",
        record.revision,
        job.id(),
        token_request
    );
    let response = app(&state)
        .oneshot(command(&path, other.raw().as_str(), &foreign))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        state
            .host_approvals
            .pending_for(record.id, job.id())
            .is_some()
    );
}
