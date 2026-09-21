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
    error::AppResult, responses, sessions::RequiredSession, skills::SkillError, state::AppState,
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/skills", get(show))
        .route("/skills/save", post(save))
        .route("/skills/delete", post(delete))
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Selection {
    new: String,
    edit: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteForm {
    directory: String,
    fingerprint: String,
}

async fn show(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: PageGraft,
    Query(query): Query<Selection>,
) -> AppResult<Response> {
    let (editor, error) = if !query.edit.is_empty() {
        match state.skills.get(&query.edit) {
            Ok(record) => (Some(record.into()), String::new()),
            Err(error) => (None, error.message().to_owned()),
        }
    } else {
        (
            (!query.new.is_empty()).then(page::Editor::new),
            String::new(),
        )
    };
    let view = page::SkillsPage::new(&state.skills, editor, error);
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
            "The skill form is malformed.",
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
    let result = if form.directory.is_empty() && form.fingerprint.is_empty() {
        state.skills.create(form.markdown.clone())
    } else {
        state
            .skills
            .update(&form.directory, &form.fingerprint, form.markdown.clone())
    };
    match result {
        Ok(record) => Ok(responses::command_navigation(&format!(
            "/skills?edit={}",
            record.directory
        ))),
        Err(error) => render_error(&state, Some(form), error.message(), error_status(error)),
    }
}

async fn delete(
    State(state): State<AppState>,
    _session: RequiredSession,
    _graft: PatchGraft,
    form: Result<Form<DeleteForm>, FormRejection>,
) -> AppResult<Response> {
    let Ok(Form(form)) = form else {
        return render_error(
            &state,
            None,
            "The deletion form is malformed.",
            PatchStatus::UnprocessableEntity,
        );
    };
    let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
        return render_error(
            &state,
            None,
            crate::local_data::HOST_PATH_RESET_PENDING,
            PatchStatus::Conflict,
        );
    };
    match state.skills.delete(&form.directory, &form.fingerprint) {
        Ok(()) => Ok(responses::command_navigation("/skills")),
        Err(error) => render_error(&state, None, error.message(), error_status(error)),
    }
}

fn error_status(error: SkillError) -> PatchStatus {
    match error {
        SkillError::Conflict | SkillError::Missing => PatchStatus::Conflict,
        _ => PatchStatus::UnprocessableEntity,
    }
}

fn render_error(
    state: &AppState,
    editor: Option<page::Editor>,
    error: &str,
    status: PatchStatus,
) -> AppResult<Response> {
    let view = page::SkillsPage::new(&state.skills, editor, error.to_owned());
    Ok(hypergraft::outcome::children_patch(
        status,
        "chat-main",
        &view,
    )?)
}
