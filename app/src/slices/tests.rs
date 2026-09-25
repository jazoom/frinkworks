use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    middleware::from_fn_with_state,
};
use tower::ServiceExt;

#[tokio::test]
async fn tool_free_activation_reaches_chat_without_source_capture() {
    let mut state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    state.environments.apply_production_seeds();
    state.chat = std::sync::Arc::new(crate::providers::ChatBackend::Scripted(
        crate::tests::ScriptedBackend::chunks([Ok("Hello from Frinkworks.".into())]),
    ));
    let app = || {
        crate::slices::router()
            .layer(from_fn_with_state(
                state.clone(),
                crate::sessions::resolve_session,
            ))
            .layer(from_fn_with_state(
                state.clone(),
                crate::security::enforce_origin,
            ))
            .layer(axum::middleware::from_fn(hypergraft::middleware::classify))
            .with_state(state.clone())
    };
    let response = app()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/connect");
    state
        .vault
        .put(crate::providers::ProviderConnection::with_key(
            crate::providers::ProviderKind::Xai,
            "test-key",
            "grok-4.6",
        ))
        .unwrap();
    let token = crate::sessions::generate_session_token().unwrap();
    state.sessions.insert(token.id());
    let body = format!(
        "action=send&provider=xai&model=grok-4.6&thinking={}&message=Hello",
        state
            .models_dev
            .effective_effort(crate::providers::ProviderKind::Xai, "grok-4.6", None)
            .unwrap()
            .as_str()
    );
    let response = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/conversations/new")
                .header(
                    header::COOKIE,
                    format!("frinkworks_session={}", token.raw().as_str()),
                )
                .header(header::ORIGIN, "http://localhost:4000")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header("Graft-Request", "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while state
            .conversations
            .list()
            .iter()
            .any(|record| record.active_job.is_some())
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(state.workflow_runs.summaries().is_empty());
    let record = state.conversations.list().pop().unwrap();
    assert!(
        record
            .messages
            .iter()
            .any(|message| message.text.contains("Hello from Frinkworks."))
    );
}
