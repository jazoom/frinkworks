mod forms;
mod page;

#[cfg(test)]
mod tests;

use axum::{
    Form, Router,
    extract::{Path, Query, State},
    response::Response,
    routing::{get, post},
};
use hypergraft::{GraftRequest, PatchGraft, PatchStatus};
use serde::Deserialize;

use crate::{
    agents::{AgentDraft, AgentError, AgentId, AgentRecord},
    error::{AppError, AppResult},
    local_data::HOST_PATH_RESET_PENDING,
    providers::ModelSelection,
    responses,
    sessions::RequiredSession,
    state::AppState,
};

use self::{
    forms::{AgentFormState, DeleteForm, FormIntent, OrphanForm, REVISION_MESSAGE},
    page::{AgentFormView, CatalogueView},
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/agents", get(catalogue).post(create))
        .route("/agents/new", get(new_agent))
        .route("/agents/orphans/remove", post(remove_orphan))
        .route(
            "/agents/{agent_id}/configuration",
            get(show_configuration).post(update_configuration),
        )
        .route("/agents/{agent_id}/delete", post(delete_agent))
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct AgentQuery {}

async fn catalogue(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: GraftRequest,
) -> AppResult<Response> {
    render_catalogue(&state, graft, PatchStatus::Ok, "")
}

async fn new_agent(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: GraftRequest,
    Query(_query): Query<AgentQuery>,
) -> AppResult<Response> {
    render_form_page(
        &state,
        graft,
        PatchStatus::Ok,
        page::NEW_TITLE,
        create_form_view(&state, starter_form(), ""),
    )
}

async fn create(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: PatchGraft,
    Query(_query): Query<AgentQuery>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> AppResult<Response> {
    let (mut form, intent) = match AgentFormState::parse(pairs) {
        Ok(parsed) => parsed,
        Err(error) => {
            return render_form_command(
                &state,
                graft,
                PatchStatus::UnprocessableEntity,
                page::NEW_TITLE,
                create_form_view(&state, starter_form(), error.message()),
            );
        }
    };
    if intent != FormIntent::Save {
        if let Err(error) = form.apply(intent) {
            return render_form_command(
                &state,
                graft,
                PatchStatus::UnprocessableEntity,
                page::NEW_TITLE,
                create_form_view(&state, form, error.message()),
            );
        }
        return render_form_command(
            &state,
            graft,
            PatchStatus::Ok,
            page::NEW_TITLE,
            create_form_view(&state, form, ""),
        );
    }
    let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
        return render_form_command(
            &state,
            graft,
            PatchStatus::Conflict,
            page::NEW_TITLE,
            create_form_view(&state, form, HOST_PATH_RESET_PENDING),
        );
    };
    let draft = match form.draft() {
        Ok(draft) => draft,
        Err(error) => {
            return render_form_command(
                &state,
                graft,
                PatchStatus::UnprocessableEntity,
                page::NEW_TITLE,
                create_form_view(&state, form, error.message()),
            );
        }
    };
    if let Err(error) = validate_agent_selection(&state, &draft) {
        return render_form_command(
            &state,
            graft,
            PatchStatus::UnprocessableEntity,
            page::NEW_TITLE,
            create_form_view(&state, form, error),
        );
    }
    match state.agents.create(draft) {
        Ok(record) => Ok(responses::command_navigation(&format!(
            "/agents/{}/configuration",
            record.id.as_hex()
        ))),
        Err(error @ (AgentError::Random | AgentError::Persist | AgentError::Corrupt)) => {
            Err(AppError::new("store agent", error))
        }
        Err(error) => render_form_command(
            &state,
            graft,
            PatchStatus::UnprocessableEntity,
            page::NEW_TITLE,
            create_form_view(&state, form, error.message()),
        ),
    }
}

async fn show_configuration(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: GraftRequest,
    Path(agent_id): Path<String>,
) -> AppResult<Response> {
    let Some(record) = load_agent(&state, &agent_id) else {
        return Ok(responses::request_navigation(graft, "/agents"));
    };
    render_form_page(
        &state,
        graft,
        PatchStatus::Ok,
        page::CONFIG_TITLE,
        AgentFormView::edit(&state, &record, AgentFormState::from_record(&record), ""),
    )
}

async fn update_configuration(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: PatchGraft,
    Path(agent_id): Path<String>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> AppResult<Response> {
    let Some(record) = load_agent(&state, &agent_id) else {
        return Ok(responses::command_navigation("/agents"));
    };
    let (mut form, intent) = match AgentFormState::parse(pairs) {
        Ok(parsed) => parsed,
        Err(error) => {
            return render_form_command(
                &state,
                graft,
                PatchStatus::UnprocessableEntity,
                page::CONFIG_TITLE,
                AgentFormView::edit(
                    &state,
                    &record,
                    AgentFormState::from_record(&record),
                    error.message(),
                ),
            );
        }
    };
    if intent != FormIntent::Save {
        if let Err(error) = form.apply(intent) {
            return render_form_command(
                &state,
                graft,
                PatchStatus::UnprocessableEntity,
                page::CONFIG_TITLE,
                AgentFormView::edit(&state, &record, form, error.message()),
            );
        }
        return render_form_command(
            &state,
            graft,
            PatchStatus::Ok,
            page::CONFIG_TITLE,
            AgentFormView::edit(&state, &record, form, ""),
        );
    }
    let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
        return render_form_command(
            &state,
            graft,
            PatchStatus::Conflict,
            page::CONFIG_TITLE,
            AgentFormView::edit(&state, &record, form, HOST_PATH_RESET_PENDING),
        );
    };
    let Ok(_operation) = state.agent_leases.acquire(record.id) else {
        return render_form_command(
            &state,
            graft,
            PatchStatus::UnprocessableEntity,
            page::CONFIG_TITLE,
            AgentFormView::edit(&state, &record, form, "Wait until this reply finishes."),
        );
    };
    let revision = match form.revision() {
        Ok(Some(revision)) => revision,
        Ok(None) | Err(_) => {
            return render_form_command(
                &state,
                graft,
                PatchStatus::UnprocessableEntity,
                page::CONFIG_TITLE,
                AgentFormView::edit(&state, &record, form, REVISION_MESSAGE),
            );
        }
    };
    let draft = match form.draft() {
        Ok(draft) => draft,
        Err(error) => {
            return render_form_command(
                &state,
                graft,
                PatchStatus::UnprocessableEntity,
                page::CONFIG_TITLE,
                AgentFormView::edit(&state, &record, form, error.message()),
            );
        }
    };
    if let Err(error) = validate_agent_selection(&state, &draft) {
        return render_form_command(
            &state,
            graft,
            PatchStatus::UnprocessableEntity,
            page::CONFIG_TITLE,
            AgentFormView::edit(&state, &record, form, error),
        );
    }
    match state.agents.update(&record.id, revision, draft) {
        Ok(updated) => render_form_command(
            &state,
            graft,
            PatchStatus::Ok,
            page::CONFIG_TITLE,
            AgentFormView::edit(&state, &updated, AgentFormState::from_record(&updated), ""),
        ),
        Err(error) => render_configuration_error(&state, graft, record, form, error),
    }
}

async fn delete_agent(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: PatchGraft,
    Path(agent_id): Path<String>,
    Form(form): Form<DeleteForm>,
) -> AppResult<Response> {
    let Some(record) = load_agent(&state, &agent_id) else {
        return Ok(responses::command_navigation("/agents"));
    };
    let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
        return render_form_command(
            &state,
            graft,
            PatchStatus::Conflict,
            page::CONFIG_TITLE,
            AgentFormView::edit(
                &state,
                &record,
                AgentFormState::from_record(&record),
                HOST_PATH_RESET_PENDING,
            ),
        );
    };
    let Ok(_operation) = state.agent_leases.acquire(record.id) else {
        return render_form_command(
            &state,
            graft,
            PatchStatus::UnprocessableEntity,
            page::CONFIG_TITLE,
            AgentFormView::edit(
                &state,
                &record,
                AgentFormState::from_record(&record),
                "Wait until this reply finishes.",
            ),
        );
    };
    let revision = match form.revision() {
        Ok(Some(revision)) => revision,
        Ok(None) | Err(_) => {
            return render_form_command(
                &state,
                graft,
                PatchStatus::UnprocessableEntity,
                page::CONFIG_TITLE,
                AgentFormView::edit(
                    &state,
                    &record,
                    AgentFormState::from_record(&record),
                    REVISION_MESSAGE,
                ),
            );
        }
    };
    match state.agents.delete(&record.id, revision) {
        Ok(()) => Ok(responses::command_navigation("/agents")),
        Err(error) => {
            let form = AgentFormState::from_record(&record);
            render_configuration_error(&state, graft, record, form, error)
        }
    }
}

