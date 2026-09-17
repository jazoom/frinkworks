mod forms;
mod job;
mod page;

#[cfg(test)]
mod tests;

use axum::{
    Extension, Form, Router,
    extract::{Query, State, rejection::FormRejection},
    response::Response,
    routing::{get, post},
};

use hypergraft::{GraftRequest, PatchGraft, PatchSet, PatchStatus};

use crate::{
    agents::AgentRecord,
    environments::EnvironmentCatalogue,
    error::AppResult,
    projects::{ProjectId, ProjectRecord, eligibility, eligible_agents},
    responses,
    sessions::{ConversationKey, OptionalSession, SessionId, SessionSnapshot},
    state::AppState,
    workflows::{self, WorkflowSelection},
};

use self::{forms::ModelForm, page::ChatViewModel};
pub(crate) use job::{AgentOutcome, AgentRunSpec, StreamRedactor, bound_reply, run_agent_action};

#[derive(Clone, Copy)]
pub(crate) struct DeskPage<'a> {
    pub(crate) project: &'a ProjectRecord,
    pub(crate) agent: &'a AgentRecord,
    pub(crate) eligible: &'a [AgentRecord],
    pub(crate) snapshot: &'a SessionSnapshot,
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/model", get(refresh_model_options).post(update_model))
        .route("/thinking-visibility", post(update_thinking_visibility))
}

#[derive(Default, serde::Deserialize)]
struct ThinkingVisibilityForm {
    #[serde(default)]
    show_thinking: bool,
}

async fn update_thinking_visibility(
    State(state): State<AppState>,
    _session: crate::sessions::RequiredSession,
    _graft: PatchGraft,
    form: Result<Form<ThinkingVisibilityForm>, FormRejection>,
) -> AppResult<Response> {
    let Ok(Form(form)) = form else {
        return thinking_visibility_patch(
            PatchStatus::UnprocessableEntity,
            state.preferences.show_thinking(),
            Some("Choose whether to show thinking."),
        );
    };
    if let Err(error) = state.preferences.set_show_thinking(form.show_thinking) {
        crate::error::trace_operation_failure("store thinking visibility preference", &error);
        return thinking_visibility_patch(
            PatchStatus::UnprocessableEntity,
            state.preferences.show_thinking(),
            Some("Power Plant could not save this preference. Try again."),
        );
    }
    thinking_visibility_patch(PatchStatus::Ok, form.show_thinking, None)
}

fn thinking_visibility_patch(
    status: PatchStatus,
    show_thinking: bool,
    error: Option<&'static str>,
) -> AppResult<Response> {
    Ok(hypergraft::outcome::children_patch(
        status,
        "thinking-visibility",
        &page::ThinkingVisibilityControl {
            show_thinking,
            thinking_visibility_error: error,
        },
    )?)
}

pub(super) fn live_router() -> hypergraft::live::LiveRouter<AppState> {
    hypergraft::live::LiveRouter::new()
        .route("/model", model_live)
        .expect("live projection paths are unique")
}

#[derive(Default, serde::Deserialize)]
struct ModelQuery {
    #[serde(default)]
    project: String,
    #[serde(default)]
    agent: String,
}

struct ResolvedDesk {
    project: ProjectRecord,
    agent: AgentRecord,
    eligible: Vec<AgentRecord>,
    snapshot: SessionSnapshot,
}

async fn model_live(
    State(state): State<AppState>,
    Extension(session): Extension<SessionId>,
    Query(query): Query<ModelQuery>,
) -> Result<hypergraft::live::LiveProjection<SessionId>, hypergraft::live::LiveReject> {
    let invalidations = state.models_dev.subscribe();
    let Some(project) = ProjectId::parse(query.project.trim()) else {
        return Err(hypergraft::live::LiveReject::Invalid);
    };
    let Some(agent) = crate::agents::AgentId::parse(query.agent.trim()) else {
        return Err(hypergraft::live::LiveReject::Invalid);
    };
    if !state.vault.has_providers()
        || resolved_desk(&state, session, &project.as_hex(), &agent.as_hex()).is_none()
    {
        return Err(hypergraft::live::LiveReject::Retire);
    }
    Ok(hypergraft::live::LiveProjection::new(
        hypergraft::live::broadcast_invalidations(invalidations),
        move |session| {
            let state = state.clone();
            async move { refresh_model_projection(&state, session, project, agent).await }
        },
    ))
}

