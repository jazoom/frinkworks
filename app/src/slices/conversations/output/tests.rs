use axum::http::StatusCode;
use tower::ServiceExt;

use super::super::tests::{app, connected, document, navigation, test_state, text};
use crate::execution::command::{CommandChunk, CommandResult, CommandStream, CommandTermination};

#[tokio::test]
async fn output_page_supports_document_navigation() {
    let state = test_state();
    let token = connected(&state);
    let record = state.conversations.create("Output".to_owned()).unwrap();
    let command = CommandResult::new(
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
    );
    let retained = state
        .outputs
        .store(
            &crate::execution::OutputKey {
                scope: crate::execution::OutputScope::conversation(record.id),
                job: crate::sessions::JobId::generate().expect("job"),
                tool_call: "call-1".to_owned(),
                model_hidden: false,
            },
            &command,
        )
        .expect("store output");
    let path = format!("/conversations/{}/output/{}", record.id, retained.reference);

    for request in [document(&path, &token), navigation(&path, &token)] {
        let page = app(&state).oneshot(request).await.unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        let body = text(page).await;
        assert!(body.contains("hello"));
        assert!(body.contains("warning"));
    }
    let mut request = navigation(&path, &token);
    request
        .headers_mut()
        .insert(hypergraft::GRAFT_REQUEST, "patch".parse().unwrap());
    let response = app(&state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn output_page_rejects_another_conversations_reference() {
    let state = test_state();
    let token = connected(&state);
    let owner = state.conversations.create("Owner".to_owned()).unwrap();
    let other = state.conversations.create("Other".to_owned()).unwrap();
    let retained = state
        .outputs
        .store(
            &crate::execution::OutputKey {
                scope: crate::execution::OutputScope::conversation(owner.id),
                job: crate::sessions::JobId::generate().expect("job"),
                tool_call: "call-1".to_owned(),
                model_hidden: false,
            },
            &CommandResult::new(
                vec![CommandChunk {
                    stream: CommandStream::Stdout,
                    text: "private".to_owned(),
                }],
                CommandTermination::Exited(0),
            ),
        )
        .expect("store output");
    let path = format!("/conversations/{}/output/{}", other.id, retained.reference);
    let page = app(&state).oneshot(document(&path, &token)).await.unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let body = text(page).await;
    assert!(!body.contains("private"));
    assert!(body.contains("belongs to another conversation"));
}
