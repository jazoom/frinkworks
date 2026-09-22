//! Add-image controls for the conversation composer.
//!
//! Uploads stage session-owned images and never create a conversation. A saved
//! conversation uses its identifier as the staging scope, so a draft and a
//! saved transcript cannot share staged references. The fragment patch touches
//! only the two attachment containers, so the composer keeps its draft text and
//! settings across an upload or a removal.

pub(super) mod page;

#[cfg(test)]
mod tests;

pub(super) use page::AttachmentsView;

use axum::{
    extract::{Multipart, Path, Query, State},
    response::Response,
};
use hypergraft::{PatchGraft, PatchStatus};

use crate::conversations::attachments::{
    AttachmentError, AttachmentId, MAXIMUM_ATTACHMENT_BYTES, MAXIMUM_ATTACHMENTS_PER_MESSAGE,
    MAXIMUM_MESSAGE_ATTACHMENT_BYTES,
};
use crate::error::AppResult;
use crate::sessions::RequiredSession;
use crate::state::AppState;

fn load_conversation(
    state: &AppState,
    value: &str,
) -> Option<crate::conversations::ConversationMetadata> {
    state
        .conversations
        .metadata_for(&crate::conversations::ConversationId::parse(value)?)
}
use page::{ControlsTemplate, RefsTemplate};

pub(super) const UPLOAD_BODY_LIMIT: usize =
    MAXIMUM_ATTACHMENT_BYTES * MAXIMUM_ATTACHMENTS_PER_MESSAGE + 1024 * 1024;

#[derive(serde::Deserialize)]
pub(super) struct DraftQuery {
    draft: String,
}

impl DraftQuery {
    fn scope(&self) -> Option<String> {
        (self.draft.len() == 64 && self.draft.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| page::draft_scope(&self.draft))
    }
}

pub(super) async fn upload_new(
    State(state): State<AppState>,
    session: RequiredSession,
    _graft: PatchGraft,
    Query(draft): Query<DraftQuery>,
    multipart: Multipart,
) -> AppResult<Response> {
    let Some(scope) = draft.scope() else {
        return Ok(crate::responses::no_store_status_response(
            axum::http::StatusCode::BAD_REQUEST,
            "Invalid draft",
        ));
    };
    upload(state, session, scope, multipart).await
}

pub(super) async fn upload_saved(
    State(state): State<AppState>,
    session: RequiredSession,
    _graft: PatchGraft,
    Path(conversation_id): Path<String>,
    multipart: Multipart,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(crate::responses::command_navigation("/conversations"));
    };
    upload(state, session, record.id.as_hex(), multipart).await
}

async fn upload(
    state: AppState,
    session: RequiredSession,
    scope: String,
    mut multipart: Multipart,
) -> AppResult<Response> {
    let store = state.conversations.attachment_store();
    let mut failure = None;
    loop {
        let mut field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(_) => {
                failure = Some(AttachmentError::Malformed);
                break;
            }
        };
        if field.name() != Some("image") {
            continue;
        }
        let mut bytes = Vec::new();
        let read_error = loop {
            match field.chunk().await {
                Ok(Some(chunk))
                    if bytes.len().saturating_add(chunk.len()) <= MAXIMUM_ATTACHMENT_BYTES =>
                {
                    bytes.extend_from_slice(&chunk)
                }
                Ok(Some(_)) => break Some(AttachmentError::TooLarge),
                Ok(None) => break None,
                Err(_) => break Some(AttachmentError::Malformed),
            }
        };
        if let Some(error) = read_error {
            failure = Some(error);
            break;
        }
        let staged = match state.conversations.staged_attachments(session.0, &scope) {
            Ok(staged) => staged,
            Err(_) => {
                failure = Some(AttachmentError::Persist);
                break;
            }
        };
        if staged.len() >= MAXIMUM_ATTACHMENTS_PER_MESSAGE {
            failure = Some(AttachmentError::Count);
            continue;
        }
        let retained: u64 = staged.iter().map(|reference| reference.byte_length).sum();
        let image = match store.normalise(&bytes) {
            Ok(image) => image,
            Err(error) => {
                failure = Some(error);
                continue;
            }
        };
        if retained.saturating_add(image.bytes.len() as u64) > MAXIMUM_MESSAGE_ATTACHMENT_BYTES {
            failure = Some(AttachmentError::Aggregate);
            continue;
        }
        let sha256 = image.sha256();
        let reference = crate::conversations::AttachmentRef {
            id: AttachmentId::generate().map_err(|error| {
                crate::error::AppError::new("create attachment identifier", error)
            })?,
            sha256,
            format: image.format,
            width: image.width,
            height: image.height,
            byte_length: image.bytes.len() as u64,
        };
        if let Err(error) =
            state
                .conversations
                .stage_attachment(session.0, &scope, reference, &image.bytes)
        {
            failure = Some(match error {
                crate::conversations::ConversationError::Image(error) => error,
                _ => AttachmentError::Persist,
            });
        }
    }
    let mut view = page::view(&state, session.0, &scope);
    if let Some(error) = failure {
        view = view.with_error(error);
    }
    render_attachments(view)
}

