use axum::http::StatusCode;
use tower::ServiceExt;

use super::*;
use crate::slices::conversations::tests::{
    app, command, connected, document, get_patch, navigation, session_id, test_state, text,
};

fn selection(record: &crate::conversations::ConversationRecord) -> String {
    format!("selected={}:{}", record.id.as_hex(), record.revision)
}

#[tokio::test]
async fn deletion_is_patch_only_and_requires_an_explicit_exact_selection() {
    let state = test_state();
    let token = connected(&state);
    let record = state.conversations.create("Keep".into()).unwrap();
    for path in ["/conversations/delete/review", "/conversations/delete"] {
        for request in [
            document(path, &token),
            navigation(path, &token),
            get_patch(path, &token),
        ] {
            assert_eq!(
                app(&state).oneshot(request).await.unwrap().status(),
                StatusCode::METHOD_NOT_ALLOWED
            );
        }
        let mut native = command(path, &token, &selection(&record));
        native.headers_mut().remove(hypergraft::GRAFT_REQUEST);
        native.headers_mut().remove(axum::http::header::ACCEPT);
        assert_eq!(
            app(&state).oneshot(native).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
    for body in [
        selection(&record),
        "confirm=delete&scope=all".into(),
        "confirm=delete&selected=invalid".into(),
        format!(
            "confirm=delete&{}&{}",
            selection(&record),
            selection(&record)
        ),
    ] {
        assert_eq!(
            app(&state)
                .oneshot(command("/conversations/delete", &token, &body))
                .await
                .unwrap()
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert!(state.conversations.contains(&record.id));
}

#[tokio::test]
async fn all_matches_freezes_ids_across_pages_and_preserves_later_and_unmatched_records() {
    let state = test_state();
    let token = connected(&state);
    let mut selected = Vec::new();
    for number in 0..53 {
        selected.push(
            state
                .conversations
                .create(format!("Delete {number}"))
                .unwrap(),
        );
    }
    let other = state.conversations.create("Keep".into()).unwrap();
    let response = app(&state)
        .oneshot(command(
            "/conversations/delete/review",
            &token,
            "scope=all&q=DELETE",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("target=\"conversation-delete-review\""));
    assert_eq!(body.matches("name=\"selected\"").count(), 53);
    for record in &selected {
        assert!(body.contains(&format!("{}:{}", record.id.as_hex(), record.revision)));
        assert!(state.conversations.contains(&record.id));
    }
    assert!(!body.contains(&other.id.as_hex()));
    let later = state.conversations.create("Delete later".into()).unwrap();
    let body = format!(
        "confirm=delete&q=DELETE&{}",
        selected.iter().map(selection).collect::<Vec<_>>().join("&")
    );
    let response = app(&state)
        .oneshot(command("/conversations/delete", &token, &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        text(response)
            .await
            .contains("navigate=\"/conversations?q=DELETE\"")
    );
    assert!(
        selected
            .iter()
            .all(|record| !state.conversations.contains(&record.id))
    );
    assert!(state.conversations.contains(&other.id));
    assert!(state.conversations.contains(&later.id));
}

#[tokio::test]
async fn stale_missing_and_reserved_selections_never_delete_other_records() {
    let state = test_state();
    let token = connected(&state);
    let first = state.conversations.create("First".into()).unwrap();
    let second = state.conversations.create("Second".into()).unwrap();
    let body = format!(
        "confirm=delete&{}&{}",
        selection(&first),
        selection(&second)
    );
    let reservation = state
        .sessions
        .reserve_command(session_id(&token), second.id)
        .unwrap();
    let response = app(&state)
        .oneshot(command("/conversations/delete", &token, &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(state.conversations.contains(&first.id));
    drop(reservation);
    state
        .conversations
        .rename(&second.id, second.revision, "Changed".into())
        .unwrap();
    let response = app(&state)
        .oneshot(command("/conversations/delete", &token, &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(state.conversations.contains(&first.id));
    let latest = state.conversations.get(&second.id).unwrap();
    state
        .conversations
        .delete(&latest.id, latest.revision)
        .unwrap();
    let response = app(&state)
        .oneshot(command("/conversations/delete", &token, &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(state.conversations.contains(&first.id));
}

#[tokio::test]
async fn pending_decisions_block_bulk_deletion() {
    let state = test_state();
    let token = connected(&state);
    crate::slices::conversations::tests::awaiting_gate(&state);
    let response = app(&state)
        .oneshot(command("/conversations/delete/review", &token, "scope=all"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = text(response).await;
    assert!(body.contains(BLOCKED));
    assert!(!body.contains("Delete permanently</button>"));
    assert_eq!(state.conversations.metadata().len(), 1);
}

#[test]
fn selection_input_has_bounds_and_no_ambiguous_fields() {
    for pairs in [
        vec![("q".into(), "x".repeat(257))],
        vec![
            ("scope".into(), "all".into()),
            ("scope".into(), "selected".into()),
        ],
        vec![("conversation".into(), "//other.test".into())],
    ] {
        assert!(DeleteForm::parse(pairs).is_err());
    }
    let pairs = (0..=MAXIMUM_SELECTION)
        .map(|_| {
            (
                "selected".into(),
                format!("{}:1", ConversationId::generate().unwrap().as_hex()),
            )
        })
        .collect();
    assert!(DeleteForm::parse(pairs).is_err());
}
