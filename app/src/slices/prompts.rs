//! Global prompt-template catalogue and editor.
//!
//! A prompt template is one Markdown file directly inside the global prompt
//! directory. The editor writes the same file format that discovery reads, so
//! a saved template is selectable in the composer on the next lookup. Direct
//! file placement stays supported and needs no restart.

mod page;

#[cfg(test)]
mod tests;

use axum::{
    Form, Router,
    extract::{Query, State, rejection::FormRejection},
    response::Response,
    routing::{get, post},
};
use hypergraft::{PageGraft, PatchGraft, PatchStatus};
use serde::Deserialize;

use crate::{
    conversations::prompts::PromptStoreError, error::AppResult, responses,
    sessions::RequiredSession, state::AppState,
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/prompts", get(show))
        .route("/prompts/save", post(save))
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Selection {
    new: String,
    edit: String,
}

async fn show(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: PageGraft,
    Query(query): Query<Selection>,
) -> AppResult<Response> {
    let (editor, error) = if !query.edit.is_empty() {
        match state.prompts.get(&query.edit) {
            Ok(record) => (Some(record.into()), String::new()),
            Err(error) => (None, error.message().to_owned()),
        }
    } else {
        (
            (!query.new.is_empty()).then(page::Editor::new),
            String::new(),
        )
    };
    let view = page::PromptsPage::new(&state.prompts, editor, error);
    match graft {
        PageGraft::Document => responses::chat_page_response(page::TITLE, &state, &view),
        PageGraft::Navigation => Ok(hypergraft::outcome::page_patch(
            page::TITLE,
            "chat-main",
            &view,
        )?),
    }
}

async fn save(
    State(state): State<AppState>,
    _session: RequiredSession,
    _graft: PatchGraft,
    form: Result<Form<page::Editor>, FormRejection>,
) -> AppResult<Response> {
    let Ok(Form(form)) = form else {
        return render_error(
            &state,
            None,
            "The prompt form is malformed.",
            PatchStatus::UnprocessableEntity,
        );
    };
    let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
        return render_error(
            &state,
            Some(form),
            crate::local_data::HOST_PATH_RESET_PENDING,
            PatchStatus::Conflict,
        );
    };
    let markdown =
        crate::conversations::prompts::compose(&form.description, &form.argument_hint, &form.body);
    let result = if form.fingerprint.is_empty() {
        state.prompts.create(&form.name, markdown)
    } else {
        state
            .prompts
            .update(&form.name, &form.fingerprint, markdown)
    };
    match result {
        Ok(record) => Ok(responses::command_navigation(&format!(
            "/prompts?edit={}",
            record.name
        ))),
        Err(error) => render_error(&state, Some(form), error.message(), error_status(error)),
    }
}

fn error_status(error: PromptStoreError) -> PatchStatus {
    match error {
        PromptStoreError::Conflict | PromptStoreError::Missing => PatchStatus::Conflict,
        _ => PatchStatus::UnprocessableEntity,
    }
}

fn render_error(
    state: &AppState,
    editor: Option<page::Editor>,
    error: &str,
    status: PatchStatus,
) -> AppResult<Response> {
    let view = page::PromptsPage::new(&state.prompts, editor, error.to_owned());
    Ok(hypergraft::outcome::children_patch(
        status,
        "chat-main",
        &view,
    )?)
}