async fn remove_orphan(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: PatchGraft,
    Form(form): Form<OrphanForm>,
) -> AppResult<Response> {
    let error = match state.sandboxes.remove_orphan(form.name.trim()).await {
        Ok(()) => "",
        Err(error) => error.message(),
    };
    let status = if error.is_empty() {
        PatchStatus::Ok
    } else {
        PatchStatus::UnprocessableEntity
    };
    render_catalogue(&state, graft.into(), status, error)
}

fn validate_agent_selection(state: &AppState, draft: &AgentDraft) -> Result<(), &'static str> {
    let Some(selection) = &draft.selection else {
        return Ok(());
    };
    valid_selection(state, selection)
}

fn valid_selection(state: &AppState, selection: &ModelSelection) -> Result<(), &'static str> {
    if !state.vault.contains(selection.provider) {
        return Err("Connect the selected provider before you save this agent.");
    }
    if state
        .models_dev
        .model(selection.provider, &selection.model)
        .is_none()
    {
        return Err("Choose an available model.");
    }
    match selection.thinking.as_ref() {
        Some(effort)
            if !state
                .models_dev
                .supports(selection.provider, &selection.model, effort) =>
        {
            Err("Choose an available thinking effort.")
        }
        None if !state
            .models_dev
            .efforts(selection.provider, &selection.model)
            .is_empty() =>
        {
            Err("Choose an available thinking effort.")
        }
        _ => Ok(()),
    }
}

