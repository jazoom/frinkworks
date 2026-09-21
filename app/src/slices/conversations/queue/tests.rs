use axum::http::StatusCode;
use tower::ServiceExt;

use crate::conversations::QueueDelivery;
use crate::providers::ProviderKind;
use crate::slices::conversations::tests::{
    app, command, connected, document, session_id, test_state, text,
};

fn model_record(state: &crate::state::AppState) -> crate::conversations::ConversationRecord {
    let record = state.conversations.create("Queue".to_owned()).unwrap();
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(
            ProviderKind::Xai,
            "grok-4.6".to_owned(),
            Some(effort),
        )
        .unwrap(),
        String::new(),
        crate::agents::ToolId::ALL.to_vec(),
        crate::tests::test_environment_id(),
    )
    .unwrap();
    state
        .conversations
        .update_execution_settings(&record.id, record.revision, settings)
        .unwrap()
}

#[tokio::test]
async fn queue_commands_use_patch_representation() {
    let state = test_state();
    let token = connected(&state);
    let record = model_record(&state);
    let path = format!("/conversations/{}/queue", record.id);
    let body = format!(
        "queue_revision={}&message=Later+please&delivery=follow-up",
        record.queue.revision
    );
    let response = app(&state)
        .oneshot(command(&path, &token, &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let page = text(response).await;
    assert!(page.contains("Later please"));
    assert!(page.contains("Follow-up"));
    assert!(
        state
            .conversations
            .get(&record.id)
            .unwrap()
            .messages
            .is_empty()
    );

    let mut navigation = document(&path, &token);
    *navigation.method_mut() = axum::http::Method::POST;
    let rejected = app(&state).oneshot(navigation).await.unwrap();
    assert_ne!(rejected.status(), StatusCode::OK);
}

#[tokio::test]
async fn stale_queue_revision_does_not_duplicate_items() {
    let state = test_state();
    let token = connected(&state);
    let record = model_record(&state);
    let path = format!("/conversations/{}/queue", record.id);
    let body = format!(
        "queue_revision={}&message=Once&delivery=follow-up",
        record.queue.revision
    );
    let first = app(&state)
        .oneshot(command(&path, &token, &body))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let second = app(&state)
        .oneshot(command(&path, &token, &body))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::CONFLICT);
    assert_eq!(
        state
            .conversations
            .get(&record.id)
            .unwrap()
            .queue
            .items
            .len(),
        1
    );
}

#[tokio::test]
async fn queue_mutations_stay_with_the_job_session() {
    let state = test_state();
    let owner_token = connected(&state);
    let owner = session_id(&owner_token);
    let other_token = connected(&state);
    let record = model_record(&state);
    let job = state
        .sessions
        .begin_conversation_job(&owner, record.id)
        .unwrap();
    let record = state
        .conversations
        .begin_message_with_model(
            &record.id,
            record.revision,
            record.model.clone(),
            job.id(),
            "Working".to_owned(),
        )
        .unwrap();
    let path = format!("/conversations/{}/queue", record.id);
    let body = format!(
        "queue_revision={}&message=Steer&delivery=steering",
        record.queue.revision
    );
    let rejected = app(&state)
        .oneshot(command(&path, &other_token, &body))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    let accepted = app(&state)
        .oneshot(command(&path, &owner_token, &body))
        .await
        .unwrap();
    assert_eq!(
        accepted.status(),
        StatusCode::OK,
        "{}",
        text(accepted).await
    );
    let queued = state.conversations.get(&record.id).unwrap();
    assert_eq!(queued.queue.items.len(), 1);
    assert_eq!(queued.queue.items[0].delivery, QueueDelivery::Steering);
}

#[tokio::test]
async fn return_to_editor_keeps_the_item_when_the_editor_has_text() {
    let state = test_state();
    let token = connected(&state);
    let record = model_record(&state);
    let queued = state
        .conversations
        .enqueue(
            &record.id,
            record.queue.revision,
            "Queued text".to_owned(),
            QueueDelivery::FollowUp,
            None,
        )
        .unwrap();
    let item = queued.queue.items[0].id;
    let path = format!(
        "/conversations/{}/queue/{}/editor",
        record.id,
        item.as_hex()
    );
    let body = format!(
        "queue_revision={}&editor=Unsent+draft",
        queued.queue.revision
    );
    let response = app(&state)
        .oneshot(command(&path, &token, &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let page = text(response).await;
    assert!(page.contains("editor already has text"));
    assert_eq!(
        state
            .conversations
            .get(&record.id)
            .unwrap()
            .queue
            .items
            .len(),
        1
    );
    let replace = format!(
        "queue_revision={}&editor=Queued+text",
        queued.queue.revision
    );
    let replaced = app(&state)
        .oneshot(command(&path, &token, &replace))
        .await
        .unwrap();
    assert_eq!(replaced.status(), StatusCode::OK);
    assert!(
        state
            .conversations
            .get(&record.id)
            .unwrap()
            .queue
            .items
            .is_empty()
    );
}