async fn refresh_model_projection(
    state: &AppState,
    session: SessionId,
    project: ProjectId,
    agent: crate::agents::AgentId,
) -> Result<PatchSet, hypergraft::live::ProjectionError> {
    if !state.vault.has_providers() {
        return Err(hypergraft::live::ProjectionError::Retire);
    }
    let desk = resolved_desk(state, session, &project.as_hex(), &agent.as_hex())
        .ok_or(hypergraft::live::ProjectionError::Retire)?;
    let view = desk_view(state, &desk).await;
    let mut patches = PatchSet::new();
    patches
        .children("desk-model-catalogue", &view.desk_model_catalogue())
        .and_then(|_| patches.children("desk-thinking-control", &view.thinking_control()))
        .and_then(|_| patches.children("desk-model-context", &view.model_context()))
        .map_err(|_| hypergraft::live::ProjectionError::Retire)?;
    Ok(patches)
}

impl ResolvedDesk {
    fn page(&self) -> DeskPage<'_> {
        DeskPage {
            project: &self.project,
            agent: &self.agent,
            eligible: &self.eligible,
            snapshot: &self.snapshot,
        }
    }
}

async fn refresh_model_options(
    State(state): State<AppState>,
    OptionalSession(session): OptionalSession,
    graft: GraftRequest,
    Query(query): Query<ModelQuery>,
) -> AppResult<Response> {
    let Some(session) = session else {
        return Ok(responses::request_navigation(graft, "/connect"));
    };
    if !state.vault.has_providers() {
        return Ok(responses::request_navigation(graft, "/connect"));
    }
    match graft {
        GraftRequest::Patch => {
            let Some(desk) = resolved_desk(&state, session, &query.project, &query.agent) else {
                return Ok(responses::request_navigation(graft, "/projects"));
            };
            let view = desk_view(&state, &desk).await;
            Ok(hypergraft::outcome::children_patch(
                PatchStatus::Ok,
                "desk-model-catalogue",
                &view.desk_model_catalogue(),
            )?)
        }
        GraftRequest::Document | GraftRequest::Navigation => {
            Ok(responses::request_navigation(graft, "/"))
        }
    }
}

async fn update_model(
    State(state): State<AppState>,
    OptionalSession(session): OptionalSession,
    graft: PatchGraft,
    Form(form): Form<ModelForm>,
) -> AppResult<Response> {
    let Some(session) = session else {
        return Ok(responses::request_navigation(graft, "/connect"));
    };
    if !state.vault.has_providers() {
        return Ok(responses::request_navigation(graft, "/connect"));
    }
    let Some(desk) = resolved_desk(&state, session, &form.project, &form.agent) else {
        return Ok(responses::request_navigation(graft, "/projects"));
    };
    if state.sessions.busy(&session) {
        let view = desk_view(&state, &desk).await;
        return reject_model_view(&state, graft, view, "Wait until this reply finishes.").await;
    }
    if form.wants_favourite_toggle() {
        return toggle_favourite(&state, graft, &desk, &form).await;
    }

    match form.validate(|kind| state.vault.contains(kind)) {
        Ok((kind, model, submitted_effort)) => {
            let model = submitted_model(&state, &form, kind, model);
            let providers = state.preferences.desk_providers(&state.vault);
            let previous = providers.iter().find(|provider| provider.selected);
            let Some(target) = providers.iter().find(|provider| provider.kind == kind) else {
                let view = desk_view(&state, &desk).await;
                return reject_model_view(&state, graft, view, "Choose a stored provider.").await;
            };
            let selection_changed =
                previous.is_none_or(|provider| provider.kind != kind || provider.model != model);
            let thinking = if selection_changed {
                state
                    .models_dev
                    .effective_effort(kind, &model, target.thinking.as_ref())
            } else {
                match submitted_effort {
                    Some(effort) if state.models_dev.supports(kind, &model, &effort) => {
                        Some(effort)
                    }
                    None if state.models_dev.efforts(kind, &model).is_empty() => None,
                    _ => {
                        let view = desk_view(&state, &desk).await;
                        return reject_model_view(
                            &state,
                            graft,
                            view,
                            "Choose an available thinking effort.",
                        )
                        .await;
                    }
                }
            };
            state
                .preferences
                .select_settings(kind, model, thinking)
                .map_err(|error| crate::error::AppError::new("store model settings", error))?;
        }
        Err(forms::ModelError::Provider) => {
            let view = desk_view(&state, &desk).await;
            return reject_model_view(&state, graft, view, "Choose a stored provider.").await;
        }
        Err(forms::ModelError::Model) => {
            let view = desk_view(&state, &desk).await;
            return reject_model_view(&state, graft, view, "That model name is too long.").await;
        }
        Err(forms::ModelError::Thinking) => {
            let view = desk_view(&state, &desk).await;
            return reject_model_view(&state, graft, view, "Choose a thinking effort.").await;
        }
    }

    let view = desk_view(&state, &desk).await;
    Ok(hypergraft::outcome::children_patch(
        PatchStatus::Ok,
        "desk-settings",
        &view.desk_settings(),
    )?)
}

