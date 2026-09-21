use axum::http::StatusCode;
use tower::ServiceExt;

use crate::providers::ProviderKind;
use crate::slices::conversations::tests::{
    app, command, connected, document, session_id, test_state, text,
};

fn model_record(state: &crate::state::AppState) -> crate::conversations::ConversationRecord {
    let record = state.conversations.create("Compact".to_owned()).unwrap();
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
        Vec::new(),
        crate::tests::test_environment_id(),
    )
    .unwrap();
    state
        .conversations
        .update_execution_settings(&record.id, record.revision, settings)
        .unwrap()
}

fn seed_exchanges(
    state: &crate::state::AppState,
    token: &str,
) -> crate::conversations::ConversationRecord {
    let session = session_id(token);
    let record = model_record(state);
    let selection = record.model.as_ref().unwrap().settings.model.clone();
    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .unwrap();
    let record = state
        .conversations
        .begin_message(
            &record.id,
            record.revision,
            selection.clone(),
            job.id(),
            "First".to_owned(),
        )
        .unwrap();
    state
        .conversations
        .settle_message(
            &record.id,
            job.id(),
            crate::providers::AssistantReply::from("First reply"),
            crate::conversations::MessageStatus::Complete,
            None,
        )
        .unwrap();
    state
        .sessions
        .finish_conversation_job(&session, record.id, job.id());
    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .unwrap();
    let record = state.conversations.get(&record.id).unwrap();
    let record = state
        .conversations
        .begin_message(
            &record.id,
            record.revision,
            selection,
            job.id(),
            "Second".to_owned(),
        )
        .unwrap();
    state
        .conversations
        .settle_message(
            &record.id,
            job.id(),
            crate::providers::AssistantReply::from("Second reply"),
            crate::conversations::MessageStatus::Complete,
            None,
        )
        .unwrap();
    state
        .sessions
        .finish_conversation_job(&session, record.id, job.id());
    state.conversations.get(&record.id).unwrap()
}

#[tokio::test]
async fn compact_commands_use_patch_representation() {
    let state = test_state();
    let token = connected(&state);
    let record = seed_exchanges(&state, &token);
    let path = format!("/conversations/{}/compact", record.id);
    let body = format!("revision={}", record.revision);
    let response = app(&state)
        .oneshot(command(&path, &token, &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = text(response).await;
    wait_settled(&state, record.id).await;
    let stored = state.conversations.get(&record.id).unwrap();
    assert!(stored.compaction.is_some());
    assert_eq!(stored.messages, record.messages);
    assert_eq!(stored.summary_requests.len(), 1);

    let mut navigation = document(&path, &token);
    *navigation.method_mut() = axum::http::Method::POST;
    let rejected = app(&state).oneshot(navigation).await.unwrap();
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn compact_rejects_an_active_conversation() {
    let state = test_state();
    let token = connected(&state);
    let record = seed_exchanges(&state, &token);
    let session = session_id(&token);
    let _job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .unwrap();
    let path = format!("/conversations/{}/compact", record.id);
    let body = format!("revision={}", record.revision);
    let response = app(&state)
        .oneshot(command(&path, &token, &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

async fn wait_settled(state: &crate::state::AppState, id: crate::conversations::ConversationId) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while state.sessions.conversation_reserved(id) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("summary settlement");
}

#[tokio::test]
async fn incomplete_summaries_never_replace_context_and_retain_request_usage() {
    use crate::providers::tests::ScriptedBackend;
    use crate::providers::{ChatBackend, CompletionReason, ModelEvent};
    for reason in [
        None,
        Some(CompletionReason::Unknown),
        Some(CompletionReason::Length),
    ] {
        let mut state = test_state();
        let mut events = vec![Ok(ModelEvent::Text("Plausible partial summary".to_owned()))];
        if let Some(reason) = reason {
            events.push(Ok(ModelEvent::Complete { reason }));
        }
        state.chat = std::sync::Arc::new(ChatBackend::Scripted(ScriptedBackend::events(events)));
        let token = connected(&state);
        let record = seed_exchanges(&state, &token);
        let path = format!("/conversations/{}/compact", record.id);
        let response = app(&state)
            .oneshot(command(
                &path,
                &token,
                &format!("revision={}", record.revision),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        wait_settled(&state, record.id).await;
        let stored = state.conversations.get(&record.id).unwrap();
        assert!(stored.compaction.is_none());
        assert_eq!(stored.summary_requests.len(), 1);
        assert_eq!(
            crate::conversations::compaction::project(&stored.messages, None, None).unwrap(),
            crate::conversations::compaction::project(&record.messages, None, None).unwrap()
        );
    }
}

#[tokio::test]
async fn cancellation_drops_a_stalled_summary_and_holds_the_reservation_until_settlement() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let mut state = test_state();
    let started = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    state.chat = Arc::new(crate::providers::ChatBackend::Scripted(
        crate::providers::tests::ScriptedBackend::hang_watched(started.clone(), dropped.clone()),
    ));
    let token = connected(&state);
    let record = seed_exchanges(&state, &token);
    let path = format!("/conversations/{}/compact", record.id);
    let body = format!("revision={}", record.revision);
    let response = app(&state)
        .oneshot(command(&path, &token, &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        state
            .sessions
            .begin_conversation_job(&session_id(&token), record.id)
            .is_err()
    );
    let duplicate = app(&state)
        .oneshot(command(&path, &token, &body))
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    let stored = state.conversations.get(&record.id).unwrap();
    state
        .sessions
        .conversation_job(record.id, stored.active_job.unwrap())
        .unwrap()
        .request_cancel();
    wait_settled(&state, record.id).await;
    assert!(dropped.load(Ordering::SeqCst));
    assert!(
        state
            .conversations
            .get(&record.id)
            .unwrap()
            .compaction
            .is_none()
    );
}
