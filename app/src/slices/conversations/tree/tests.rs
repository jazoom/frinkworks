use axum::http::StatusCode;
use tower::ServiceExt;

use super::super::tests::{
    app, command, connected, document, navigation, normalised, seeded_history, session_id,
    test_state, text,
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

#[tokio::test]
async fn continue_here_selects_an_earlier_response_and_keeps_alternatives() {
    let mut state = test_state();
    let backend = crate::providers::tests::ScriptedBackend::accept();
    state.chat = std::sync::Arc::new(crate::providers::ChatBackend::Scripted(backend.clone()));
    let token = connected(&state);
    super::super::tests::ready_starter_environment(&state).await;
    let record = seeded_history(&state, "Branches", 3);
    let mut settings = record.model.as_ref().unwrap().settings.clone();
    settings.environment = super::super::default_environment(&state).unwrap();
    settings.tools.clear();
    settings.model = crate::providers::ModelSelection::new(
        crate::providers::ProviderKind::Xai,
        "grok-4.6".to_owned(),
        state
            .models_dev
            .effective_effort(crate::providers::ProviderKind::Xai, "grok-4.6", None),
    )
    .unwrap();
    let record = state
        .conversations
        .update_execution_settings(&record.id, record.revision, settings)
        .unwrap();
    let destination = record.messages[1].id;
    let leaf = record.messages.last().unwrap().id;
    let action = format!("/conversations/{}/tree/continue", record.id.as_hex());
    let body = format!(
        "revision={}&active_leaf={}&destination={}",
        record.revision,
        leaf.as_hex(),
        destination.as_hex()
    );
    let response = app(&state)
        .oneshot(command(&action, &token, &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let updated = state.conversations.get(&record.id).unwrap();
    assert_eq!(updated.messages.last().unwrap().id, destination);
    assert_eq!(updated.revision, record.revision + 1);

    // A repeated stale form changes no branch and returns a conflict.
    let response = app(&state)
        .oneshot(command(&action, &token, &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        state.conversations.get(&record.id).unwrap().revision,
        updated.revision
    );

    // Existing descendants stay retained and visible as alternatives.
    let body = text(
        app(&state)
            .oneshot(document(
                &format!("/conversations/{}/tree", record.id.as_hex()),
                &token,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(normalised(&body).contains("Question 1"));
    assert!(normalised(&body).contains("Reply 2"));
    assert!(normalised(&body).contains("Continue here"));
    assert_eq!(
        state
            .conversations
            .metadata_for(&record.id)
            .and_then(|metadata| metadata.active_leaf),
        Some(destination)
    );

    assert!(backend.last_history().is_empty());
    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/messages", record.id),
            &token,
            &format!("revision={}&message=Alternative", updated.revision),
        ))
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(status, StatusCode::OK, "{}", text(response).await);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while state
            .conversations
            .get(&record.id)
            .unwrap()
            .active_job
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("settlement");
    let loaded = state.conversations.get(&record.id).unwrap();
    assert_eq!(loaded.messages[2].parent, Some(destination));
    let history = backend.last_history();
    let texts: Vec<_> = history.iter().map(|turn| turn.text.as_str()).collect();
    assert_eq!(texts, vec!["Question 0", "Reply 0", "Alternative"]);
}

#[tokio::test]
async fn continue_here_rejects_unsettled_destinations_without_changing_the_branch() {
    let state = test_state();
    let token = connected(&state);
    let record = seeded_history(&state, "Unsettled branch", 1);
    let selection = crate::providers::ModelSelection::new(
        crate::providers::ProviderKind::Xai,
        "grok-4.6".to_owned(),
        None,
    )
    .unwrap();
    let job = crate::sessions::JobId::generate().unwrap();
    let pending = state
        .conversations
        .begin_message(
            &record.id,
            record.revision,
            selection,
            job,
            "Unfinished".to_owned(),
        )
        .unwrap();
    let pending_id = pending.messages.last().unwrap().id;
    let action = format!("/conversations/{}/tree/continue", record.id.as_hex());
    let body = format!(
        "revision={}&active_leaf={}&destination={}",
        pending.revision,
        pending_id.as_hex(),
        pending_id.as_hex()
    );
    let response = app(&state)
        .oneshot(command(&action, &token, &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        state.conversations.get(&record.id).unwrap().messages,
        pending.messages
    );

    state
        .conversations
        .settle_message(
            &record.id,
            job,
            "",
            crate::conversations::MessageStatus::Interrupted,
            None,
        )
        .unwrap();
    let settled = state.conversations.get(&record.id).unwrap();
    let interrupted = settled.messages.last().unwrap().id;
    let body = format!(
        "revision={}&active_leaf={}&destination={}",
        settled.revision,
        interrupted.as_hex(),
        interrupted.as_hex()
    );
    let response = app(&state)
        .oneshot(command(&action, &token, &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        state.conversations.get(&record.id).unwrap().messages,
        settled.messages
    );
}

#[tokio::test]
async fn revise_restores_a_root_prompt_without_mutating_the_conversation() {
    let state = test_state();
    let token = connected(&state);
    let record = seeded_history(&state, "Revise root", 2);
    let source = record.messages[0].id;
    let base = format!("/conversations/{}", record.id.as_hex());
    let before = state.conversations.get(&record.id).expect("record");

    let response = app(&state)
        .oneshot(document(
            &format!("{base}?revise={}", source.as_hex()),
            &token,
        ))
        .await
        .expect("document");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("data-revision-state"));
    assert!(body.contains("name=\"revise_source\""));
    assert!(body.contains("name=\"revise_parent\""));
    assert!(body.contains("name=\"revise_active_leaf\""));
    assert!(body.contains("data-revision-text=\"Question 0\""));

    // A stale or invalid source opens no draft and writes nothing.
    for suffix in [
        format!("?revise={}", record.messages[1].id.as_hex()),
        "?revise=nothex".to_owned(),
        format!(
            "?revise={}",
            super::super::tests::seeded_history(&state, "Other", 1).messages[0]
                .id
                .as_hex()
        ),
    ] {
        let response = app(&state)
            .oneshot(document(&format!("{base}{suffix}"), &token))
            .await
            .expect("document");
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{suffix}"
        );
        assert!(!text(response).await.contains("data-revision-state"));
    }

    let after = state.conversations.get(&record.id).expect("record");
    assert_eq!(before.revision, after.revision);
    assert_eq!(before.messages, after.messages);
}

#[tokio::test]
async fn revise_appends_a_root_replacement_and_rejects_a_stale_form() {
    let mut state = test_state();
    let backend = crate::providers::tests::ScriptedBackend::accept();
    state.chat = std::sync::Arc::new(crate::providers::ChatBackend::Scripted(backend.clone()));
    let token = connected(&state);
    super::super::tests::ready_starter_environment(&state).await;
    let record = seeded_history(&state, "Revise append", 2);
    let mut settings = record.model.as_ref().unwrap().settings.clone();
    settings.environment = super::super::default_environment(&state).unwrap();
    settings.tools.clear();
    settings.model = crate::providers::ModelSelection::new(
        crate::providers::ProviderKind::Xai,
        "grok-4.6".to_owned(),
        state
            .models_dev
            .effective_effort(crate::providers::ProviderKind::Xai, "grok-4.6", None),
    )
    .unwrap();
    let record = state
        .conversations
        .update_execution_settings(&record.id, record.revision, settings)
        .unwrap();
    let source = record.messages[0].id;
    let active_leaf = record.messages.last().unwrap().id;
    let action = format!("/conversations/{}/messages", record.id.as_hex());
    let body = format!(
        "revision={}&message=Revised+question&revise_source={}&revise_parent=&revise_active_leaf={}",
        record.revision,
        source.as_hex(),
        active_leaf.as_hex()
    );

    let response = app(&state)
        .oneshot(command(&action, &token, &body))
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(status, StatusCode::OK, "{}", text(response).await);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while state
            .conversations
            .get(&record.id)
            .unwrap()
            .active_job
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("settlement");

    let loaded = state.conversations.get(&record.id).unwrap();
    // The replacement starts a fresh root branch. It does not extend the
    // original first exchange.
    assert_eq!(loaded.messages.len(), 2);
    assert_eq!(loaded.messages[0].text, "Revised question");
    assert_eq!(loaded.messages[0].parent, None);
    assert_eq!(loaded.messages[1].parent, Some(loaded.messages[0].id));

    // Retained descendants stay reachable through the tree as alternatives.
    let tree = text(
        app(&state)
            .oneshot(document(
                &format!("/conversations/{}/tree", record.id.as_hex()),
                &token,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(normalised(&tree).contains("Question 0"));
    assert!(normalised(&tree).contains("Reply 1"));
    let history: Vec<_> = backend
        .last_history()
        .iter()
        .map(|turn| turn.text.clone())
        .collect();
    assert_eq!(history, vec!["Revised question"]);

    // A stale form changes no branch and starts no model request.
    let before = state.conversations.get(&record.id).unwrap();
    let stale = format!(
        "revision={}&message=Stale+question&revise_source={}&revise_parent=&revise_active_leaf={}",
        record.revision,
        source.as_hex(),
        active_leaf.as_hex()
    );
    let response = app(&state)
        .oneshot(command(&action, &token, &stale))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let after = state.conversations.get(&record.id).unwrap();
    assert_eq!(before.messages, after.messages);
    assert_eq!(before.revision, after.revision);
    assert!(
        backend
            .last_history()
            .iter()
            .all(|turn| turn.text != "Stale question")
    );
}

#[tokio::test]
async fn revise_rejects_a_non_user_source_and_a_mismatched_parent() {
    let mut state = test_state();
    let backend = crate::providers::tests::ScriptedBackend::accept();
    state.chat = std::sync::Arc::new(crate::providers::ChatBackend::Scripted(backend.clone()));
    let token = connected(&state);
    super::super::tests::ready_starter_environment(&state).await;
    let record = seeded_history(&state, "Revise guard", 2);
    let mut settings = record.model.as_ref().unwrap().settings.clone();
    settings.environment = super::super::default_environment(&state).unwrap();
    settings.tools.clear();
    settings.model = crate::providers::ModelSelection::new(
        crate::providers::ProviderKind::Xai,
        "grok-4.6".to_owned(),
        state
            .models_dev
            .effective_effort(crate::providers::ProviderKind::Xai, "grok-4.6", None),
    )
    .unwrap();
    let record = state
        .conversations
        .update_execution_settings(&record.id, record.revision, settings)
        .unwrap();
    let assistant = record.messages[1].id;
    let source = record.messages[0].id;
    let active_leaf = record.messages.last().unwrap().id;
    let action = format!("/conversations/{}/messages", record.id.as_hex());
    let before = state.conversations.get(&record.id).unwrap();

    let body = format!(
        "revision={}&message=Nope&revise_source={}&revise_parent=&revise_active_leaf={}",
        record.revision,
        assistant.as_hex(),
        active_leaf.as_hex()
    );
    let response = app(&state)
        .oneshot(command(&action, &token, &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        state.conversations.get(&record.id).unwrap().messages,
        before.messages
    );

    let body = format!(
        "revision={}&message=Nope&revise_source={}&revise_parent={}&revise_active_leaf={}",
        record.revision,
        source.as_hex(),
        assistant.as_hex(),
        active_leaf.as_hex()
    );
    let response = app(&state)
        .oneshot(command(&action, &token, &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        state.conversations.get(&record.id).unwrap().messages,
        before.messages
    );
    assert!(backend.last_history().is_empty());
}
