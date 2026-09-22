mod page;

#[cfg(test)]
mod tests;

pub(super) use page::QueueView;

use axum::{
    Form,
    extract::{Path, State},
    response::Response,
};
use hypergraft::{PatchGraft, PatchStatus};
use serde::Deserialize;

use crate::{
    conversations::{ConversationError, QueueDelivery, QueueItemId},
    error::AppResult,
    sessions::RequiredSession,
    state::AppState,
};

use super::{
    REVISION_MESSAGE, detail_view, load_conversation, parse_revision, render_detail_command,
    status_for,
};

#[derive(Deserialize)]
pub(super) struct EnqueueForm {
    queue_revision: String,
    message: String,
    #[serde(default)]
    delivery: String,
    #[serde(default, rename = "attachment_0")]
    attachment_0: String,
    #[serde(default, rename = "attachment_1")]
    attachment_1: String,
    #[serde(default, rename = "attachment_2")]
    attachment_2: String,
    #[serde(default, rename = "attachment_3")]
    attachment_3: String,
    #[serde(default, rename = "attachment_4")]
    attachment_4: String,
    #[serde(default, rename = "attachment_5")]
    attachment_5: String,
    #[serde(default, rename = "attachment_6")]
    attachment_6: String,
    #[serde(default, rename = "attachment_7")]
    attachment_7: String,
}

impl EnqueueForm {
    fn attachment_ids(&self) -> Result<Vec<crate::conversations::AttachmentId>, &'static str> {
        let mut ids = Vec::new();
        for value in [
            &self.attachment_0,
            &self.attachment_1,
            &self.attachment_2,
            &self.attachment_3,
            &self.attachment_4,
            &self.attachment_5,
            &self.attachment_6,
            &self.attachment_7,
        ] {
            if value.trim().is_empty() {
                continue;
            }
            let id = crate::conversations::attachments::parse_attachment_id(value)
                .ok_or("That staged image is not valid. Add it again.")?;
            if ids.contains(&id) {
                return Err("That staged image is not valid. Add it again.");
            }
            ids.push(id);
        }
        Ok(ids)
    }
}

#[derive(Deserialize)]
pub(super) struct QueueItemForm {
    queue_revision: String,
    #[serde(default)]
    editor: String,
}

pub(super) async fn enqueue(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path(conversation_id): Path<String>,
    Form(form): Form<EnqueueForm>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(crate::responses::command_navigation("/conversations"));
    };
    let Some(queue_revision) = parse_revision(&form.queue_revision) else {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(&state, session.0, &record, &record.title, REVISION_MESSAGE),
        );
    };
    if let Err(error) = bind_session(&state, session.0, &record) {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(&state, session.0, &record, &record.title, error),
        );
    }
    let delivery = match form.delivery.trim() {
        "" | "follow-up" => QueueDelivery::FollowUp,
        "steering" => QueueDelivery::Steering,
        _ => {
            return render_detail_command(
                graft,
                PatchStatus::UnprocessableEntity,
                detail_view(
                    &state,
                    session.0,
                    &record,
                    &record.title,
                    "Choose follow-up or steering for this queued message.",
                ),
            );
        }
    };
    let job = match delivery {
        QueueDelivery::Steering => record.active_job,
        QueueDelivery::FollowUp => None,
    };
    if delivery == QueueDelivery::Steering && job.is_none() {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                "Steering needs an active request. Queue it as a follow-up instead.",
            ),
        );
    }
    let attachments = match form.attachment_ids() {
        Ok(attachments) => attachments,
        Err(error) => {
            return render_detail_command(
                graft,
                PatchStatus::UnprocessableEntity,
                detail_view(&state, session.0, &record, &record.title, error),
            );
        }
    };
    let scope = record.id.as_hex();
    let queued = if attachments.is_empty() {
        state
            .conversations
            .enqueue(&record.id, queue_revision, form.message, delivery, job)
    } else {
        state.conversations.enqueue_with_attachments(
            &record.id,
            queue_revision,
            form.message,
            Some(session.0),
            &scope,
            attachments,
            delivery,
            job,
        )
    };
    match queued {
        Ok(updated) => render_detail_command(
            graft,
            PatchStatus::Ok,
            detail_view(&state, session.0, &updated, &updated.title, ""),
        ),
        Err(error) => render_detail_command(
            graft,
            status_for(error),
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                queue_error(error),
            ),
        ),
    }
}

pub(super) async fn remove(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path((conversation_id, item_id)): Path<(String, String)>,
    Form(form): Form<QueueItemForm>,
) -> AppResult<Response> {
    mutate_item(
        state,
        session,
        graft,
        conversation_id,
        item_id,
        form,
        ItemAction::Remove,
    )
    .await
}

pub(super) async fn return_to_editor(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path((conversation_id, item_id)): Path<(String, String)>,
    Form(form): Form<QueueItemForm>,
) -> AppResult<Response> {
    mutate_item(
        state,
        session,
        graft,
        conversation_id,
        item_id,
        form,
        ItemAction::ReturnToEditor,
    )
    .await
}

enum ItemAction {
    Remove,
    ReturnToEditor,
}

async fn mutate_item(
    state: AppState,
    session: RequiredSession,
    graft: PatchGraft,
    conversation_id: String,
    item_id: String,
    form: QueueItemForm,
    action: ItemAction,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(crate::responses::command_navigation("/conversations"));
    };
    let Some(queue_revision) = parse_revision(&form.queue_revision) else {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(&state, session.0, &record, &record.title, REVISION_MESSAGE),
        );
    };
    let Some(item_id) = QueueItemId::parse(&item_id) else {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                "That queued message is no longer available.",
            ),
        );
    };
    if let Err(error) = bind_session(&state, session.0, &record) {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(&state, session.0, &record, &record.title, error),
        );
    }
    if matches!(action, ItemAction::ReturnToEditor)
        && !record
            .queue
            .items
            .iter()
            .any(|item| item.id == item_id && item.text == form.editor)
    {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(&state, session.0, &record, &record.title, "").with_queue_return(
                item_id,
                "The editor already has text. Confirm replace to use this queued message.",
            ),
        );
    }
    let result = match action {
        ItemAction::ReturnToEditor => {
            state
                .conversations
                .return_queue_item(&record.id, queue_revision, item_id, session.0)
        }
        ItemAction::Remove => state
            .conversations
            .remove_queue_item(&record.id, queue_revision, item_id)
            .map(|(record, _)| record),
    };
    match result {
        Ok(updated) => render_detail_command(
            graft,
            PatchStatus::Ok,
            detail_view(&state, session.0, &updated, &updated.title, ""),
        ),
        Err(error) => render_detail_command(
            graft,
            status_for(error),
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                queue_error(error),
            ),
        ),
    }
}

fn bind_session(
    state: &AppState,
    session: crate::sessions::SessionId,
    record: &crate::conversations::ConversationRecord,
) -> Result<(), &'static str> {
    if record.active_job.is_none_or(|job| {
        state
            .sessions
            .owns_conversation_job(&session, record.id, job)
    }) {
        Ok(())
    } else {
        Err(
            "This conversation is running in another session. Queue changes stay with that session.",
        )
    }
}

fn queue_error(error: ConversationError) -> &'static str {
    match error {
        ConversationError::Full => {
            "This conversation already has the maximum number of queued messages."
        }
        ConversationError::Message => "Enter a queued message within the conversation limit.",
        ConversationError::Conflict => "That queued message is no longer available.",
        ConversationError::Active => {
            "This conversation is running in another session. Queue changes stay with that session."
        }
        ConversationError::Selection => "Choose an available model before you queue a message.",
        other => other.message(),
    }
}
