use axum::http::StatusCode;
use tower::ServiceExt;

use super::super::tests::{
    app, connected, document, navigation, normalised, seeded_history, session_id, test_state, text,
};

#[tokio::test]
async fn tree_route_renders_bounded_entries_without_mutating_the_conversation() {
    let state = test_state();
    let token = connected(&state);
    let record = seeded_history(&state, "Tree", 40);
    let job = state
        .sessions
        .begin_conversation_job(&session_id(&token), record.id)
        .unwrap();
    let selection = crate::providers::ModelSelection::new(
        crate::providers::ProviderKind::Xai,
        "grok-4.6".to_owned(),
        None,
    )
    .unwrap();
    let record = state
        .conversations
        .begin_message(
            &record.id,
            record.revision,
            selection,
            job.id(),
            "Unfinished input".to_owned(),
        )
        .unwrap();
    let base = format!("/conversations/{}/tree", record.id.as_hex());
    let before = state.conversations.get(&record.id).expect("record");
    let catalogue_before = state.conversations.metadata().len();

    let body = text(
        app(&state)
            .oneshot(document(&base, &token))
            .await
            .expect("document"),
    )
    .await;
    assert!(normalised(&body).contains("Question 0"));
    assert!(normalised(&body).contains("Reply 0"));
    let tree = body.split_once("id=\"tree-detail\"").unwrap().1;
    assert!(!normalised(tree).contains("Question 32"));
    assert!(body.contains(&format!("/conversations/{}?around=", record.id.as_hex())));

    let response = app(&state)
        .oneshot(navigation(&base, &token))
        .await
        .expect("navigation");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("target=\"chat-main\""));

    let mut patch = navigation(&base, &token);
    patch
        .headers_mut()
        .insert(hypergraft::GRAFT_REQUEST, "patch".parse().unwrap());
    let response = app(&state).oneshot(patch).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("target=\"tree-detail\""));
    assert!(!body.contains("target=\"conversation-detail\""));

    let after = state.conversations.get(&record.id).expect("record");
    assert_eq!(before.revision, after.revision);
    assert_eq!(before.messages, after.messages);
    assert_eq!(after.active_job, Some(job.id()));
    assert!(state.sessions.conversation_reserved(record.id));
    assert_eq!(
        state
            .conversations
            .metadata_for(&record.id)
            .and_then(|metadata| metadata.active_leaf),
        after.messages.last().map(|message| message.id)
    );
    assert_eq!(state.conversations.metadata().len(), catalogue_before);
}

#[tokio::test]
async fn tree_search_never_changes_the_active_leaf_or_revision() {
    let state = test_state();
    let token = connected(&state);
    let record = seeded_history(&state, "Tree", 3);
    let base = format!("/conversations/{}/tree", record.id.as_hex());
    let before = state.conversations.get(&record.id).expect("record");

    let body = text(
        app(&state)
            .oneshot(document(&format!("{base}?q=Question+1"), &token))
            .await
            .expect("document"),
    )
    .await;
    let tree = body.split_once("id=\"tree-detail\"").unwrap().1;
    assert!(normalised(tree).contains("Question 1"));
    assert!(!normalised(tree).contains("Question 0 "));

    let after = state.conversations.get(&record.id).expect("record");
    assert_eq!(before.revision, after.revision);
    assert_eq!(before.messages, after.messages);
}

#[tokio::test]
async fn tree_inspection_leaf_is_read_only() {
    let state = test_state();
    let token = connected(&state);
    let record = seeded_history(&state, "Tree", 2);
    let base = format!("/conversations/{}", record.id.as_hex());
    let leaf = record.messages.last().expect("leaf").id.as_hex();
    let anchor = record.messages.first().expect("entry").id.as_hex();
    let before = state.conversations.get(&record.id).expect("record");

    let body = text(
        app(&state)
            .oneshot(document(
                &format!("{base}?leaf={leaf}&around={anchor}"),
                &token,
            ))
            .await
            .expect("document"),
    )
    .await;
    assert!(body.contains("Earlier"));
    assert!(body.contains("Later"));

    let response = app(&state)
        .oneshot(document(
            &format!("{base}?leaf=nothex&around={anchor}"),
            &token,
        ))
        .await
        .expect("document");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let foreign = seeded_history(&state, "Other tree", 1);
    for suffix in [
        format!("?leaf={}", foreign.messages[0].id.as_hex()),
        format!("/tree?parent={}", foreign.messages[0].id.as_hex()),
        "/tree?cursor=-1".to_owned(),
        "/tree?cursor=invalid".to_owned(),
        format!("/tree?q={}", "x".repeat(201)),
    ] {
        let response = app(&state)
            .oneshot(document(&format!("{base}{suffix}"), &token))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{suffix}"
        );
    }
    let body = text(
        app(&state)
            .oneshot(document(&format!("{base}?leaf={leaf}"), &token))
            .await
            .unwrap(),
    )
    .await;
    assert!(body.contains("id=\"conversation-history-status\""));

    let after = state.conversations.get(&record.id).expect("record");
    assert_eq!(before.revision, after.revision);
    assert_eq!(before.messages, after.messages);
}
