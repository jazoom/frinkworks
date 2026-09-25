mod forms;
mod page;

#[cfg(test)]
mod tests;

use axum::{
    Form, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    response::Response,
    routing::{get, post},
};
use hypergraft::{PageGraft, PatchGraft, PatchStatus};

use crate::{
    error::AppResult,
    responses,
    sessions::{OptionalSession, RequiredSession},
    state::AppState,
    workflows::{CatalogueError, WorkflowId, WorkflowRecord},
};

use crate::environments::SnapshotAvailability;

use self::{
    forms::{
        FormError, FormErrors, FormIntent, WorkflowFormState, fill_step_from_preset, parse_delete,
    },
    page::{CatalogueView, WorkflowFormView},
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/workflows", get(catalogue).post(create))
        .route("/workflows/new", get(new_workflow))
        .route(
            "/workflows/{workflow_id}/configuration",
            get(show_configuration).post(update_configuration),
        )
        .route("/workflows/{workflow_id}/delete", post(delete_workflow))
        .layer(DefaultBodyLimit::max(forms::MAXIMUM_FORM_BYTES))
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct Selection {
    conversation: String,
    workflow: String,
}

async fn catalogue(
    State(state): State<AppState>,
    _session: OptionalSession,
    graft: PageGraft,
    Query(query): Query<Selection>,
) -> AppResult<Response> {
    render_catalogue(&state, graft, &query)
}

async fn new_workflow(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: PageGraft,
) -> AppResult<Response> {
    render_form_page(
        &state,
        graft.into(),
        PatchStatus::Ok,
        page::NEW_TITLE,
        WorkflowFormView::create(WorkflowFormState::blank(), FormErrors::default()),
    )
    .await
}

async fn create(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: PatchGraft,
    Form(pairs): Form<Vec<(String, String)>>,
) -> AppResult<Response> {
    let (mut form, intent) = match WorkflowFormState::parse(pairs) {
        Ok(parsed) => parsed,
        Err(error) => {
            return render_form_command(
                &state,
                graft,
                PatchStatus::UnprocessableEntity,
                page::NEW_TITLE,
                WorkflowFormView::create(
                    WorkflowFormState::blank(),
                    FormErrors::summary(error.message()),
                ),
            )
            .await;
        }
    };
    if intent != FormIntent::Save {
        if let Err(error) = form.apply(intent) {
            return render_form_command(
                &state,
                graft,
                PatchStatus::UnprocessableEntity,
                page::NEW_TITLE,
                WorkflowFormView::create(form, FormErrors::summary(error.message())),
            )
            .await;
        }
        if let FormIntent::ApplyPreset { step } = intent
            && let Err(error) = apply_form_preset(&state, &mut form, step)
        {
            return render_form_command(
                &state,
                graft,
                PatchStatus::UnprocessableEntity,
                page::NEW_TITLE,
                WorkflowFormView::create(form, FormErrors::summary(error)),
            )
            .await;
        }
        return render_form_command(
            &state,
            graft,
            PatchStatus::Ok,
            page::NEW_TITLE,
            WorkflowFormView::create(form, FormErrors::default()),
        )
        .await;
    }
    let definition = match form.to_definition() {
        Ok(definition) => definition,
        Err(errors) => {
            return render_form_command(
                &state,
                graft,
                PatchStatus::UnprocessableEntity,
                page::NEW_TITLE,
                WorkflowFormView::create(form, errors),
            )
            .await;
        }
    };
    if let Some(errors) = reject_invalid_settings(&state, &form, &definition).await {
        return render_form_command(
            &state,
            graft,
            PatchStatus::UnprocessableEntity,
            page::NEW_TITLE,
            WorkflowFormView::create(form, errors),
        )
        .await;
    }
    let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
        return render_form_command(
            &state,
            graft,
            PatchStatus::Conflict,
            page::NEW_TITLE,
            WorkflowFormView::create(form, FormErrors::summary("Local data reset is pending.")),
        )
        .await;
    };
    match state.workflows.create(definition) {
        Ok(record) => Ok(responses::request_navigation(
            graft,
            &format!("/workflows/{}/configuration", record.id.as_hex()),
        )),
        Err(error) => {
            render_form_command(
                &state,
                graft,
                status_for(error),
                page::NEW_TITLE,
                WorkflowFormView::create(form, error.into()),
            )
            .await
        }
    }
}