pub(super) async fn remove_new(
    State(state): State<AppState>,
    session: RequiredSession,
    _graft: PatchGraft,
    Path(attachment_id): Path<String>,
    Query(draft): Query<DraftQuery>,
) -> AppResult<Response> {
    let Some(scope) = draft.scope() else {
        return Ok(crate::responses::no_store_status_response(
            axum::http::StatusCode::BAD_REQUEST,
            "Invalid draft",
        ));
    };
    remove(state, session, scope, attachment_id).await
}

pub(super) async fn remove_saved(
    State(state): State<AppState>,
    session: RequiredSession,
    _graft: PatchGraft,
    Path((conversation_id, attachment_id)): Path<(String, String)>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(crate::responses::command_navigation("/conversations"));
    };
    remove(state, session, record.id.as_hex(), attachment_id).await
}

async fn remove(
    state: AppState,
    session: RequiredSession,
    scope: String,
    attachment_id: String,
) -> AppResult<Response> {
    let mut view = page::view(&state, session.0, &scope);
    let Some(id) = AttachmentId::parse(attachment_id.trim()) else {
        view = view.with_message("That image is no longer staged. Add it again.");
        return render_attachments(view);
    };
    if let Err(error) = state
        .conversations
        .release_attachment(session.0, &scope, id)
    {
        view = view.with_error(match error {
            crate::conversations::ConversationError::Image(error) => error,
            _ => AttachmentError::Persist,
        });
        return render_attachments(view);
    }
    view = page::view(&state, session.0, &scope);
    render_attachments(view)
}

/// Serve one staged image for the composer preview. The route is
/// session-scoped, so a foreign session cannot read another draft's staging.
pub(super) async fn serve_new(
    State(state): State<AppState>,
    session: RequiredSession,
    Path(attachment_id): Path<String>,
    Query(draft): Query<DraftQuery>,
) -> AppResult<Response> {
    let Some(scope) = draft.scope() else {
        return Ok(crate::responses::no_store_status_response(
            axum::http::StatusCode::BAD_REQUEST,
            "Invalid draft",
        ));
    };
    serve(state, session, scope, attachment_id).await
}

/// Serve one retained image for a saved conversation. The reference must
/// belong to that conversation, so the route never exposes another path.
pub(super) async fn serve_saved(
    State(state): State<AppState>,
    session: RequiredSession,
    Path((conversation_id, attachment_id)): Path<(String, String)>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(crate::responses::no_store_status_response(
            axum::http::StatusCode::NOT_FOUND,
            "Not found",
        ));
    };
    serve(state, session, record.id.as_hex(), attachment_id).await
}

async fn serve(
    state: AppState,
    session: RequiredSession,
    scope: String,
    attachment_id: String,
) -> AppResult<Response> {
    let Some(id) = AttachmentId::parse(attachment_id.trim()) else {
        return Ok(crate::responses::no_store_status_response(
            axum::http::StatusCode::NOT_FOUND,
            "Not found",
        ));
    };
    let loaded = if scope.starts_with("draft-") {
        state
            .conversations
            .load_staged_attachment(session.0, &scope, id)
    } else {
        state
            .conversations
            .load_attachment(&record_id(&scope), id)
            .or_else(|_| {
                state
                    .conversations
                    .load_staged_attachment(session.0, &scope, id)
            })
    };
    let Ok((reference, bytes)) = loaded else {
        return Ok(crate::responses::no_store_status_response(
            axum::http::StatusCode::NOT_FOUND,
            "Not found",
        ));
    };
    let mut response = Response::new(axum::body::Body::from(bytes));
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static(reference.format.media_type()),
    );
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        axum::http::HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

fn record_id(scope: &str) -> crate::conversations::ConversationId {
    crate::conversations::ConversationId::parse(scope)
        .expect("the saved scope is a validated conversation identifier")
}

fn render_attachments(view: AttachmentsView) -> AppResult<Response> {
    let controls = ControlsTemplate { view: &view };
    let refs = RefsTemplate { view: &view };
    let mut patches = hypergraft::PatchSet::new();
    patches.children("conversation-attachment-controls", &controls)?;
    patches.children("conversation-attachment-refs", &refs)?;
    Ok(patches.respond(PatchStatus::Ok)?)
}
