use axum::http::StatusCode;
use tower::ServiceExt;

use super::super::tests::{app, connected, document, get_patch, test_state, text};
use crate::execution::command::{CommandChunk, CommandResult, CommandStream, CommandTermination};

fn store_output(
    state: &crate::state::AppState,
    conversation: crate::conversations::ConversationId,
) -> String {
    state
        .outputs
        .store(
            &crate::execution::OutputKey {
                scope: crate::execution::OutputScope::conversation(conversation),
                job: crate::sessions::JobId::generate().expect("job"),
                tool_call: "call-1".to_owned(),
                model_hidden: false,
            },
            &CommandResult::new(
                vec![
                    CommandChunk {
                        stream: CommandStream::Stdout,
                        text: "hello".to_owned(),
                    },
                    CommandChunk {
                        stream: CommandStream::Stderr,
                        text: "warning".to_owned(),
                    },
                ],
                CommandTermination::Exited(0),
            ),
        )
        .expect("store output")
        .reference
}

#[tokio::test]
async fn output_expansion_patches_the_transcript_record() {
    let state = test_state();
    let token = connected(&state);
    let record = state.conversations.create("Output".to_owned()).unwrap();
    let reference = store_output(&state, record.id);
    let path = format!("/conversations/{}/output?reference={reference}", record.id);
    let response = app(&state).oneshot(get_patch(&path, &token)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("hello"));
    assert!(body.contains("warning"));
    assert!(body.contains(&format!("target=\"output-{reference}\"")));
    // The fragment replaces the preview, so it repeats no expansion control.
    assert!(!body.contains("View full retained output"));
}

#[tokio::test]
async fn output_expansion_rejects_another_conversations_reference() {
    let state = test_state();
    let token = connected(&state);
    let owner = state.conversations.create("Owner".to_owned()).unwrap();
    let other = state.conversations.create("Other".to_owned()).unwrap();
    let reference = store_output(&state, owner.id);
    let path = format!("/conversations/{}/output?reference={reference}", other.id);
    let response = app(&state).oneshot(get_patch(&path, &token)).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = text(response).await;
    assert!(!body.contains("hello"));
    assert!(body.contains("belongs to another conversation"));
}

#[tokio::test]
async fn a_document_get_redirects_to_the_conversation() {
    let state = test_state();
    let token = connected(&state);
    let record = state.conversations.create("Output".to_owned()).unwrap();
    let reference = store_output(&state, record.id);
    let path = format!("/conversations/{}/output?reference={reference}", record.id);
    let response = app(&state).oneshot(document(&path, &token)).await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()[axum::http::header::LOCATION],
        format!("/conversations/{}", record.id.as_hex())
    );
}

#[tokio::test]
async fn a_missing_conversation_navigates_for_a_patch_request() {
    let state = test_state();
    let token = connected(&state);
    let path = "/conversations/00000000000000000000000000000000/output?reference=00000000000000000000000000000000";
    let response = app(&state).oneshot(get_patch(path, &token)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("navigate=\"/conversations\""));
}

#[tokio::test]
async fn the_standalone_output_page_route_is_gone() {
    let state = test_state();
    let token = connected(&state);
    let record = state.conversations.create("Output".to_owned()).unwrap();
    let reference = store_output(&state, record.id);
    let path = format!("/conversations/{}/output/{reference}", record.id);
    let response = app(&state).oneshot(document(&path, &token)).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
