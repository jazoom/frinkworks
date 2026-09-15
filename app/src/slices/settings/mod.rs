mod page;

#[cfg(test)]
mod tests;

use axum::{
    Form, Router,
    extract::{State, rejection::FormRejection},
    response::Response,
    routing::{get, post},
};
use hypergraft::{PageGraft, PatchGraft, PatchStatus};
use serde::Deserialize;

use crate::{
    error::AppResult,
    local_data::{ResetError, ResetRequest},
    models::models_dev::RefreshResult,
    preferences::Theme,
    responses,
    sessions::RequiredSession,
    state::AppState,
};

use self::page::{
    CONFIRMATION_ABSENT, CONFIRMATION_DUPLICATED, CONFIRMATION_MALFORMED, LocalDataSection,
    ModelCatalogueSetting, RECORD_FAILED, ResetStatusPage, SettingsPage, ThemeSetting,
    WORKFLOW_BUSY,
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/settings", get(show))
        .route("/settings/theme", post(update_theme))
        .route(
            "/settings/model-catalogue/refresh",
            post(refresh_model_catalogue),
        )
        .route("/settings/local-data/reset", post(reset_local_data))
}

async fn show(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: PageGraft,
) -> AppResult<Response> {
    if state.local_data.is_pending() {
        return render_reset_status_page(&state, graft);
    }
    let page = SettingsPage::new(state.preferences.theme());
    match graft {
        PageGraft::Document => responses::chat_page_response(page::TITLE, &state, &page),
        PageGraft::Navigation => Ok(hypergraft::outcome::page_patch(
            page::TITLE,
            "chat-main",
            &page,
        )?),
    }
}

#[derive(Deserialize)]
struct ThemeForm {
    theme: String,
}

async fn update_theme(
    State(state): State<AppState>,
    _session: RequiredSession,
    _graft: PatchGraft,
    form: Result<Form<ThemeForm>, FormRejection>,
) -> AppResult<Response> {
    let Ok(Form(form)) = form else {
        return theme_patch(
            PatchStatus::UnprocessableEntity,
            &state,
            Some("Choose a listed theme."),
        );
    };
    let Some(theme) = Theme::parse(&form.theme) else {
        return theme_patch(
            PatchStatus::UnprocessableEntity,
            &state,
            Some("Choose a listed theme."),
        );
    };
    if let Err(error) = state.preferences.set_theme(theme) {
        crate::error::trace_operation_failure("store theme preference", &error);
        return theme_patch(
            PatchStatus::UnprocessableEntity,
            &state,
            Some("Power Plant could not save the theme. Try again."),
        );
    }
    theme_patch(PatchStatus::Ok, &state, None)
}

async fn refresh_model_catalogue(
    State(state): State<AppState>,
    _session: RequiredSession,
    _graft: PatchGraft,
) -> AppResult<Response> {
    let (status, message, error) = match state.models_dev.refresh_now().await {
        RefreshResult::Updated => (PatchStatus::Ok, Some("Model catalogue updated."), None),
        RefreshResult::Unchanged | RefreshResult::Skipped => (
            PatchStatus::Ok,
            Some("Model catalogue is up to date."),
            None,
        ),
        RefreshResult::Failed => (
            PatchStatus::UnprocessableEntity,
            None,
            Some("Power Plant could not refresh the model catalogue. Try again."),
        ),
    };
    Ok(hypergraft::outcome::children_patch(
        status,
        "model-catalogue-setting",
        &ModelCatalogueSetting::result(message, error),
    )?)
}

async fn reset_local_data(
    State(state): State<AppState>,
    _session: RequiredSession,
    _graft: PatchGraft,
    form: Result<Form<Vec<(String, String)>>, FormRejection>,
) -> AppResult<Response> {
    let Ok(Form(pairs)) = form else {
        return reset_section_patch(PatchStatus::UnprocessableEntity, CONFIRMATION_MALFORMED);
    };
    if let Err(message) = parse_confirmation(pairs) {
        return reset_section_patch(PatchStatus::UnprocessableEntity, message);
    }
    match state
        .local_data
        .request_reset(
            &state.workflow_execution,
            &state.projects,
            &state.agents,
            &state.conversations,
            &state.presets,
            &state.workflows,
        )
        .await
    {
        Ok(ResetRequest::Recorded | ResetRequest::Pending) => reset_status_patch(),
        Err(ResetError::WorkflowBusy) => reset_section_patch(PatchStatus::Conflict, WORKFLOW_BUSY),
        Err(ResetError::Catalogue(conflict)) => {
            reset_section_patch(PatchStatus::Conflict, conflict.message())
        }
        Err(ResetError::Persist(error)) => {
            crate::error::trace_operation_failure("record local data reset", &error);
            reset_section_patch(PatchStatus::UnprocessableEntity, RECORD_FAILED)
        }
    }
}

fn parse_confirmation(pairs: Vec<(String, String)>) -> Result<(), &'static str> {
    let mut confirmation = None;
    for (name, value) in pairs {
        if name != "confirmation" {
            return Err(CONFIRMATION_MALFORMED);
        }
        if confirmation.is_some() {
            return Err(CONFIRMATION_DUPLICATED);
        }
        confirmation = Some(value);
    }
    match confirmation.as_deref() {
        Some("reset") => Ok(()),
        None => Err(CONFIRMATION_ABSENT),
        Some(_) => Err(CONFIRMATION_MALFORMED),
    }
}

fn theme_patch(
    status: PatchStatus,
    state: &AppState,
    error: Option<&'static str>,
) -> AppResult<Response> {
    Ok(hypergraft::outcome::children_patch(
        status,
        "theme-setting",
        &ThemeSetting::new(state.preferences.theme(), error),
    )?)
}

fn reset_section_patch(status: PatchStatus, error: &'static str) -> AppResult<Response> {
    Ok(hypergraft::outcome::children_patch(
        status,
        "local-data-reset",
        &LocalDataSection::new(Some(error)),
    )?)
}

fn reset_status_patch() -> AppResult<Response> {
    Ok(hypergraft::outcome::children_patch(
        PatchStatus::Ok,
        "chat-main",
        &ResetStatusPage,
    )?)
}

fn render_reset_status_page(state: &AppState, graft: PageGraft) -> AppResult<Response> {
    match graft {
        PageGraft::Document => {
            responses::chat_page_response(page::RESET_STATUS_TITLE, state, &ResetStatusPage)
        }
        PageGraft::Navigation => Ok(hypergraft::outcome::page_patch(
            page::RESET_STATUS_TITLE,
            "chat-main",
            &ResetStatusPage,
        )?),
    }
}
