use askama::Template;

#[cfg(test)]
mod tests;

use axum::{
    extract::{Path, Query, State},
    response::Response,
};
use hypergraft::PageGraft;
use serde::Deserialize;

use crate::{
    error::AppResult, execution::OutputScope, responses, sessions::RequiredSession, state::AppState,
};

use super::super::load_conversation;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct OutputQuery {
    offset: String,
}

#[derive(Template)]
#[template(path = "conversations/output/templates/index.html")]
pub(super) struct OutputView {
    pub(super) heading: String,
    pub(super) reference: String,
    pub(super) back_href: String,
    pub(super) error: String,
    pub(super) chunks: Vec<OutputChunkView>,
    pub(super) next_offset: Option<String>,
    pub(super) next_href: String,
    pub(super) truncated: bool,
    pub(super) line_truncated: bool,
}

pub(super) struct OutputChunkView {
    pub(super) stream: &'static str,
    pub(super) stderr: bool,
    pub(super) text: String,
}

pub(crate) async fn show(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PageGraft,
    Path((conversation_id, reference)): Path<(String, String)>,
    Query(query): Query<OutputQuery>,
) -> AppResult<Response> {
    let _ = session;
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(responses::request_navigation(graft, "/conversations"));
    };
    let reference = reference.trim();
    if reference.len() != 32 || !reference.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return render_output(
            &state,
            &record,
            reference,
            "That command output reference is not valid.",
        );
    }
    let request = match query.offset.trim() {
        "" => crate::tools::read::parse_request(None, None),
        value => match value.parse::<u64>() {
            Ok(offset) => crate::tools::read::parse_request(Some(offset), None),
            Err(_) => {
                return render_output(
                    &state,
                    &record,
                    reference,
                    "That output offset is not valid.",
                );
            }
        },
    };
    let request = match request {
        Ok(request) => request,
        Err(error) => {
            return render_output(&state, &record, reference, error.message());
        }
    };
    let scope = OutputScope::conversation(record.id);
    match state.outputs.page(reference, &scope, request) {
        Ok(page) => {
            let base = format!("/conversations/{}/output/{reference}", record.id.as_hex());
            let next_href = page
                .next
                .map(|next| format!("{base}?offset={next}"))
                .unwrap_or_default();
            let view = OutputView {
                heading: format!("Command output · {}", record.title),
                reference: reference.to_owned(),
                back_href: format!("/conversations/{}", record.id.as_hex()),
                error: String::new(),
                chunks: page
                    .chunks
                    .iter()
                    .map(|chunk| OutputChunkView {
                        stream: chunk.stream.label(),
                        stderr: chunk.stream.is_stderr(),
                        text: chunk.text.clone(),
                    })
                    .collect(),
                next_offset: page.next.map(|next| next.to_string()),
                next_href,
                truncated: page.truncated,
                line_truncated: page.line_truncated,
            };
            render_output_view(&state, view)
        }
        Err(error) => render_output(&state, &record, reference, error.message()),
    }
}

fn render_output(
    state: &AppState,
    record: &crate::conversations::ConversationRecord,
    reference: &str,
    message: &str,
) -> AppResult<Response> {
    render_output_view(
        state,
        OutputView {
            heading: format!("Command output · {}", record.title),
            reference: reference.to_owned(),
            back_href: format!("/conversations/{}", record.id.as_hex()),
            error: message.to_owned(),
            chunks: Vec::new(),
            next_offset: None,
            next_href: String::new(),
            truncated: false,
            line_truncated: false,
        },
    )
}

fn render_output_view(state: &AppState, view: OutputView) -> AppResult<Response> {
    responses::chat_page_response("Command output", state, &view)
}