fn load_agent(state: &AppState, raw: &str) -> Option<AgentRecord> {
    AgentId::parse(raw).and_then(|id| state.agents.get(&id))
}

fn starter_form() -> AgentFormState {
    AgentFormState::blank()
}

fn create_form_view(state: &AppState, form: AgentFormState, error: &'static str) -> AgentFormView {
    AgentFormView::create(state, form, error)
}

fn render_configuration_error(
    state: &AppState,
    graft: PatchGraft,
    record: AgentRecord,
    form: AgentFormState,
    error: AgentError,
) -> AppResult<Response> {
    if matches!(error, AgentError::Missing) {
        return Ok(responses::command_navigation("/agents"));
    }
    let status = match error {
        AgentError::Persist | AgentError::Random | AgentError::Corrupt => {
            return Err(AppError::new("store agent", error));
        }
        AgentError::Conflict => PatchStatus::Conflict,
        _ => PatchStatus::UnprocessableEntity,
    };
    let view = match error {
        AgentError::Conflict => {
            let latest = state.agents.get(&record.id).unwrap_or(record);
            AgentFormView::edit(
                state,
                &latest,
                AgentFormState::from_record(&latest),
                error.message(),
            )
        }
        _ => AgentFormView::edit(state, &record, form, error.message()),
    };
    render_form_command(state, graft, status, page::CONFIG_TITLE, view)
}

fn render_catalogue(
    state: &AppState,
    graft: GraftRequest,
    status: PatchStatus,
    error: &'static str,
) -> AppResult<Response> {
    let view = CatalogueView::from_parts(&state.agents.list(), state.sandboxes.orphans(), error);
    render_desk(state, graft, status, page::CATALOGUE_TITLE, &view)
}

fn render_form_page(
    state: &AppState,
    graft: GraftRequest,
    status: PatchStatus,
    title: &str,
    view: AgentFormView,
) -> AppResult<Response> {
    match graft {
        GraftRequest::Document => {
            let mut response = responses::chat_page_response(title, state, &view)?;
            responses::apply_patch_status(&mut response, status);
            Ok(response)
        }
        GraftRequest::Navigation => Ok(hypergraft::outcome::page_patch(title, "chat-main", &view)?),
        GraftRequest::Patch => Ok(hypergraft::outcome::children_patch(
            status,
            "agent-form",
            &view.contents(),
        )?),
    }
}

fn render_form_command(
    _state: &AppState,
    _graft: PatchGraft,
    status: PatchStatus,
    _title: &str,
    view: AgentFormView,
) -> AppResult<Response> {
    Ok(hypergraft::outcome::children_patch(
        status,
        "agent-form",
        &view.contents(),
    )?)
}

fn render_desk<T: askama::Template>(
    state: &AppState,
    graft: GraftRequest,
    status: PatchStatus,
    title: &str,
    view: &T,
) -> AppResult<Response> {
    match graft {
        GraftRequest::Document => {
            let mut response = responses::chat_page_response(title, state, view)?;
            responses::apply_patch_status(&mut response, status);
            Ok(response)
        }
        GraftRequest::Navigation => Ok(hypergraft::outcome::page_patch(title, "chat-main", view)?),
        GraftRequest::Patch => Ok(hypergraft::outcome::children_patch(
            status,
            "chat-main",
            view,
        )?),
    }
}
