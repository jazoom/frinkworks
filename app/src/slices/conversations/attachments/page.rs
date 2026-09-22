use askama::Template;

use crate::conversations::AttachmentId;
use crate::conversations::attachments::AttachmentError;
use crate::state::AppState;

pub(crate) fn draft_scope(nonce: &str) -> String {
    format!("draft-{nonce}")
}

pub(crate) struct AttachmentsView {
    pub(crate) attachments: Vec<StagedAttachmentView>,
    pub(crate) upload_action: String,
    pub(crate) max: usize,
    pub(crate) error: String,
    pub(crate) has_room: bool,
}

impl AttachmentsView {
    pub(crate) fn empty(upload_action: String) -> Self {
        Self {
            attachments: Vec::new(),
            upload_action,
            max: crate::conversations::attachments::MAXIMUM_ATTACHMENTS_PER_MESSAGE,
            error: String::new(),
            has_room: true,
        }
    }

    pub(crate) fn with_error(mut self, error: AttachmentError) -> Self {
        self.error = error.message().to_owned();
        self
    }

    pub(crate) fn with_message(mut self, message: &str) -> Self {
        self.error = message.to_owned();
        self
    }
}

pub(crate) struct StagedAttachmentView {
    pub(crate) id: String,
    pub(crate) src: String,
    pub(crate) remove_action: String,
    pub(crate) label: String,
}

#[derive(Template)]
#[template(path = "conversations/attachments/templates/index.html")]
pub(crate) struct ControlsTemplate<'a> {
    pub(crate) view: &'a AttachmentsView,
}

#[derive(Template)]
#[template(path = "conversations/attachments/templates/refs.html")]
pub(crate) struct RefsTemplate<'a> {
    pub(crate) view: &'a AttachmentsView,
}

pub(crate) fn upload_action(scope: &str) -> String {
    if let Some(nonce) = scope.strip_prefix("draft-") {
        format!("/conversations/new/attachments?draft={nonce}")
    } else {
        format!("/conversations/{scope}/attachments")
    }
}

pub(crate) fn remove_action(scope: &str, id: AttachmentId) -> String {
    if let Some(nonce) = scope.strip_prefix("draft-") {
        format!(
            "/conversations/new/attachments/{}/remove?draft={nonce}",
            id.as_hex()
        )
    } else {
        format!("/conversations/{scope}/attachments/{}/remove", id.as_hex())
    }
}

pub(crate) fn image_source(scope: &str, id: AttachmentId) -> String {
    if let Some(nonce) = scope.strip_prefix("draft-") {
        format!(
            "/conversations/new/attachments/{}?draft={nonce}",
            id.as_hex()
        )
    } else {
        format!("/conversations/{scope}/attachments/{}", id.as_hex())
    }
}

pub(crate) fn view(
    state: &AppState,
    session: crate::sessions::SessionId,
    scope: &str,
) -> AttachmentsView {
    let mut view = AttachmentsView::empty(upload_action(scope));
    let references = match state.conversations.staged_attachments(session, scope) {
        Ok(references) => references,
        Err(error) => {
            view.error = error.message().to_owned();
            view.has_room = false;
            return view;
        }
    };
    view.attachments = references
        .iter()
        .map(|reference| StagedAttachmentView {
            id: reference.id.as_hex(),
            src: image_source(scope, reference.id),
            remove_action: remove_action(scope, reference.id),
            label: format!("{} by {} image", reference.width, reference.height),
        })
        .collect();
    view.has_room = view.attachments.len() < view.max;
    view
}
