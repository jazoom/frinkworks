use axum::http::StatusCode;
use tower::ServiceExt;

use crate::conversations::questions::{PendingQuestion, QuestionAnswer, QuestionOption};
use crate::providers::ProviderKind;
use crate::slices::conversations::tests::{
    app, command, connected, document, session_id, test_state, text,
};

fn model_record(state: &crate::state::AppState) -> crate::conversations::ConversationRecord {
    let record = state.conversations.create("Question".to_owned()).unwrap();
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

fn pending(
    state: &crate::state::AppState,
    token: &str,
    options: bool,
) -> (
    crate::conversations::ConversationRecord,
    std::sync::Arc<crate::sessions::Job>,
) {
    let session = session_id(token);
    let record = model_record(state);
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
            "Ask me".to_owned(),
        )
        .unwrap();
    let question = PendingQuestion {
        conversation: record.id,
        job: job.id(),
        session,
        tool_call: "call-1".to_owned(),
        prompt: "Which path?".to_owned(),
        options: if options {
            vec![
                QuestionOption {
                    id: "docs".to_owned(),
                    label: "docs".to_owned(),
                },
                QuestionOption {
                    id: "src".to_owned(),
                    label: "src".to_owned(),
                },
            ]
        } else {
            Vec::new()
        },
        allow_free_text: !options,
    };
    state.conversations.submit_question(question).unwrap();
    job.set_awaiting_question();
    (record, job)
}

#[tokio::test]
async fn stop_during_a_question_keeps_observation_until_settlement() {
    let state = test_state();
    let token = connected(&state);
    let (record, job) = pending(&state, &token, true);
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
    assert_eq!(body.matches("id=\"conversation-stop\"").count(), 1);
    assert!(body.contains(&format!("value=\"{}\"", job.id())));
    assert!(job.cancel_requested());
    assert!(
        state
            .conversations
            .pending_question(record.id, job.id())
            .is_none()
    );
    assert_eq!(
        state.conversations.get(&record.id).unwrap().active_job,
        Some(job.id())
    );
}

#[tokio::test]
async fn question_commands_use_patch_representation() {
    let state = test_state();
    let token = connected(&state);
    let (record, job) = pending(&state, &token, true);
    let page = app(&state)
        .oneshot(document(&format!("/conversations/{}", record.id), &token))
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let body = text(page).await;
    assert!(body.contains("id=\"conversation-question\""));
    assert_eq!(body.matches("id=\"conversation-stop\"").count(), 1);
    let path = format!("/conversations/{}/questions/answer", record.id);
    let body = format!("job={}&tool_call=call-1&option=docs", job.id());
    let response = app(&state)
        .oneshot(command(&path, &token, &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        state
            .conversations
            .pending_question(record.id, job.id())
            .is_none()
    );
    assert!(
        state
            .conversations
            .get(&record.id)
            .unwrap()
            .directory_approvals
            .is_empty()
    );

    let mut navigation = document(&path, &token);
    *navigation.method_mut() = axum::http::Method::POST;
    let rejected = app(&state).oneshot(navigation).await.unwrap();
    assert_ne!(rejected.status(), StatusCode::OK);
}

#[tokio::test]
async fn stale_and_duplicate_answers_are_rejected() {
    let state = test_state();
    let token = connected(&state);
    let (record, job) = pending(&state, &token, false);
    let path = format!("/conversations/{}/questions/answer", record.id);
    let stale = format!("job={}&tool_call=call-9&text=docs", job.id());
    let response = app(&state)
        .oneshot(command(&path, &token, &stale))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        state
            .conversations
            .pending_question(record.id, job.id())
            .is_some()
    );

    let valid = format!("job={}&tool_call=call-1&text=docs", job.id());
    let first = app(&state)
        .oneshot(command(&path, &token, &valid))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let second = app(&state)
        .oneshot(command(&path, &token, &valid))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::CONFLICT);
    assert_eq!(
        state
            .conversations
            .wait_question(record.id, &job, "call-1")
            .await,
        Ok(QuestionAnswer::FreeText {
            text: "docs".to_owned()
        })
    );
}

#[tokio::test]
async fn question_cancellation_stays_with_the_job_session() {
    let state = test_state();
    let owner_token = connected(&state);
    let other_token = connected(&state);
    let (record, job) = pending(&state, &owner_token, false);
    let path = format!("/conversations/{}/questions/cancel", record.id);
    let body = format!("job={}&tool_call=call-1", job.id());
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
    assert_eq!(
        state
            .conversations
            .wait_question(record.id, &job, "call-1")
            .await,
        Ok(QuestionAnswer::Cancelled)
    );
    assert!(!job.cancel_requested());
    assert!(
        state
            .sessions
            .owns_conversation_job(&session_id(&owner_token), record.id, job.id())
    );
}

#[tokio::test]
async fn answers_do_not_grant_host_or_directory_authority() {
    let state = test_state();
    let token = connected(&state);
    let (record, job) = pending(&state, &token, true);
    let before = state.conversations.get(&record.id).unwrap();
    assert!(before.directory_approvals.is_empty());
    let path = format!("/conversations/{}/questions/answer", record.id);
    let body = format!("job={}&tool_call=call-1&option=src", job.id());
    let response = app(&state)
        .oneshot(command(&path, &token, &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let after = state.conversations.get(&record.id).unwrap();
    assert!(after.directory_approvals.is_empty());
    assert!(
        state
            .host_approvals
            .pending_for(record.id, job.id())
            .is_none()
    );
    assert_eq!(
        state
            .conversations
            .wait_question(record.id, &job, "call-1")
            .await,
        Ok(QuestionAnswer::Option {
            id: "src".to_owned(),
            label: "src".to_owned()
        })
    );
}
