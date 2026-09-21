use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    response::Response,
};
use hypergraft::{GraftRequest, PageGraft, PatchGraft, PatchStatus};
use serde::Deserialize;

use crate::{
    conversations::forks::ForkError,
    error::{AppError, AppResult},
    sessions::RequiredSession,
    state::AppState,
};

use super::super::page::ConversationDetailView;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ForkQuery {
    message: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForkForm {
    revision: String,
    message: String,
}

#[derive(Template)]
#[template(path = "conversations/forks/templates/index.html")]
pub(super) struct ForkView {
    pub(super) heading: String,
    pub(super) source_href: String,
    pub(super) excerpt: String,
    pub(super) entries: usize,
    pub(super) revision: String,
    pub(super) message: String,
    pub(super) candidate_review: bool,
    pub(super) can_fork: bool,
    pub(super) error: String,
}

pub(crate) async fn show(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: PageGraft,
    Path(conversation_id): Path<String>,
    Query(query): Query<ForkQuery>,
) -> AppResult<Response> {
    let Some(record) = super::super::load_conversation(&state, &conversation_id) else {
        return Ok(crate::responses::request_navigation(
            graft,
            "/conversations",
        ));
    };
    let view = match view_for(&record, query.message.trim()) {
        Ok(view) => view,
        Err(error) => error_view(&record, query.message.trim(), error.message()),
    };
    super::super::render_page(
        &state,
        graft.into(),
        PatchStatus::Ok,
        "Fork | Power Plant",
        &view,
    )
}

pub(crate) async fn create(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path(conversation_id): Path<String>,
    Form(form): Form<ForkForm>,
) -> AppResult<Response> {
    let Some(record) = super::super::load_conversation(&state, &conversation_id) else {
        return Ok(crate::responses::command_navigation("/conversations"));
    };
    let reject = |status: PatchStatus, message: &str| {
        let view = error_view(&record, form.message.trim(), message);
        super::super::render_page(
            &state,
            GraftRequest::Patch,
            status,
            "Fork | Power Plant",
            &view,
        )
    };
    if super::super::parse_revision(&form.revision) != Some(record.revision) {
        return reject(
            PatchStatus::Conflict,
            "The conversation changed. Open fork again.",
        );
    }
    if record.active_job.is_some() {
        return reject(
            PatchStatus::Conflict,
            "Wait for the current work to finish before you fork.",
        );
    }
    let Some(boundary) = crate::conversations::MessageId::parse(form.message.trim()) else {
        return reject(
            PatchStatus::UnprocessableEntity,
            ForkError::Missing.message(),
        );
    };
    let snapshot = match crate::conversations::forks::snapshot(&record, boundary) {
        Ok(snapshot) => snapshot,
        Err(error) => return reject(PatchStatus::UnprocessableEntity, error.message()),
    };
    let _ = graft;
    let nonce = crate::execution::draft_nonce().map_err(|_| {
        AppError::new(
            "create fork draft nonce",
            std::io::Error::other("system random source unavailable"),
        )
    })?;
    if let Err(error) =
        state
            .forks
            .insert(session.0, nonce.clone(), snapshot.clone(), &state.sessions)
    {
        return reject(PatchStatus::Conflict, error);
    }
    let mut draft = super::super::new::NewForm {
        draft_nonce: nonce,
        title: snapshot.source_title.clone(),
        fork_source: snapshot.source.as_hex(),
        ..Default::default()
    };
    if let Some(model) = &snapshot.model {
        super::super::settings::copy_settings_to_draft(&mut draft, &model.settings);
    }
    let view = ConversationDetailView::from_new(&state, session.0, draft, "");
    let mut patches = hypergraft::PatchSet::new().title(&view.document_title);
    patches.children("chat-main", &view)?;
    patches.replace_location("/conversations/new")?;
    Ok(patches.respond(PatchStatus::Ok)?)
}

fn view_for(
    record: &crate::conversations::ConversationRecord,
    raw_message: &str,
) -> Result<ForkView, ForkError> {
    let boundary = crate::conversations::MessageId::parse(raw_message).ok_or(ForkError::Missing)?;
    let snapshot = crate::conversations::forks::snapshot(record, boundary)?;
    let last = snapshot
        .messages
        .last()
        .expect("a fork snapshot is never empty");
    Ok(ForkView {
        heading: format!("Fork conversation · {}", record.title),
        source_href: format!("/conversations/{}", record.id.as_hex()),
        excerpt: excerpt(last),
        entries: snapshot.messages.len(),
        revision: record.revision.to_string(),
        message: raw_message.to_owned(),
        candidate_review: snapshot.candidate_review,
        can_fork: true,
        error: String::new(),
    })
}

fn error_view(
    record: &crate::conversations::ConversationRecord,
    raw_message: &str,
    error: &str,
) -> ForkView {
    ForkView {
        heading: format!("Fork conversation · {}", record.title),
        source_href: format!("/conversations/{}", record.id.as_hex()),
        excerpt: String::new(),
        entries: 0,
        revision: record.revision.to_string(),
        message: raw_message.to_owned(),
        candidate_review: false,
        can_fork: false,
        error: error.to_owned(),
    }
}

fn excerpt(message: &crate::conversations::ConversationMessage) -> String {
    let mut text = message.text.clone();
    if text.trim().is_empty() {
        text = message
            .activity
            .iter()
            .filter_map(|activity| match activity {
                crate::providers::AssistantActivity::Response(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n");
    }
    const MAXIMUM: usize = 280;
    if text.len() <= MAXIMUM {
        return text;
    }
    let mut end = MAXIMUM;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}
