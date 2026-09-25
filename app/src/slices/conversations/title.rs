#[cfg(test)]
mod tests;

use askama::Template;
use axum::extract::{Path, State};
use hypergraft::{
    PatchSet, PatchStatus,
    live::{LiveProjection, LiveReject, LiveRouter, ProjectionError},
};

use crate::{
    conversations::{ConversationId, ConversationRecord},
    error::{AppError, AppResult},
    sessions::SessionId,
    state::AppState,
};

#[derive(Template)]
#[template(
    path = "conversations/templates/detail.html",
    block = "conversation_heading"
)]
struct Title<'a> {
    heading: &'a str,
}

fn patches(record: &ConversationRecord) -> Result<PatchSet, hypergraft::PatchBuildError> {
    PatchSet::new().with_children(
        "conversation-heading",
        &Title {
            heading: &record.title,
        },
    )
}

pub(super) fn response(record: &ConversationRecord) -> AppResult<axum::response::Response> {
    patches(record)
        .and_then(|patches| patches.respond(PatchStatus::Ok))
        .map_err(|error| AppError::new("render conversation title", error))
}

pub(in crate::slices) fn live_router() -> LiveRouter<AppState> {
    LiveRouter::new()
        .route("/conversations", super::recent::live)
        .expect("unique recent projection")
        .route("/conversations/{conversation_id}", live)
        .expect("unique title projection")
}

async fn live(
    State(state): State<AppState>,
    Path(raw): Path<String>,
) -> Result<LiveProjection<SessionId>, LiveReject> {
    let updates = state.conversations.subscribe_titles();
    let id = ConversationId::parse(&raw).ok_or(LiveReject::Invalid)?;
    if state.conversations.get(&id).is_none() {
        return Err(LiveReject::Retire);
    }
    Ok(LiveProjection::new(
        hypergraft::live::broadcast_invalidations(updates),
        move |_| {
            let state = state.clone();
            async move {
                let record = state
                    .conversations
                    .get(&id)
                    .ok_or(ProjectionError::Retire)?;
                patches(&record).map_err(|_| ProjectionError::Retire)
            }
        },
    ))
}