async fn show_configuration(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: PageGraft,
    Path(workflow_id): Path<String>,
) -> AppResult<Response> {
    let Some(record) = load_workflow(&state, &workflow_id) else {
        return Ok(responses::request_navigation(graft, "/workflows"));
    };
    render_form_page(
        &state,
        graft.into(),
        PatchStatus::Ok,
        page::CONFIG_TITLE,
        WorkflowFormView::edit_state(&record, FormErrors::default(), ""),
    )
    .await
}

async fn update_configuration(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: PatchGraft,
    Path(workflow_id): Path<String>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> AppResult<Response> {
    let Some(record) = load_workflow(&state, &workflow_id) else {
        return Ok(responses::request_navigation(graft, "/workflows"));
    };
    let (mut form, intent) = match WorkflowFormState::parse(pairs) {
        Ok(parsed) => parsed,
        Err(error) => {
            return render_form_command(
                &state,
                graft,
                PatchStatus::UnprocessableEntity,
                page::CONFIG_TITLE,
                WorkflowFormView::edit_state(&record, FormErrors::summary(error.message()), ""),
            )
            .await;
        }
    };
    if intent != FormIntent::Save {
        if let Err(error) = form.apply(intent) {
            return render_form_command(
                &state,
                graft,
                PatchStatus::UnprocessableEntity,
                page::CONFIG_TITLE,
                WorkflowFormView::edit(&record, form, FormErrors::summary(error.message()), ""),
            )
            .await;
        }
        if let FormIntent::ApplyPreset { step } = intent
            && let Err(error) = apply_form_preset(&state, &mut form, step)
        {
            return render_form_command(
                &state,
                graft,
                PatchStatus::UnprocessableEntity,
                page::CONFIG_TITLE,
                WorkflowFormView::edit(&record, form, FormErrors::summary(error), ""),
            )
            .await;
        }
        return render_form_command(
            &state,
            graft,
            PatchStatus::Ok,
            page::CONFIG_TITLE,
            WorkflowFormView::edit(&record, form, FormErrors::default(), ""),
        )
        .await;
    }
    let Some(revision) = form.revision else {
        return render_form_command(
            &state,
            graft,
            PatchStatus::UnprocessableEntity,
            page::CONFIG_TITLE,
            WorkflowFormView::edit(
                &record,
                form,
                FormErrors::summary(FormError::Revision.message()),
                "",
            ),
        )
        .await;
    };
    let definition = match form.to_definition() {
        Ok(definition) => definition,
        Err(errors) => {
            return render_form_command(
                &state,
                graft,
                PatchStatus::UnprocessableEntity,
                page::CONFIG_TITLE,
                WorkflowFormView::edit(&record, form, errors, ""),
            )
            .await;
        }
    };
    if let Some(errors) = reject_invalid_settings(&state, &form, &definition).await {
        return render_form_command(
            &state,
            graft,
            PatchStatus::UnprocessableEntity,
            page::CONFIG_TITLE,
            WorkflowFormView::edit(&record, form, errors, ""),
        )
        .await;
    }
    let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
        return render_form_command(
            &state,
            graft,
            PatchStatus::Conflict,
            page::CONFIG_TITLE,
            WorkflowFormView::edit(
                &record,
                form,
                FormErrors::summary("Local data reset is pending."),
                "",
            ),
        )
        .await;
    };
    match state.workflows.update(&record.id, revision, definition) {
        Ok(updated) => {
            render_form_command(
                &state,
                graft,
                PatchStatus::Ok,
                page::CONFIG_TITLE,
                WorkflowFormView::edit_state(&updated, FormErrors::default(), ""),
            )
            .await
        }
        Err(error) => {
            render_form_command(
                &state,
                graft,
                status_for(error),
                page::CONFIG_TITLE,
                WorkflowFormView::edit(&record, form, error.into(), ""),
            )
            .await
        }
    }
}

