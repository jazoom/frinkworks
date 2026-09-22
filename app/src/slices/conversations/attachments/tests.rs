use std::io::Cursor;

use axum::{
    body::{Body, to_bytes},
    http::{Request, header},
};
use tower::ServiceExt;

use super::super::tests::{app, connected, test_state};
use crate::{
    conversations::{AttachmentRef, attachments::AttachmentId},
    sessions::JobId,
    state::AppState,
};

const DRAFT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SCOPE: &str = "draft-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn stage(state: &AppState, session: crate::sessions::SessionId, scope: &str) -> AttachmentId {
    let mut bytes = Vec::new();
    image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]))
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .expect("encode");
    let image = state
        .conversations
        .attachment_store()
        .normalise(&bytes)
        .expect("normalise");
    let reference = AttachmentRef {
        id: AttachmentId::generate().expect("id"),
        sha256: image.sha256(),
        format: image.format,
        width: image.width,
        height: image.height,
        byte_length: image.bytes.len() as u64,
    };
    state
        .conversations
        .stage_attachment(session, scope, reference.clone(), &image.bytes)
        .expect("stage");
    reference.id
}

fn conversation(state: &AppState) -> crate::conversations::ConversationRecord {
    let selection = crate::providers::ModelSelection::new(
        crate::providers::ProviderKind::Xai,
        "grok-4.6".to_owned(),
        None,
    )
    .expect("model");
    state
        .conversations
        .create_saved(
            crate::conversations::ConversationId::generate().expect("id"),
            None,
            Some(
                crate::conversations::ConversationModelConfiguration::direct(
                    selection,
                    crate::tests::test_environment_id(),
                ),
            ),
            Vec::new(),
        )
        .expect("create")
}

#[tokio::test]
async fn upload_rejects_svg_without_creating_a_conversation() {
    let state = test_state();
    let token = connected(&state);
    let request = Request::builder().method("POST")
        .uri(format!("/conversations/new/attachments?draft={DRAFT}"))
        .header(header::COOKIE, format!("powerplant_session={token}"))
        .header(header::CONTENT_TYPE, "multipart/form-data; boundary=image-test")
        .header(hypergraft::GRAFT_REQUEST, "patch").header(header::ACCEPT, hypergraft::MEDIA_TYPE)
        .body(Body::from("--image-test\r\nContent-Disposition: form-data; name=\"image\"; filename=\"x.png\"\r\nContent-Type: image/png\r\n\r\n<svg></svg>\r\n--image-test--\r\n")).expect("request");
    let response = app(&state).oneshot(request).await.expect("upload");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body")
            .to_vec(),
    )
    .expect("text");
    assert!(body.contains("conversation-attachment-controls"));
    assert!(body.contains("Use a PNG, JPEG or WebP image."));
    assert!(state.conversations.metadata().is_empty());
}

#[test]
fn foreign_claims_and_duplicate_claims_leave_the_message_unchanged() {
    let state = test_state();
    let owner = crate::sessions::generate_session_token()
        .expect("session")
        .id();
    let foreign = crate::sessions::generate_session_token()
        .expect("session")
        .id();
    let id = stage(&state, owner, SCOPE);
    let record = conversation(&state);
    for (session, scope, ids) in [
        (foreign, SCOPE, vec![id]),
        (owner, "other-draft", vec![id]),
        (owner, SCOPE, vec![id, id]),
    ] {
        assert!(
            state
                .conversations
                .begin_message_with_attachments(
                    &record.id,
                    record.revision,
                    None,
                    JobId::generate().expect("job"),
                    String::new(),
                    Some(session),
                    scope,
                    ids
                )
                .is_err()
        );
        assert!(
            state
                .conversations
                .get(&record.id)
                .expect("record")
                .messages
                .is_empty()
        );
    }
    let started = state
        .conversations
        .begin_message_with_attachments(
            &record.id,
            record.revision,
            None,
            JobId::generate().expect("job"),
            String::new(),
            Some(owner),
            SCOPE,
            vec![id],
        )
        .expect("claim");
    assert_eq!(started.messages[0].attachments[0].id, id);
    let other = conversation(&state);
    assert!(
        state
            .conversations
            .begin_message_with_attachments(
                &other.id,
                other.revision,
                None,
                JobId::generate().expect("job"),
                String::new(),
                Some(owner),
                SCOPE,
                vec![id]
            )
            .is_err()
    );
    assert!(
        state
            .conversations
            .get(&other.id)
            .expect("record")
            .messages
            .is_empty()
    );
    assert!(state.conversations.load_attachment(&other.id, id).is_err());
    assert!(state.conversations.load_attachment(&record.id, id).is_ok());
}

