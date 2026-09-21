use axum::http::StatusCode;
use tower::ServiceExt;

use super::super::tests::{
    app, command, connected, document, form_value, hidden_named, navigation, session_id,
    test_state, text,
};
use crate::{
    conversations::{ConversationModelConfiguration, ConversationRecord, MessageStatus},
    providers::{
        AssistantActivity, AssistantReply, ChatBackend, ModelSelection, ProviderKind, ToolOutput,
        tests::ScriptedBackend,
    },
    sessions::JobId,
    state::AppState,
};

async fn source_with_tool_use(state: &AppState) -> ConversationRecord {
    state.environments.apply_production_seeds();
    let environment = super::super::default_environment(state).expect("environment");
    let record = state
        .conversations
        .create_saved(
            crate::conversations::ConversationId::generate().expect("conversation"),
            Some("Source".to_owned()),
            Some(ConversationModelConfiguration::direct(
                ModelSelection::new(
                    ProviderKind::Xai,
                    "grok-4.6".to_owned(),
                    state
                        .models_dev
                        .effective_effort(ProviderKind::Xai, "grok-4.6", None),
                )
                .expect("selection"),
                environment,
            )),
            Vec::new(),
        )
        .expect("source");
    let job = JobId::generate().expect("job");
    let record = state
        .conversations
        .begin_message_with_model(
            &record.id,
            record.revision,
            None,
            job,
            "Run the command".to_owned(),
        )
        .expect("begin");
    let tool = ToolOutput {
        resource: None,
        label: "run".to_owned(),
        output: "hi".to_owned(),
        command: None,
    };
    let mut reply = AssistantReply::default();
    reply.push_response("Done");
    reply.activity.push(AssistantActivity::ToolCall {
        id: "call-1".to_owned(),
        name: "run".to_owned(),
        arguments: serde_json::json!({ "command": "echo hi" }),
        result: Some(tool.clone()),
    });
    reply.tools.push(tool);
    state
        .conversations
        .settle_message(&record.id, job, reply, MessageStatus::Complete, None)
        .expect("settle");
    state.conversations.get(&record.id).expect("source record")
}

fn draft_fields(
    record: &ConversationRecord,
    nonce: &str,
    fork_source: &str,
    message: &str,
) -> String {
    let settings = record.model.as_ref().expect("model");
    format!(
        "action=send&provider={}&model={}&thinking={}&environment={}&message={}&draft_nonce={}&fork_source={}",
        settings.settings.model.provider.as_str(),
        settings.settings.model.model,
        settings
            .settings
            .model
            .thinking
            .as_ref()
            .map_or("", |effort| effort.as_str()),
        settings.settings.environment.as_hex(),
        form_value(message),
        nonce,
        fork_source,
    )
}

#[tokio::test]
async fn fork_routes_support_document_navigation_and_patch() {
    let state = test_state();
    let token = connected(&state);
    let record = source_with_tool_use(&state).await;
    let boundary = record.messages.last().expect("reply").id;
    let path = format!("/conversations/{}/fork?message={}", record.id, boundary);

    let document_response = app(&state).oneshot(document(&path, &token)).await.unwrap();
    assert_eq!(document_response.status(), StatusCode::OK);
    let document_body = text(document_response).await;
    assert_eq!(document_body.matches("id=\"chat-main\"").count(), 1);
    assert!(document_body.contains("Fork conversation"));

    let navigation_response = app(&state)
        .oneshot(navigation(&path, &token))
        .await
        .unwrap();
    assert_eq!(navigation_response.status(), StatusCode::OK);
    assert!(
        text(navigation_response)
            .await
            .contains("operation=\"children\" target=\"chat-main\"")
    );

    let patch_response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/fork", record.id),
            &token,
            &format!("revision={}&message={boundary}", record.revision),
        ))
        .await
        .unwrap();
    assert_eq!(patch_response.status(), StatusCode::OK);
    let patch_body = text(patch_response).await;
    assert!(patch_body.contains("location=\"/conversations/new\""));
    assert!(patch_body.contains("target=\"chat-main\""));
    assert_eq!(state.conversations.list().len(), 1);
}