async fn delete_workflow(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: PatchGraft,
    Path(workflow_id): Path<String>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> AppResult<Response> {
    let Some(record) = load_workflow(&state, &workflow_id) else {
        return Ok(responses::request_navigation(graft, "/workflows"));
    };
    let (revision, confirmed) = match parse_delete(&pairs) {
        Ok(parsed) => parsed,
        Err(error) => {
            return render_form_command(
                &state,
                graft,
                PatchStatus::UnprocessableEntity,
                page::CONFIG_TITLE,
                WorkflowFormView::edit_state(&record, FormErrors::default(), error.message()),
            )
            .await;
        }
    };
    if !confirmed {
        return render_form_command(
            &state,
            graft,
            PatchStatus::UnprocessableEntity,
            page::CONFIG_TITLE,
            WorkflowFormView::edit_state(
                &record,
                FormErrors::default(),
                "Tick the box to delete this workflow.",
            ),
        )
        .await;
    }
    let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
        return render_form_command(
            &state,
            graft,
            PatchStatus::Conflict,
            page::CONFIG_TITLE,
            WorkflowFormView::edit_state(
                &record,
                FormErrors::default(),
                "Local data reset is pending.",
            ),
        )
        .await;
    };
    match state.workflows.delete(&record.id, revision) {
        Ok(()) => Ok(responses::request_navigation(graft, "/workflows")),
        Err(error) => {
            render_form_command(
                &state,
                graft,
                status_for(error),
                page::CONFIG_TITLE,
                WorkflowFormView::edit_state(&record, FormErrors::default(), error.message()),
            )
            .await
        }
    }
}

async fn reject_invalid_settings(
    state: &AppState,
    form: &WorkflowFormState,
    definition: &crate::workflows::definition::WorkflowDefinition,
) -> Option<FormErrors> {
    let mut errors = FormErrors::for_state(form);
    errors.summary = "Fix the highlighted settings.";
    let mut invalid = false;
    let mut ready = Vec::new();
    for record in state.environments.list() {
        let Some(id) = record.ready_preparation else {
            continue;
        };
        let Some(preparation) = state.environments.preparation(&id) else {
            continue;
        };
        let Some(snapshot) = preparation.snapshot else {
            continue;
        };
        if state.environment_snapshots.inspect(&snapshot).await != SnapshotAvailability::Available {
            continue;
        }
        ready.push(record.id);
    }
    let default = definition.default_environment();
    if !ready.contains(&default) {
        errors.default_environment = "Choose a ready environment.";
        invalid = true;
    }
    for (index, step) in definition.steps().iter().enumerate() {
        if let crate::workflows::definition::StepAction::Agent(action) = &step.action
            && let crate::workflows::definition::ModelStepSettings::Override(settings) =
                &action.settings
            && let Some(model) = &settings.model
            && state
                .models_dev
                .model(model.provider, &model.model)
                .is_some()
        {
            let efforts = state.models_dev.efforts(model.provider, &model.model);
            if model
                .thinking
                .as_ref()
                .map_or(!efforts.is_empty(), |effort| !efforts.contains(effort))
            {
                errors.steps[index].settings =
                    "Choose a reasoning effort that this model supports.";
                invalid = true;
            }
        }
        if let Some(crate::workflows::definition::StepEnvironment::Override { environment_id }) =
            step.environment()
            && !ready.contains(&environment_id)
        {
            errors.steps[index].environment = "Choose a ready environment.";
            invalid = true;
        }
    }
    invalid.then_some(errors)
}

fn load_workflow(state: &AppState, raw: &str) -> Option<WorkflowRecord> {
    WorkflowId::parse(raw).and_then(|id| state.workflows.get(&id))
}

