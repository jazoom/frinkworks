use axum::http::StatusCode;
use tower::ServiceExt;

use super::super::tests::{app, connected, document, navigation, session_id, test_state, text};
use crate::conversations::{MessageStatus, RequestId, RequestUsage};
use crate::execution::{ResourceKind, ResourceSource};
use crate::providers::{AssistantReply, AuthMethod, ModelSelection, ModelUsage, ProviderKind};

fn request_with_sources() -> (RequestUsage, String) {
    let source = ResourceSource::new(
        ResourceKind::Instruction,
        "project",
        "/project/AGENTS.md",
        b"Use the project test command.",
    );
    let hash = source.content_hash.clone();
    let request = RequestUsage {
        id: RequestId::generate().expect("request"),
        usage: ModelUsage::new(ProviderKind::Xai, "grok-4.6"),
        auth: AuthMethod::ApiKey,
        prices: None,
        sources: vec![source],
        advertised: Vec::new(),
    };
    (request, hash)
}

#[tokio::test]
async fn context_view_reports_the_sources_of_one_request() {
    let state = test_state();
    let token = connected(&state);
    let owner = session_id(&token);
    let record = state
        .conversations
        .create("Context".to_owned())
        .expect("record");
    let job = state
        .sessions
        .begin_conversation_job(&owner, record.id)
        .expect("job");
    let selection =
        ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None).expect("model");
    state
        .conversations
        .begin_message(
            &record.id,
            record.revision,
            selection,
            job.id(),
            "Question".to_owned(),
        )
        .expect("begin");
    let (request, hash) = request_with_sources();
    let request_id = request.id.as_hex();
    let mut reply = AssistantReply::default();
    reply.usage.push(request);
    state
        .conversations
        .settle_message(&record.id, job.id(), reply, MessageStatus::Complete, None)
        .expect("settle");
    let path = format!("/conversations/{}/context/{request_id}", record.id);
    let conversation = format!("/conversations/{}", record.id);
    for request in [
        document(&conversation, &token),
        navigation(&conversation, &token),
    ] {
        let response = app(&state).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = text(response).await;
        let section = body
            .split("id=\"instruction-sources-heading\"")
            .nth(1)
            .unwrap();
        let section = section.split("</section>").next().unwrap();
        assert!(section.contains("/project/AGENTS.md"));
        assert!(section.contains(&path));
        assert!(!section.contains("Authorised root"));
    }

    for request in [document(&path, &token), navigation(&path, &token)] {
        let response = app(&state).oneshot(request).await.expect("page");
        assert_eq!(response.status(), StatusCode::OK);
        let body = text(response).await;
        assert!(body.contains("/project/AGENTS.md"));
        assert!(body.contains(&hash));
    }
}

#[tokio::test]
async fn context_view_rejects_a_request_from_another_conversation() {
    let state = test_state();
    let token = connected(&state);
    let owner = session_id(&token);
    let record = state
        .conversations
        .create("Empty".to_owned())
        .expect("record");
    let job = state
        .sessions
        .begin_conversation_job(&owner, record.id)
        .expect("job");
    state
        .sessions
        .finish_conversation_job(&owner, record.id, job.id());
    let request_id = RequestId::generate().expect("request").as_hex();
    let path = format!("/conversations/{}/context/{request_id}", record.id);
    let response = app(&state)
        .oneshot(document(&path, &token))
        .await
        .expect("page");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("not part of this conversation"));
}
