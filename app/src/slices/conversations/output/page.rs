use askama::Template;

#[cfg(test)]
mod tests;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Response,
};
use hypergraft::{GraftRequest, PageGraft, PatchStatus};
use serde::Deserialize;

use crate::{
    error::AppResult,
    execution::{OutputScope, command::CommandChunk},
    responses,
    sessions::RequiredSession,
    state::AppState,
};

use super::super::{load_conversation, page::output_target};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct OutputQuery {
    reference: String,
}

/// The replacement body for one command output record. The transcript preview
/// and the on-demand expansion render this same template, so a patch morphs
/// the existing text instead of appending a second copy.
#[derive(Template)]
#[template(path = "conversations/output/templates/index.html")]
pub(super) struct OutputFragment {
    pub(super) error: String,
    pub(super) chunks: Vec<OutputChunkView>,
    pub(super) truncated: bool,
}

pub(super) struct OutputChunkView {
    pub(super) stream: &'static str,
    pub(super) stderr: bool,
    pub(super) text: String,
}

/// Render the output body from raw chunks. Both the initial preview and the
/// expansion request use this function.
pub(crate) fn body_html(error: String, chunks: &[CommandChunk], truncated: bool) -> String {
    OutputFragment {
        error,
        chunks: chunks_view(chunks),
        truncated,
    }
    .render()
    .expect("output fragment template")
}

fn chunks_view(chunks: &[CommandChunk]) -> Vec<OutputChunkView> {
    crate::execution::command::merge_adjacent_streams(chunks)
        .into_iter()
        .map(|chunk| OutputChunkView {
            stream: chunk.stream.label(),
            stderr: chunk.stream.is_stderr(),
            text: chunk.text,
        })
        .collect()
}

/// Patch the full retained output into the transcript record that holds the
/// preview. The enhanced GET form opts out of history reconciliation, so the
/// address stays on the conversation page. A native GET redirects there
/// instead of serving a standalone output page.
pub(crate) async fn expand(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: GraftRequest,
    Path(conversation_id): Path<String>,
    Query(query): Query<OutputQuery>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(redirect(graft, "/conversations"));
    };
    let back = format!("/conversations/{}", record.id.as_hex());
    if graft != GraftRequest::Patch {
        return Ok(redirect(graft, &back));
    }
    let reference = query.reference.trim().to_owned();
    if reference.len() != 32 || !reference.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(responses::no_store_status_response(
            StatusCode::BAD_REQUEST,
            "Bad request",
        ));
    }
    let scope = OutputScope::conversation(record.id);
    let (chunks, truncated, error) = match state.outputs.full(&reference, &scope) {
        Ok((chunks, truncated)) => (chunks, truncated, String::new()),
        Err(error) => (Vec::new(), false, error.message().to_owned()),
    };
    let chunks = chunks_view(&chunks);
    let status = if error.is_empty() {
        PatchStatus::Ok
    } else {
        PatchStatus::UnprocessableEntity
    };
    Ok(hypergraft::outcome::children_patch(
        status,
        output_target(&reference),
        &OutputFragment {
            error,
            chunks,
            truncated,
        },
    )?)
}

fn redirect(graft: GraftRequest, destination: &str) -> Response {
    match graft {
        GraftRequest::Document => responses::page_redirect(PageGraft::Document, destination),
        GraftRequest::Navigation => responses::page_redirect(PageGraft::Navigation, destination),
        // A patch request navigates through an envelope, not an HTTP redirect.
        GraftRequest::Patch => responses::command_navigation(destination),
    }
}
