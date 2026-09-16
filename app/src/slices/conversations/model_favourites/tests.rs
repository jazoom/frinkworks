use crate::providers::ProviderKind;
use crate::slices::conversations::tests::{app, command, connected, document, test_state, text};
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use tower::ServiceExt;

#[tokio::test]
async fn favourites_require_patch_commands_and_connected_catalogue_models() {
    let state = test_state();
    let token = connected(&state);
    let path = "/conversations/models/favourite";
    for request in [
        document(path, &token),
        Request::builder()
            .method("POST")
            .uri(path)
            .header(header::COOKIE, format!("powerplant_session={token}"))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from("provider=xai&model=grok-4.6"))
            .unwrap(),
    ] {
        let response = app(&state).oneshot(request).await.unwrap();
        assert!(response.status().is_client_error());
    }
    for fields in [
        "provider=missing&model=grok-4.6",
        "provider=deepseek&model=deepseek-chat",
        "provider=xai&model=",
        "provider=xai&model=%3Cscript%3E",
    ] {
        let response = app(&state)
            .oneshot(command(path, &token, fields))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            text(response)
                .await
                .contains("target=\"conversation-model-catalogue\"")
        );
    }
    assert!(
        state.preferences.desk_providers(&state.vault)[0]
            .favourites
            .is_empty()
    );
    assert!(state.conversations.list().is_empty());
}

#[tokio::test]
async fn favourites_persist_without_changing_selection_or_creating_a_conversation() {
    let mut state = test_state();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preferences.json");
    state.preferences = std::sync::Arc::new(crate::preferences::Preferences::open(path.clone()));
    let token = connected(&state);
    for expected in [true, false] {
        let response = app(&state)
            .oneshot(command(
                "/conversations/models/favourite",
                &token,
                "provider=xai&model=+grok-4.6+",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = text(response).await;
        assert!(body.contains("target=\"conversation-model-catalogue\""));
        assert!(!body.contains("target=\"conversation-detail\""));
        let reopened = crate::preferences::Preferences::open(path.clone());
        let provider = reopened
            .desk_providers(&state.vault)
            .into_iter()
            .find(|provider| provider.kind == ProviderKind::Xai)
            .unwrap();
        assert_eq!(
            provider.favourites.contains(&"grok-4.6".to_owned()),
            expected
        );
        assert_eq!(provider.model, "grok-4.6");
        assert!(provider.thinking.is_none());
        assert!(reopened.conversation_defaults().is_none());
        assert!(state.conversations.list().is_empty());
    }
}