#[tokio::test]
async fn a_fork_after_tool_use_copies_context_without_starting_an_agent() {
    let mut state = test_state();
    let backend = ScriptedBackend::accept();
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend.clone()));
    let token = connected(&state);
    let record = source_with_tool_use(&state).await;
    let boundary = record.messages.last().expect("reply").id;
    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/fork", record.id),
            &token,
            &format!("revision={}&message={boundary}", record.revision),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("location=\"/conversations/new\""));
    assert!(body.contains("Fork draft"));
    assert_eq!(state.conversations.list().len(), 1);
    assert!(backend.last_history().is_empty());

    let sent = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            &draft_fields(
                &record,
                &hidden_named(&body, "draft_nonce"),
                &hidden_named(&body, "fork_source"),
                "Continue from the fork",
            ),
        ))
        .await
        .unwrap();
    assert_eq!(sent.status(), StatusCode::OK, "{}", text(sent).await);
    let destination = state
        .conversations
        .list()
        .into_iter()
        .find(|record| record.forked_from.is_some())
        .expect("fork destination");
    let provenance = destination.forked_from.clone().expect("provenance");
    assert_eq!(provenance.source, record.id);
    assert_eq!(provenance.source_revision, record.revision);
    assert_eq!(provenance.boundary, boundary);
    assert!(destination.directory_approvals.is_empty());
    assert_eq!(destination.messages.len(), 4);
    assert!(destination.messages[1].activity.iter().any(|activity| {
        matches!(activity, AssistantActivity::ToolCall { id, .. } if id == "call-1")
    }));
    assert_eq!(destination.messages[2].text, "Continue from the fork");
    let duplicate = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            &draft_fields(
                &record,
                &hidden_named(&body, "draft_nonce"),
                &hidden_named(&body, "fork_source"),
                "Continue from the fork",
            ),
        ))
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    assert_eq!(state.conversations.list().len(), 2);
}

#[tokio::test]
async fn a_fork_beside_pending_prepared_changes_keeps_ownership() {
    let mut state = test_state();
    let backend = ScriptedBackend::accept();
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend));
    let token = connected(&state);
    let record = source_with_tool_use(&state).await;
    let (run, _directory) = crate::workflows::handoff::tests::prepared_run(&state, &record);
    state.workflow_runs.create(run.clone()).expect("run");
    let boundary = record.messages.last().expect("reply").id;
    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/fork", record.id),
            &token,
            &format!("revision={}&message={boundary}", record.revision),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    let sent = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            &draft_fields(
                &record,
                &hidden_named(&body, "draft_nonce"),
                &hidden_named(&body, "fork_source"),
                "Fork beside prepared changes",
            ),
        ))
        .await
        .unwrap();
    assert_eq!(sent.status(), StatusCode::OK, "{}", text(sent).await);
    let owned = state.workflow_runs.get(&run.id).expect("run");
    assert_eq!(owned.conversation_id, Some(record.id));
    assert!(state.workflow_runs.for_conversation(&record.id).len() == 1);
    let destination = state
        .conversations
        .list()
        .into_iter()
        .find(|record| record.forked_from.is_some())
        .expect("fork destination");
    assert!(
        state
            .workflow_runs
            .for_conversation(&destination.id)
            .is_empty()
    );
}

#[tokio::test]
async fn stale_and_unsafe_boundaries_do_not_create_drafts() {
    let state = test_state();
    let token = connected(&state);
    let record = source_with_tool_use(&state).await;
    let boundary = record.messages.last().expect("reply").id;
    let path = format!("/conversations/{}/fork", record.id);
    for (fields, status) in [
        (
            format!("revision=0&message={boundary}"),
            StatusCode::CONFLICT,
        ),
        (
            format!(
                "revision={}&message={}",
                record.revision,
                crate::conversations::MessageId::generate()
                    .expect("unknown")
                    .as_hex()
            ),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let response = app(&state)
            .oneshot(command(&path, &token, &fields))
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert!(
            !text(response)
                .await
                .contains("location=\"/conversations/new\"")
        );
    }
    assert_eq!(state.conversations.list().len(), 1);
}

#[tokio::test]
async fn another_session_cannot_consume_a_fork_draft() {
    let state = test_state();
    let owner = connected(&state);
    let other = connected(&state);
    let record = source_with_tool_use(&state).await;
    let boundary = record.messages.last().expect("reply").id;
    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/fork", record.id),
            &owner,
            &format!("revision={}&message={boundary}", record.revision),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    let fields = draft_fields(
        &record,
        &hidden_named(&body, "draft_nonce"),
        &hidden_named(&body, "fork_source"),
        "Stolen fork token",
    );
    let response = app(&state)
        .oneshot(command("/conversations/new", &other, &fields))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(state.conversations.list().len(), 1);
}

#[tokio::test]
async fn an_expired_fork_token_never_becomes_an_empty_conversation() {
    let state = test_state();
    let token = connected(&state);
    let record = source_with_tool_use(&state).await;
    let boundary = record.messages.last().expect("reply").id;
    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/fork", record.id),
            &token,
            &format!("revision={}&message={boundary}", record.revision),
        ))
        .await
        .unwrap();
    let body = text(response).await;
    let nonce = hidden_named(&body, "draft_nonce");
    let fork_source = hidden_named(&body, "fork_source");
    state
        .forks
        .claim(session_id(&token), &nonce)
        .expect("claim")
        .commit();
    let response = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            &draft_fields(&record, &nonce, &fork_source, "Lost context"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(state.conversations.list().len(), 1);
}
