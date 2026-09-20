use axum::http::StatusCode;
use tower::ServiceExt;

use crate::{
    agents::ToolId,
    execution::ToolLocation,
    providers::ProviderKind,
    slices::conversations::tests::{app, command, connected, session_id, test_state, text},
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
