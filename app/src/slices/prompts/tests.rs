use crate::{
    providers::{ProviderConnection, ProviderKind},
    state::AppState,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    middleware::from_fn_with_state,
};
use tower::ServiceExt;

fn fixture() -> (AppState, String) {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    state
        .vault
        .put(ProviderConnection::with_key(
            ProviderKind::Xai,
            "test-key",
            "grok-4.6",
        ))
        .unwrap();
    let token = crate::sessions::generate_session_token().unwrap();
    state.sessions.insert(token.id());
    (
        state,
        format!("frinkworks_session={}", token.raw().as_str()),
    )
}

fn app(state: &AppState) -> axum::Router {
    super::router()
        .layer(from_fn_with_state(
            state.clone(),
            crate::sessions::resolve_session,
        ))
        .layer(axum::middleware::from_fn(hypergraft::middleware::classify))
        .with_state(state.clone())
}

fn request(
    method: &str,
    path: &str,
    cookie: &str,
    graft: Option<&str>,
    body: String,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(graft) = graft {
        builder = builder
            .header(hypergraft::GRAFT_REQUEST, graft)
            .header(header::ACCEPT, hypergraft::MEDIA_TYPE);
    }
    builder.body(Body::from(body)).unwrap()
}

#[tokio::test]
async fn catalogue_supports_documents_and_navigation_but_not_targeted_patches() {
    let (state, cookie) = fixture();
    for graft in [None, Some("navigation"), Some("patch")] {
        let response = app(&state)
            .oneshot(request(
                "GET",
                "/prompts?new=1",
                &cookie,
                graft,
                String::new(),
            ))
            .await
            .unwrap();
        if graft == Some("patch") {
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            continue;
        }
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = std::str::from_utf8(&body).unwrap();
        if graft.is_none() {
            assert!(text.contains("<!doctype html>"));
            assert_eq!(text.matches("id=\"chat-main\"").count(), 1);
        } else {
            assert!(text.contains("target=\"chat-main\""));
        }
    }
}

#[tokio::test]
async fn mutations_require_patch_requests_and_a_session() {
    let (state, cookie) = fixture();
    state
        .prompts
        .create(
            "review",
            crate::conversations::prompts::compose("Review.", "", "Review $1."),
        )
        .unwrap();
    for graft in [None, Some("navigation")] {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("name", "review")
            .append_pair("fingerprint", "stale")
            .append_pair("body", "Changed.")
            .finish();
        let response = app(&state)
            .oneshot(request("POST", "/prompts/save", &cookie, graft, body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    assert_eq!(state.prompts.get("review").unwrap().body, "Review $1.");

    let disconnected = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let response = app(&disconnected)
        .oneshot(request(
            "POST",
            "/prompts/save",
            "",
            Some("patch"),
            String::new(),
        ))
        .await
        .unwrap();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(
        std::str::from_utf8(&body)
            .unwrap()
            .contains("navigate=\"/connect\"")
    );
}

#[tokio::test]
async fn invalid_input_is_escaped_and_retained_without_a_write() {
    let (state, cookie) = fixture();
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("name", "bad name")
        .append_pair("body", "</textarea><script>alert('not markup')</script>")
        .finish();
    let response = app(&state)
        .oneshot(request(
            "POST",
            "/prompts/save",
            &cookie,
            Some("patch"),
            body,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = std::str::from_utf8(&body).unwrap();
    assert!(!text.contains("<script>"));
    assert!(text.contains("&#60;/textarea&#62;"), "{text}");
    assert!(text.contains("bad name"), "{text}");
    assert!(text.contains("target=\"chat-main\""));
    assert!(state.prompts.list().unwrap().records.is_empty());
}

#[tokio::test]
async fn a_malformed_file_stays_escaped_and_revision_bound_during_repair() {
    let (state, cookie) = fixture();
    let raw = "---\n</textarea><script>untrusted</script>";
    std::fs::write(state.prompts.host_dir().join("broken.md"), raw).unwrap();
    let record = state.prompts.get("broken").unwrap();
    let response = app(&state)
        .oneshot(request(
            "GET",
            "/prompts?edit=broken",
            &cookie,
            None,
            String::new(),
        ))
        .await
        .unwrap();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("&#60;/textarea&#62;"));
    assert!(!text.contains("<script>untrusted"));
    assert!(text.contains(&record.fingerprint));
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("name", "broken")
        .append_pair("fingerprint", &record.fingerprint)
        .append_pair("body", "Repaired $1.")
        .finish();
    let response = app(&state)
        .oneshot(request(
            "POST",
            "/prompts/save",
            &cookie,
            Some("patch"),
            body,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let repaired = state.prompts.get("broken").unwrap();
    assert_eq!(repaired.body, "Repaired $1.");
    assert!(repaired.problem.is_none());
}

#[tokio::test]
async fn valid_saves_navigate_and_stale_fingerprints_return_conflicts() {
    let (state, cookie) = fixture();
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("name", "review")
        .append_pair("description", "Review a change.")
        .append_pair("argument_hint", "<path>")
        .append_pair("body", "Review $1.")
        .finish();
    let response = app(&state)
        .oneshot(request(
            "POST",
            "/prompts/save",
            &cookie,
            Some("patch"),
            body,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(
        std::str::from_utf8(&body)
            .unwrap()
            .contains("navigate=\"/prompts?edit=review\"")
    );
    let record = state.prompts.get("review").unwrap();
    assert_eq!(record.description, "Review a change.");
    assert_eq!(record.argument_hint, "<path>");

    // A stale fingerprint never replaces the stored file.
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("name", "review")
        .append_pair("fingerprint", "stale")
        .append_pair("body", "Changed.")
        .finish();
    let response = app(&state)
        .oneshot(request(
            "POST",
            "/prompts/save",
            &cookie,
            Some("patch"),
            body,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(state.prompts.get("review").unwrap(), record);

    // A duplicate create keeps the first file.
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("name", "review")
        .append_pair("body", "Different.")
        .finish();
    let response = app(&state)
        .oneshot(request(
            "POST",
            "/prompts/save",
            &cookie,
            Some("patch"),
            body,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(state.prompts.get("review").unwrap(), record);
}
