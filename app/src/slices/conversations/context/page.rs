use askama::Template;

#[cfg(test)]
mod tests;

use axum::{
    extract::{Path, State},
    response::Response,
};
use hypergraft::PageGraft;

use crate::{error::AppResult, responses, sessions::RequiredSession, state::AppState};

use super::super::load_conversation;

#[derive(Template)]
#[template(path = "conversations/context/templates/index.html")]
pub(super) struct ContextView {
    pub(super) heading: String,
    pub(super) model: String,
    pub(super) request_id: String,
    pub(super) back_href: String,
    pub(super) error: String,
    pub(super) sources: Vec<ResourceView>,
    pub(super) advertised: Vec<ResourceView>,
}

pub(super) struct ResourceView {
    pub(super) kind: &'static str,
    pub(super) scope: String,
    pub(super) path: String,
    pub(super) content_hash: String,
}

pub(crate) async fn show(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: PageGraft,
    Path((conversation_id, request_id)): Path<(String, String)>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(responses::request_navigation(graft, "/conversations"));
    };
    let _ = graft;
    let Some(request_id) = crate::conversations::RequestId::parse(request_id.trim()) else {
        return render_context(
            &state,
            &record,
            request_id.as_str(),
            "That request reference is not valid.",
        );
    };
    let Some(request) = request_for(&record, request_id) else {
        return render_context(
            &state,
            &record,
            &request_id.as_hex(),
            "That request is not part of this conversation.",
        );
    };
    let view = ContextView {
        heading: format!("Request context · {}", record.title),
        model: format!(
            "{} · {}",
            request.usage.provider.label(),
            request.usage.model
        ),
        request_id: request.id.as_hex(),
        back_href: format!("/conversations/{}", record.id.as_hex()),
        error: String::new(),
        sources: request
            .sources
            .iter()
            .map(resource_view)
            .collect::<Vec<_>>(),
        advertised: request
            .advertised
            .iter()
            .map(resource_view)
            .collect::<Vec<_>>(),
    };
    render_context_view(&state, view)
}

fn request_for(
    record: &crate::conversations::ConversationRecord,
    id: crate::conversations::RequestId,
) -> Option<crate::conversations::RequestUsage> {
    record
        .messages
        .iter()
        .flat_map(|message| message.requests.iter())
        .find(|request| request.id == id)
        .or_else(|| {
            record
                .summary_requests
                .iter()
                .find(|request| request.id == id)
        })
        .or_else(|| {
            record
                .compaction
                .as_ref()
                .map(|compaction| &compaction.request)
                .filter(|request| request.id == id)
        })
        .cloned()
}

fn resource_view(source: &crate::execution::ResourceSource) -> ResourceView {
    ResourceView {
        kind: source.kind.label(),
        scope: source.scope.clone(),
        path: source.path.clone(),
        content_hash: source.content_hash.clone(),
    }
}

fn render_context(
    state: &AppState,
    record: &crate::conversations::ConversationRecord,
    request_id: &str,
    message: &str,
) -> AppResult<Response> {
    render_context_view(
        state,
        ContextView {
            heading: format!("Request context · {}", record.title),
            model: String::new(),
            request_id: request_id.to_owned(),
            back_href: format!("/conversations/{}", record.id.as_hex()),
            error: message.to_owned(),
            sources: Vec::new(),
            advertised: Vec::new(),
        },
    )
}

fn render_context_view(state: &AppState, view: ContextView) -> AppResult<Response> {
    responses::chat_page_response("Request context", state, &view)
}