fn status_for(error: CatalogueError) -> PatchStatus {
    match error {
        CatalogueError::Conflict | CatalogueError::Missing => PatchStatus::Conflict,
        _ => PatchStatus::UnprocessableEntity,
    }
}

fn render_catalogue(state: &AppState, graft: PageGraft, query: &Selection) -> AppResult<Response> {
    let view = CatalogueView::new(state, query);
    match graft {
        PageGraft::Document => {
            let mut response = responses::chat_page_response(page::INDEX_TITLE, state, &view)?;
            responses::apply_patch_status(&mut response, PatchStatus::Ok);
            Ok(response)
        }
        PageGraft::Navigation => Ok(hypergraft::outcome::page_patch(
            page::INDEX_TITLE,
            "chat-main",
            &view,
        )?),
    }
}

async fn attach_environments(state: &AppState, view: WorkflowFormView) -> WorkflowFormView {
    let mut options = Vec::new();
    for record in state.environments.list() {
        let Some(ready) = record.ready_preparation else {
            continue;
        };
        let Some(preparation) = state.environments.preparation(&ready) else {
            continue;
        };
        let Some(snapshot) = preparation.snapshot else {
            continue;
        };
        if state.environment_snapshots.inspect(&snapshot).await != SnapshotAvailability::Available {
            continue;
        }
        let selected = view.default_environment == record.id.as_hex();
        options.push(page::EnvironmentOption {
            id: record.id.as_hex(),
            name: record.name.clone(),
            context: format!(
                "{} · {}",
                record.recipe.oci_image.as_str(),
                snapshot.snapshot_digest.short_hex()
            ),
            selected,
        });
    }
    let no_ready = options.is_empty();
    let presets = state
        .presets
        .list()
        .into_iter()
        .map(|preset| page::PresetChoice {
            id: preset.id.as_hex(),
            name: preset.name,
        })
        .collect();
    let providers = crate::providers::ProviderKind::ALL
        .into_iter()
        .map(|kind| page::ProviderChoice {
            value: kind.as_str(),
            label: kind.label(),
        })
        .collect();
    view.with_environments(options, no_ready)
        .with_catalogues(presets, providers)
}

fn apply_form_preset(
    state: &AppState,
    form: &mut WorkflowFormState,
    step: usize,
) -> Result<(), &'static str> {
    let draft = form.steps.get_mut(step).ok_or("Choose a model phase.")?;
    if draft.action != "agent" {
        return Err("Presets apply only to model phases.");
    }
    let id = crate::presets::PresetId::parse(&draft.settings_preset)
        .ok_or("Choose an available preset.")?;
    let preset = state
        .presets
        .get(&id)
        .ok_or("That preset is unavailable. Choose another preset.")?;
    fill_step_from_preset(draft, &preset);
    Ok(())
}

async fn render_form_page(
    state: &AppState,
    graft: hypergraft::GraftRequest,
    status: PatchStatus,
    title: &str,
    view: WorkflowFormView,
) -> AppResult<Response> {
    let view = attach_environments(state, view).await;
    match graft {
        hypergraft::GraftRequest::Document => {
            let mut response = responses::chat_page_response(title, state, &view)?;
            responses::apply_patch_status(&mut response, status);
            Ok(response)
        }
        hypergraft::GraftRequest::Navigation => {
            Ok(hypergraft::outcome::page_patch(title, "chat-main", &view)?)
        }
        hypergraft::GraftRequest::Patch => Ok(hypergraft::outcome::children_patch(
            status,
            "workflow-form",
            &view.contents(),
        )?),
    }
}

async fn render_form_command(
    state: &AppState,
    _graft: PatchGraft,
    status: PatchStatus,
    _title: &str,
    view: WorkflowFormView,
) -> AppResult<Response> {
    let view = attach_environments(state, view).await;
    Ok(hypergraft::outcome::children_patch(
        status,
        "workflow-form",
        &view.contents(),
    )?)
}
