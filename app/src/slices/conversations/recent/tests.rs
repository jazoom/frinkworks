use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use tower::ServiceExt;

use crate::slices::conversations::tests::{
    app, awaiting_gate, connected, document, navigation, test_state, text,
};

#[tokio::test]
async fn recent_projection_is_bounded_escaped_and_uses_the_canonical_catalogue_route() {
    let state = test_state();
    let token = connected(&state);
    for _ in 0..13 {
        state
            .conversations
            .create_saved(
                crate::conversations::ConversationId::generate().expect("id"),
                Some("<script>title</script>".to_owned()),
                None,
                vec![],
            )
            .expect("record");
    }
    let request = Request::builder()
        .uri("/conversations?index=true")
        .header(header::COOKIE, format!("frinkworks_session={token}"))
        .header("Graft-Request", "patch")
        .header(header::ACCEPT, "text/vnd.hypergraft.patches+html")
        .body(Body::empty())
        .expect("request");
    let response = app(&state).oneshot(request).await.expect("projection");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("target=\"recent-conversations\""));
    assert!(body.contains("target=\"attention-count\""));
    assert_eq!(body.matches("data-recent-conversation").count(), 12);
    assert!(!body.contains("<script>title</script>"));
    for request in [
        document("/conversations?index=true", &token),
        navigation("/conversations?index=true", &token),
    ] {
        let response = app(&state).oneshot(request).await.expect("canonical page");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            text(response)
                .await
                .contains("data-section=\"conversations\"")
        );
    }
}

#[tokio::test]
async fn sidebar_projection_counts_the_authoritative_gate() {
    let state = test_state();
    let token = connected(&state);
    awaiting_gate(&state);
    let request = Request::builder()
        .uri("/conversations?index=true")
        .header(header::COOKIE, format!("frinkworks_session={token}"))
        .header("Graft-Request", "patch")
        .header(header::ACCEPT, "text/vnd.hypergraft.patches+html")
        .body(Body::empty())
        .expect("request");
    let response = app(&state).oneshot(request).await.expect("projection");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("target=\"attention-count\""));
    assert!(body.contains(">1</template>"));
    let response = app(&state)
        .oneshot(document("/conversations", &token))
        .await
        .expect("catalogue");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(text(response).await.contains("id=\"attention-count\""));
    assert_eq!(state.workflow_runs.active_runs().len(), 1);
}
