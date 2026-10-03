use crate::{
    preferences::Preferences,
    providers::{ModelSelection, ProviderConnection, ProviderKind, ThinkingEffort},
    slices::conversations::tests::{
        app, command, connected, document, hidden_named, navigation, test_state, text,
    },
};
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use tower::ServiceExt;

const PATH: &str = "/conversations/models/default";

#[tokio::test]
async fn defaults_require_a_session_and_patch_commands() {
    let state = test_state();
    let token = connected(&state);
    for request in [
        document(PATH, &token),
        navigation(PATH, &token),
        Request::builder()
            .method("POST")
            .uri(PATH)
            .header(header::COOKIE, format!("frinkworks_session={token}"))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from("kind=model&provider=xai&model=grok-4.7"))
            .unwrap(),
    ] {
        let response = app(&state).oneshot(request).await.unwrap();
        assert_ne!(response.status(), StatusCode::OK);
    }
    assert_eq!(
        state
            .preferences
            .selected_provider(&state.vault)
            .unwrap()
            .model,
        "grok-4.6"
    );
    state.vault.forget(ProviderKind::Xai).unwrap();
    let response = app(&state)
        .oneshot(command(
            PATH,
            &token,
            "kind=model&provider=xai&model=grok-4.7",
        ))
        .await
        .unwrap();
    assert!(text(response).await.contains("navigate=\"/connect\""));
}

#[tokio::test]
async fn invalid_defaults_leave_preferences_unchanged() {
    let mut state = test_state();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preferences.json");
    state.preferences = std::sync::Arc::new(Preferences::open(path.clone()));
    let token = connected(&state);
    state
        .vault
        .put(ProviderConnection::with_key(
            ProviderKind::Deepseek,
            "test-key",
            "deepseek-flash",
        ))
        .unwrap();
    for fields in [
        "kind=unknown&provider=xai&model=grok-4.7",
        "kind=model&provider=unknown&model=grok-4.7",
        "kind=model&provider=openrouter&model=openai/gpt-5",
        "kind=model&provider=xai&model=%3Cscript%3E",
        "kind=model&provider=deepseek&model=deepseek-v4-flash",
        "kind=thinking&provider=xai&model=grok-4.7&thinking=maximum",
        "kind=thinking&provider=xai&model=grok-4.7&thinking=",
        "kind=thinking&provider=xai&model=grok-build-0.1&thinking=high",
        "kind=thinking&provider=xai&model=grok-4.7&thinking=high%00",
    ] {
        let response = app(&state)
            .oneshot(command(PATH, &token, fields))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{fields}"
        );
        assert!(
            text(response)
                .await
                .contains("target=\"conversation-model-defaults\"")
        );
        assert!(!path.exists());
    }
    assert!(state.conversations.list().is_empty());
}

#[tokio::test]
async fn normalised_defaults_persist_independently_without_changes_to_saved_conversations() {
    let mut state = test_state();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preferences.json");
    state.preferences = std::sync::Arc::new(Preferences::open(path.clone()));
    let token = connected(&state);
    let selection = ModelSelection::new(
        ProviderKind::Xai,
        "grok-4.6".to_owned(),
        ThinkingEffort::new("low".to_owned()),
    )
    .unwrap();
    let settings = crate::execution::ExecutionSettings::new(
        selection,
        "Keep these instructions.".to_owned(),
        vec![],
        crate::tests::test_environment_id(),
    )
    .unwrap();
    state
        .preferences
        .set_conversation_defaults(settings.clone())
        .unwrap();
    let record = state.conversations.create("Original".to_owned()).unwrap();
    let record = state
        .conversations
        .update_execution_settings(&record.id, record.revision, settings.clone())
        .unwrap();
    for (fields, model, thinking) in [
        (
            "kind=thinking&provider=+xai+&model=+grok-4.7+&thinking=+xhigh+",
            "grok-4.6",
            "xhigh",
        ),
        (
            "kind=model&provider=+xai+&model=+grok-4.7+&thinking=low",
            "grok-4.7",
            "xhigh",
        ),
        (
            "kind=model&provider=xai&model=grok-build-0.1",
            "grok-build-0.1",
            "xhigh",
        ),
    ] {
        let response = app(&state)
            .oneshot(command(PATH, &token, fields))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = text(response).await;
        assert!(body.contains("target=\"conversation-model-defaults\""));
        assert!(!body.contains("target=\"conversation-detail\""));
        state.preferences = std::sync::Arc::new(Preferences::open(path.clone()));
        assert_eq!(
            state
                .preferences
                .selected_provider(&state.vault)
                .unwrap()
                .model,
            model
        );
        assert_eq!(
            state.preferences.default_thinking().unwrap().as_str(),
            thinking
        );
        let saved = state.preferences.conversation_defaults().unwrap();
        assert_eq!(saved.instructions, settings.instructions);
        assert_eq!(saved.tools, settings.tools);
        let response = app(&state)
            .oneshot(document("/conversations/new", &token))
            .await
            .unwrap();
        let body = text(response).await;
        assert_eq!(hidden_named(&body, "model"), model);
        let select = body
            .split("id=\"conversation-thinking\"")
            .nth(1)
            .unwrap()
            .split("</select>")
            .next()
            .unwrap();
        if model == "grok-build-0.1" {
            assert!(select.contains("disabled"));
            assert!(select.contains("value=\"\""));
        } else {
            let option = select
                .split(&format!("value=\"{thinking}\""))
                .nth(1)
                .unwrap()
                .split('>')
                .next()
                .unwrap();
            assert!(option.contains("selected"));
        }
        assert_eq!(state.conversations.get(&record.id), Some(record.clone()));
    }
    assert_eq!(state.conversations.list().len(), 1);
}

#[tokio::test]
async fn failed_persistence_reports_an_error_without_a_default_change() {
    let mut state = test_state();
    let directory = tempfile::tempdir().unwrap();
    state.preferences = std::sync::Arc::new(Preferences::open(directory.path().to_owned()));
    let token = connected(&state);
    for fields in [
        "kind=model&provider=xai&model=grok-4.7",
        "kind=thinking&provider=xai&model=grok-4.7&thinking=high",
    ] {
        let response = app(&state)
            .oneshot(command(PATH, &token, fields))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            text(response)
                .await
                .contains("Frinkworks cannot save the default.")
        );
    }
    assert_eq!(
        state
            .preferences
            .selected_provider(&state.vault)
            .unwrap()
            .model,
        "grok-4.6"
    );
    assert!(state.preferences.default_thinking().is_none());
}