async fn toggle_favourite(
    state: &AppState,
    graft: PatchGraft,
    desk: &ResolvedDesk,
    form: &ModelForm,
) -> AppResult<Response> {
    match form.validate_favourite(|kind| state.vault.contains(kind)) {
        Ok((kind, model)) => {
            let model = submitted_model(state, form, kind, model);
            match state.preferences.toggle_favourite(kind, &model) {
                Ok(_) => {}
                Err(crate::preferences::FavouriteError::Full) => {
                    let view = desk_view(state, desk).await;
                    return reject_model_view(state, graft, view, "The favourites list is full.")
                        .await;
                }
                Err(crate::preferences::FavouriteError::Persist(error)) => {
                    return Err(crate::error::AppError::new("store favourite", error));
                }
            }
        }
        Err(forms::ModelError::Provider) => {
            let view = desk_view(state, desk).await;
            return reject_model_view(state, graft, view, "Choose a stored provider.").await;
        }
        Err(forms::ModelError::Model | forms::ModelError::Thinking) => {
            let view = desk_view(state, desk).await;
            return reject_model_view(state, graft, view, "Choose a model.").await;
        }
    }
    let view = desk_view(state, desk).await;
    Ok(hypergraft::outcome::children_patch(
        PatchStatus::Ok,
        "desk-model-catalogue",
        &view.desk_model_catalogue(),
    )?)
}

fn submitted_model(
    state: &AppState,
    form: &ModelForm,
    kind: crate::providers::ProviderKind,
    model: String,
) -> String {
    if form.provider_model_synced {
        return model;
    }
    let providers = state.preferences.desk_providers(&state.vault);
    let Some(selected) = providers.iter().find(|provider| provider.selected) else {
        return model;
    };
    if selected.kind == kind {
        return model;
    }
    providers
        .iter()
        .find(|provider| provider.kind == kind)
        .map(|provider| provider.model.clone())
        .unwrap_or(model)
}

async fn reject_model_view(
    _state: &AppState,
    _graft: PatchGraft,
    view: ChatViewModel,
    message: &'static str,
) -> AppResult<Response> {
    let mut view = view;
    view.desk_error = message;
    Ok(hypergraft::outcome::children_patch(
        PatchStatus::UnprocessableEntity,
        "desk-settings",
        &view.desk_settings(),
    )?)
}

pub(crate) async fn view(
    state: &AppState,
    page: DeskPage<'_>,
    error: &'static str,
    desk_error: &'static str,
    workflow_query: &str,
) -> ChatViewModel {
    let mut rendered = ChatViewModel::from_session(
        page.agent,
        page.snapshot,
        &state.preferences.desk_providers(&state.vault),
        &state.models_dev,
        error,
        desk_error,
    )
    .with_project(page.project, page.agent, page.eligible);
    rendered.show_thinking = state.preferences.show_thinking();
    attach_workflow_ui(state, page.snapshot, &mut rendered, workflow_query);
    attach_environment_preview(state, &mut rendered).await;
    attach_sandbox_status(state, &mut rendered).await;
    rendered
}

