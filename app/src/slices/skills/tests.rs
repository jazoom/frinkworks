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
                "/skills?new=1",
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
    let record = state
        .skills
        .create("---\nname: existing\ndescription: task\n---\nbody".to_owned())
        .unwrap();
    for path in ["/skills/save", "/skills/delete"] {
        for graft in [None, Some("navigation")] {
            let body = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("directory", &record.directory)
                .append_pair("fingerprint", &record.fingerprint)
                .finish();
            let response = app(&state)
                .oneshot(request("POST", path, &cookie, graft, body))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert_eq!(state.skills.get(&record.directory).unwrap(), record);
        }
        let disconnected = crate::tests::test_state(crate::config::RuntimeConfig::development());
        let response = app(&disconnected)
            .oneshot(request("POST", path, "", Some("patch"), String::new()))
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(
            std::str::from_utf8(&body)
                .unwrap()
                .contains("navigate=\"/connect\"")
        );
        assert_eq!(state.skills.get(&record.directory).unwrap(), record);
    }
}

#[tokio::test]
async fn invalid_markdown_is_escaped_and_retained_without_a_write() {
    let (state, cookie) = fixture();
    let markdown = "</textarea><script>alert('not markup')</script>";
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("markdown", markdown)
        .finish();
    let response = app(&state)
        .oneshot(request(
            "POST",
            "/skills/save",
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
    assert!(text.contains("target=\"chat-main\""));
    assert!(state.skills.list().unwrap().is_empty());
}

#[tokio::test]
async fn valid_commands_navigate_and_stale_fingerprints_return_conflicts() {
    let (state, cookie) = fixture();
    let markdown = "---\nname: new-skill\ndescription: A task\n---\n# Instructions\n";
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("markdown", markdown)
        .finish();
    let response = app(&state)
        .oneshot(request(
            "POST",
            "/skills/save",
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
            .contains("navigate=\"/skills?edit=new-skill\"")
    );
    let record = state.skills.get("new-skill").unwrap();
    for path in ["/skills/save", "/skills/delete"] {
        let mut body = url::form_urlencoded::Serializer::new(String::new());
        body.append_pair("directory", &record.directory)
            .append_pair("fingerprint", "stale");
        if path.ends_with("save") {
            body.append_pair("markdown", markdown);
        }
        let response = app(&state)
            .oneshot(request("POST", path, &cookie, Some("patch"), body.finish()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(state.skills.get("new-skill").unwrap(), record);
    }
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("directory", &record.directory)
        .append_pair("fingerprint", &record.fingerprint)
        .finish();
    let response = app(&state)
        .oneshot(request(
            "POST",
            "/skills/delete",
            &cookie,
            Some("patch"),
            body,
        ))
        .await
        .unwrap();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(
        std::str::from_utf8(&body)
            .unwrap()
            .contains("navigate=\"/skills\"")
    );
    assert!(state.skills.list().unwrap().is_empty());
}