#[test]
fn queue_return_preserves_bytes_and_allows_a_new_atomic_claim() {
    let state = test_state();
    let session = crate::sessions::generate_session_token()
        .expect("session")
        .id();
    let record = conversation(&state);
    let scope = record.id.as_hex();
    let id = stage(&state, session, &scope);
    let queued = state
        .conversations
        .enqueue_with_attachments(
            &record.id,
            record.queue.revision,
            String::new(),
            Some(session),
            &scope,
            vec![id],
            crate::conversations::QueueDelivery::FollowUp,
            None,
        )
        .expect("queue");
    let returned = state
        .conversations
        .return_queue_item(
            &record.id,
            queued.queue.revision,
            queued.queue.items[0].id,
            session,
        )
        .expect("return");
    assert!(returned.queue.items.is_empty());
    assert!(
        state
            .conversations
            .load_staged_attachment(session, &scope, id)
            .is_ok()
    );
    let started = state
        .conversations
        .begin_message_with_attachments(
            &record.id,
            returned.revision,
            None,
            JobId::generate().expect("job"),
            String::new(),
            Some(session),
            &scope,
            vec![id],
        )
        .expect("send");
    assert_eq!(started.messages[0].attachments[0].id, id);
}

#[tokio::test]
async fn staged_previews_require_the_owner_and_scope() {
    let state = test_state();
    let owner = connected(&state);
    let foreign = connected(&state);
    let session = crate::sessions::SessionId::from_validated(
        &crate::sessions::ValidatedToken::parse(&owner).expect("token"),
    );
    let record = conversation(&state);
    let id = stage(&state, session, &record.id.as_hex());
    let path = format!("/conversations/{}/attachments/{id}", record.id);
    for (cookie, status) in [
        (&owner, axum::http::StatusCode::OK),
        (&foreign, axum::http::StatusCode::NOT_FOUND),
    ] {
        let request = Request::builder()
            .uri(&path)
            .header(header::COOKIE, format!("powerplant_session={cookie}"))
            .body(Body::empty())
            .expect("request");
        let response = app(&state).oneshot(request).await.expect("preview");
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        if status.is_success() {
            assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
        }
    }
}

#[test]
fn a_fork_draft_keeps_images_after_source_deletion() {
    let state = test_state();
    let session = crate::sessions::generate_session_token()
        .expect("session")
        .id();
    let id = stage(&state, session, SCOPE);
    let record = conversation(&state);
    let job = JobId::generate().expect("job");
    state
        .conversations
        .begin_message_with_attachments(
            &record.id,
            record.revision,
            None,
            job,
            String::new(),
            Some(session),
            SCOPE,
            vec![id],
        )
        .expect("send");
    state
        .conversations
        .settle_message(
            &record.id,
            job,
            "Image response",
            crate::conversations::MessageStatus::Complete,
            None,
        )
        .expect("settle");
    let source = state.conversations.get(&record.id).expect("source");
    let mut snapshot =
        crate::conversations::forks::snapshot(&source, source.messages.last().expect("reply").id)
            .expect("snapshot");
    snapshot
        .retain_images(state.conversations.clone(), session, DRAFT)
        .expect("pin images");
    state
        .conversations
        .delete(&source.id, source.revision)
        .expect("delete source");
    let fork = state
        .conversations
        .create_fork(
            crate::conversations::ConversationId::generate().expect("id"),
            None,
            source.model,
            Vec::new(),
            snapshot.messages.clone(),
            &snapshot,
        )
        .expect("fork");
    drop(snapshot);
    let reference = &fork.messages[0].attachments[0];
    assert!(
        state
            .conversations
            .load_attachment(&fork.id, reference.id)
            .is_ok()
    );
    state
        .conversations
        .delete(&fork.id, fork.revision)
        .expect("delete fork");
    assert!(
        state
            .conversations
            .attachment_store()
            .load(reference)
            .is_err()
    );
}

#[test]
fn concurrent_claims_commit_exactly_one_message() {
    let state = test_state();
    let session = crate::sessions::generate_session_token()
        .expect("session")
        .id();
    let id = stage(&state, session, SCOPE);
    let first = conversation(&state);
    let second = conversation(&state);
    let barrier = std::sync::Barrier::new(2);
    let claim = |record: &crate::conversations::ConversationRecord| {
        barrier.wait();
        state.conversations.begin_message_with_attachments(
            &record.id,
            record.revision,
            None,
            JobId::generate().expect("job"),
            String::new(),
            Some(session),
            SCOPE,
            vec![id],
        )
    };
    std::thread::scope(|scope| {
        let first_result = scope.spawn(|| claim(&first));
        let second_result = scope.spawn(|| claim(&second));
        assert_ne!(
            first_result.join().expect("thread").is_ok(),
            second_result.join().expect("thread").is_ok()
        );
    });
    assert_eq!(
        state
            .conversations
            .get(&first.id)
            .expect("first")
            .messages
            .len()
            + state
                .conversations
                .get(&second.id)
                .expect("second")
                .messages
                .len(),
        2
    );
}

#[test]
fn foreign_release_cannot_delete_a_shared_object() {
    let state = test_state();
    let owner = crate::sessions::generate_session_token()
        .expect("session")
        .id();
    let foreign = crate::sessions::generate_session_token()
        .expect("session")
        .id();
    let first = stage(&state, owner, SCOPE);
    let second = stage(&state, owner, SCOPE);
    assert!(
        state
            .conversations
            .release_attachment(foreign, SCOPE, first)
            .is_err()
    );
    state
        .conversations
        .release_attachment(owner, SCOPE, first)
        .expect("remove");
    assert!(
        state
            .conversations
            .load_staged_attachment(owner, SCOPE, second)
            .is_ok()
    );
}