async fn attach_sandbox_status(state: &AppState, page: &mut ChatViewModel) {
    // This cursor precedes the status reads so concurrent catalogue changes remain observable.
    let cursor = state.environments.refresh_cursor();
    let seed_id = state
        .environments
        .seed_id(crate::environments::seeds::ALPINE_GIT_V1);
    let record = seed_id.and_then(|id| state.environments.get(&id));
    let latest = record
        .as_ref()
        .and_then(|record| state.environments.preparation(&record.latest_preparation));
    let ready_availability = match record.as_ref() {
        Some(record) => match state.environments.copy_ready_pointer(&record.id) {
            Ok(pointer) => Some(state.environment_snapshots.inspect(&pointer.snapshot).await),
            Err(_) => None,
        },
        None => None,
    };
    page.sandbox_status =
        page::SandboxStatus::from_parts(record.as_ref(), latest.as_ref(), ready_availability);
    page.quick_ready = page.sandbox_status.is_ready();
    page.sandbox_cursor = EnvironmentCatalogue::cursor_token(cursor);
}

async fn attach_environment_preview(state: &AppState, page: &mut ChatViewModel) {
    page.preview_ready = false;
    page.environment_preview.clear();
    page.environment_preview_error = "";
    let Some(option) = page.workflow_options.iter().find(|option| option.selected) else {
        return;
    };
    let Some(selection) = WorkflowSelection::parse(&option.token) else {
        page.environment_preview_error = "Choose a workflow.";
        return;
    };
    let Ok(resolved) = state.workflows.resolve(&selection) else {
        page.environment_preview_error = "That workflow is not valid. Choose another.";
        return;
    };
    match workflows::preview_environments(
        &resolved.pinned.definition,
        &state.environments,
        &state.environment_snapshots,
    )
    .await
    {
        Ok(preview) => {
            for environment in &preview.environments {
                page.environment_preview.push(page::PreviewLine {
                    text: format!(
                        "{} · preparation {} · {}",
                        environment.name,
                        environment.preparation_ordinal,
                        environment.snapshot_short
                    ),
                });
            }
            for step in preview.steps {
                page.environment_preview.push(page::PreviewLine {
                    text: format!(
                        "{} · {} · preparation {} · {}",
                        step.step,
                        step.environment_name,
                        step.preparation_ordinal,
                        step.snapshot_short
                    ),
                });
            }
            page.preview_ready = true;
        }
        Err(error) => page.environment_preview_error = error.message(),
    }
}

fn attach_workflow_ui(
    state: &AppState,
    session: &SessionSnapshot,
    page: &mut ChatViewModel,
    workflow_query: &str,
) {
    let records = state.workflows.list();
    if records.is_empty() {
        page.workflow_options = Vec::new();
        page.workflow_empty = true;
    } else {
        let mut preferred = session.preferred_workflow;
        if let Some(id) = preferred
            && state.workflows.get(&id).is_none()
        {
            preferred = None;
        }
        let queried = WorkflowSelection::parse(workflow_query.trim()).map(|item| item.workflow_id);
        let selected = queried.or(preferred).or_else(|| {
            if records.len() == 1 {
                Some(records[0].id)
            } else {
                None
            }
        });
        page.workflow_empty = false;
        page.workflow_options = records
            .iter()
            .map(|record| page::WorkflowOption {
                token: WorkflowSelection {
                    workflow_id: record.id,
                    definition_version: record.definition_version,
                }
                .as_token(),
                label: record.definition.name().to_owned(),
                summary: crate::workflows::summary::process_summary(&record.definition),
                selected: selected == Some(record.id),
            })
            .collect();
    }
}

async fn desk_view(state: &AppState, desk: &ResolvedDesk) -> ChatViewModel {
    view(state, desk.page(), "", "", "").await
}

fn resolved_desk(
    state: &AppState,
    session: SessionId,
    project: &str,
    agent: &str,
) -> Option<ResolvedDesk> {
    let project_id = ProjectId::parse(project.trim())?;
    let agent_id = crate::agents::AgentId::parse(agent.trim())?;
    let project = state.projects.get(&project_id)?;
    let record = state.agents.get(&agent_id)?;
    eligibility(&record, &project)?;
    let key = ConversationKey {
        project_id: project.id,
        agent_id: record.id,
    };
    let snapshot = state.sessions.snapshot(&session, &key)?;
    let eligible = eligible_agents(&state.agents.list(), &project);
    Some(ResolvedDesk {
        project,
        agent: record,
        eligible,
        snapshot,
    })
}
