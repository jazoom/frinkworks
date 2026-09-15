use askama::Template;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    middleware::from_fn_with_state,
};
use tower::ServiceExt;

#[tokio::test]
async fn decisions_retain_gate_and_conversation_identity_without_commands() {
    let (state, token, _, child) = crate::slices::human_gates::tests::conversation_at_gate();
    let gate = child
        .gates
        .iter()
        .find(|gate| gate.state == crate::workflows::gates::HumanGateState::AwaitingDecision)
        .unwrap();
    let gate_href = format!("/runs/{}/gates/{}", child.id.as_hex(), gate.id.as_hex());
    let owner = format!("/conversations/{}", child.conversation_id.unwrap().as_hex());
    let app = crate::slices::router()
        .layer(from_fn_with_state(
            state.clone(),
            crate::sessions::resolve_session,
        ))
        .layer(axum::middleware::from_fn(hypergraft::middleware::classify))
        .with_state(state.clone());
    for kind in [None, Some("navigation"), Some("patch")] {
        let mut request = Request::builder()
            .uri("/attention?page=18446744073709551615")
            .header(header::COOKIE, format!("powerplant_session={token}"));
        if let Some(kind) = kind {
            request = request
                .header(hypergraft::GRAFT_REQUEST, kind)
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if kind == Some("patch") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::OK
            }
        );
        if kind != Some("patch") {
            let body = String::from_utf8(
                to_bytes(response.into_body(), 1024 * 1024)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            assert!(body.contains(&owner));
            assert!(body.contains(&gate_href));
            assert!(!body.contains("name=\"candidate\""));
        }
    }
    let response = app
        .oneshot(
            Request::builder()
                .uri("/runs")
                .header(header::COOKIE, format!("powerplant_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains(&owner));
    assert!(body.contains(&format!("data-run-id=\"{}\"", child.id.as_hex())));
    state
        .workflow_runs
        .mutate(&child.id, |run| {
            run.cancel(crate::workflows::run::now_ms())?;
            Ok(())
        })
        .unwrap();
    let body = super::page::AttentionPage::new(&state, 0, None)
        .render()
        .unwrap();
    assert!(!body.contains(&gate_href));
    let record = state
        .conversations
        .get(&child.conversation_id.unwrap())
        .unwrap();
    let job = record.active_job.unwrap();
    let session = crate::sessions::generate_session_token().unwrap().id();
    state
        .host_approvals
        .submit(crate::execution::HostCommandRequest {
            token: String::new(),
            session,
            job,
            conversation: record.id,
            execution_revision: 1,
            command: "echo test".to_owned(),
            directory: std::path::PathBuf::from("/tmp"),
            explanation: "Fixture".to_owned(),
            run: Some(child.id.as_hex()),
            step: None,
            attempt: None,
        })
        .unwrap();
    let body = super::page::AttentionPage::new(&state, 0, None)
        .render()
        .unwrap();
    assert!(body.contains("Needs command approval"));
    assert!(body.contains(&owner));
    assert!(!body.contains("echo test"));
    state.host_approvals.invalidate_job(job);
    assert!(
        !super::page::AttentionPage::new(&state, 0, None)
            .render()
            .unwrap()
            .contains("Needs command approval")
    );
}

/// Context selects the return destination only. Forged, malformed or
/// deleted identifiers fall back to the catalogue without reflection.
#[tokio::test]
async fn contextual_attention_validates_identifiers_and_preserves_context() {
    let (state, token, _, child) = crate::slices::human_gates::tests::conversation_at_gate();
    let owner = format!("/conversations/{}", child.conversation_id.unwrap().as_hex());
    let gate = child
        .gates
        .iter()
        .find(|gate| gate.state == crate::workflows::gates::HumanGateState::AwaitingDecision)
        .unwrap();
    let gate_href = format!("/runs/{}/gates/{}", child.id.as_hex(), gate.id.as_hex());

    let contextual =
        super::page::AttentionPage::new(&state, 0, Some(child.conversation_id.unwrap()))
            .render()
            .unwrap();
    assert!(contextual.contains(">Back to conversation<"));
    assert!(contextual.contains(&format!("href=\"{owner}\"")));
    assert!(contextual.contains(&owner));
    assert!(contextual.contains(&gate_href));

    let plain = super::page::AttentionPage::new(&state, 0, None)
        .render()
        .unwrap();
    assert!(!plain.contains(">Back to conversation<"));
    assert!(plain.contains(&owner));
    assert!(plain.contains(&gate_href));

    let unknown =
        crate::conversations::ConversationId::parse("0123456789abcdef0123456789abcdef").unwrap();
    assert!(state.conversations.get(&unknown).is_none());
    let fallback = super::page::AttentionPage::new(&state, 0, Some(unknown))
        .render()
        .unwrap();
    assert!(!fallback.contains(">Back to conversation<"));
    assert!(fallback.contains(&gate_href));

    let app = crate::slices::router()
        .layer(from_fn_with_state(
            state.clone(),
            crate::sessions::resolve_session,
        ))
        .layer(axum::middleware::from_fn(hypergraft::middleware::classify))
        .with_state(state.clone());
    let cookie = format!("powerplant_session={token}");
    let conversation = child.conversation_id.unwrap().as_hex();
    for (uri, has_context) in [
        (format!("/attention?conversation={conversation}"), true),
        (
            "/attention?conversation=not-a-conversation-id".to_owned(),
            false,
        ),
        (
            "/attention?conversation=https%3A%2F%2Fexample.com%2Fevil".to_owned(),
            false,
        ),
        (
            "/attention?conversation=0123456789abcdef0123456789abcdef".to_owned(),
            false,
        ),
    ] {
        for kind in [None, Some("navigation")] {
            let mut request = Request::builder().uri(&uri).header(header::COOKIE, &cookie);
            if let Some(kind) = kind {
                request = request
                    .header(hypergraft::GRAFT_REQUEST, kind)
                    .header(header::ACCEPT, hypergraft::MEDIA_TYPE);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = String::from_utf8(
                to_bytes(response.into_body(), 1024 * 1024)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            assert_eq!(
                body.contains(">Back to conversation<"),
                has_context,
                "{uri}"
            );
            assert!(body.contains(&owner), "{uri}");
            assert!(body.contains(&gate_href), "{uri}");
            assert!(!body.contains("example.com/evil"), "{uri}");
        }
    }
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/attention?page=0&conversation={conversation}"))
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(!body.contains("/attention?conversation="));
    assert!(body.contains(&owner));
}

/// The shared suffix keeps refresh and decision pages on the validated
/// conversation without touching return-path construction elsewhere.
#[test]
fn attention_context_suffix_preserves_valid_context_only() {
    let id = crate::conversations::ConversationId::parse(&"a".repeat(32)).expect("identifier");
    assert_eq!(
        super::page::context_suffix(Some(id)),
        format!("&conversation={}", id.as_hex())
    );
    assert!(super::page::context_suffix(None).is_empty());
}
