use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
};
use hypergraft::{GraftRequest, PatchGraft, PatchStatus};
use serde::{Deserialize, Serialize};

use crate::{
    agents::AccessMode,
    conversations::{ConversationRecord, DocumentId, PlanRevisionReference},
    error::{AppError, AppResult},
    projects::ProjectId,
    providers::{ModelSelection, ProviderKind, ThinkingEffort},
    responses,
    sessions::RequiredSession,
    state::AppState,
    workflows::{
        self, PhaseModelSelection, PinnedPreset, ResolveWorkflowError, WorkflowJob, WorkflowRun,
        WorkflowSelection,
        definition::{CommitPolicy, ExecutionMode, LaunchInputSource},
    },
};

const TITLE_SUFFIX: &str = " | Power Plant";

#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct WorkflowQuery {
    stage: String,
    workflow: String,
    target: String,
    brief: String,
    commit_policy: String,
    plan: String,
    task_document: String,
    task_revision: String,
    task_hash: String,
    task_index: String,
    #[serde(default)]
    phase: Vec<String>,
}

#[derive(Deserialize)]
pub(super) struct WorkflowLaunchForm {
    revision: String,
    workflow: String,
    brief: String,
    #[serde(default)]
    target: String,
    #[serde(default)]
    plan: String,
    #[serde(default)]
    commit_policy: String,
    #[serde(default)]
    preview_workflow: String,
    #[serde(default)]
    preview_target: String,
    #[serde(default)]
    preview_commit_policy: String,
    #[serde(default)]
    preview_plan: String,
    #[serde(default)]
    task_document: String,
    #[serde(default)]
    task_revision: String,
    #[serde(default)]
    task_hash: String,
    #[serde(default)]
    task_index: String,
    #[serde(default)]
    preview_task_document: String,
    #[serde(default)]
    preview_task_revision: String,
    #[serde(default)]
    preview_task_hash: String,
    #[serde(default)]
    preview_task_index: String,
    #[serde(default)]
    phase: Vec<String>,
    #[serde(default)]
    confirm_additional_access: String,
}

struct WorkflowOption {
    token: String,
    name: String,
    summary: String,
    process_phases: Vec<crate::workflows::summary::ProcessPhase>,
    approvals: String,
    selected: bool,
}

struct TargetOption {
    id: String,
    name: String,
    access: String,
    selected: bool,
}

struct PlanOption {
    value: String,
    title: String,
    revision: String,
    content_hash: String,
    content_bytes: String,
    selected: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct PlanChoiceToken {
    document: String,
    revision: u32,
    content_hash: String,
    object_hash: String,
    artefact_hash: String,
}

struct SelectedPlan {
    reference: PlanRevisionReference,
    content: String,
}

struct SelectedTask {
    document_id: DocumentId,
    revision: u32,
    content_hash: String,
    index: u32,
    markdown: String,
    task_list: String,
}

struct PhaseChoice {
    value: String,
    label: String,
    detail: String,
    selected: bool,
}

struct PhaseModelOption {
    step: String,
    name: String,
    choices: Vec<PhaseChoice>,
}

struct CommitPolicyOption {
    value: String,
    label: String,
    detail: String,
    selected: bool,
}

#[derive(Serialize, Deserialize)]
struct PhaseChoiceToken {
    step: String,
    provider: String,
    model: String,
    thinking: Option<String>,
    preset: Option<String>,
    preset_revision: Option<u32>,
}

#[derive(Template)]
#[template(path = "conversations/templates/workflow.html", block = "launch_page")]
struct WorkflowLaunchView {
    stage: &'static str,
    document_title: String,
    conversation_id: String,
    revision: String,
    brief: String,
    workflows: Vec<WorkflowOption>,
    targets: Vec<TargetOption>,
    directory_launch: bool,
    plans: Vec<PlanOption>,
    requires_plan: bool,
    plan_optional: bool,
    requires_task_list: bool,
    task_lists: Vec<PlanOption>,
    task_document: String,
    task_revision: String,
    task_hash: String,
    task_index: String,
    task_preview: String,
    commit_policies: Vec<CommitPolicyOption>,
    phase_models: Vec<PhaseModelOption>,
    model_summary: String,
    access_summary: String,
    access_preview: String,
    phase_summaries: Vec<String>,
    environment_summary: String,
    input_summary: String,
    launch_blocked: bool,
    error: &'static str,
}

#[derive(Template)]
#[template(path = "conversations/templates/workflow.html", block = "launch_form")]
struct WorkflowLaunchContents<'a> {
    stage: &'static str,
    conversation_id: &'a str,
    revision: &'a str,
    brief: &'a str,
    workflows: &'a [WorkflowOption],
    targets: &'a [TargetOption],
    directory_launch: bool,
    plans: &'a [PlanOption],
    requires_plan: bool,
    plan_optional: bool,
    requires_task_list: bool,
    task_lists: &'a [PlanOption],
    task_document: &'a str,
    task_revision: &'a str,
    task_hash: &'a str,
    task_index: &'a str,
    task_preview: &'a str,
    commit_policies: &'a [CommitPolicyOption],
    phase_models: &'a [PhaseModelOption],
    model_summary: &'a str,
    access_summary: &'a str,
    access_preview: &'a str,
    phase_summaries: &'a [String],
    environment_summary: &'a str,
    input_summary: &'a str,
    launch_blocked: bool,
    error: &'static str,
}

impl WorkflowLaunchView {
    fn contents(&self) -> WorkflowLaunchContents<'_> {
        WorkflowLaunchContents {
            stage: self.stage,
            conversation_id: &self.conversation_id,
            revision: &self.revision,
            brief: &self.brief,
            workflows: &self.workflows,
            targets: &self.targets,
            directory_launch: self.directory_launch,
            plans: &self.plans,
            requires_plan: self.requires_plan,
            plan_optional: self.plan_optional,
            requires_task_list: self.requires_task_list,
            task_lists: &self.task_lists,
            task_document: &self.task_document,
            task_revision: &self.task_revision,
            task_hash: &self.task_hash,
            task_index: &self.task_index,
            task_preview: &self.task_preview,
            commit_policies: &self.commit_policies,
            phase_models: &self.phase_models,
            model_summary: &self.model_summary,
            access_summary: &self.access_summary,
            access_preview: &self.access_preview,
            phase_summaries: &self.phase_summaries,
            environment_summary: &self.environment_summary,
            input_summary: &self.input_summary,
            launch_blocked: self.launch_blocked,
            error: self.error,
        }
    }
}

fn parse_fields<T: serde::de::DeserializeOwned>(
    fields: Vec<(String, String)>,
) -> Result<(T, Vec<String>), serde::de::value::Error> {
    let mut phases = Vec::new();
    let fields = fields.into_iter().filter(|(key, value)| {
        if key == "phase" {
            phases.push(value.clone());
            false
        } else {
            true
        }
    });
    let form = T::deserialize(serde::de::value::MapDeserializer::new(fields))?;
    Ok((form, phases))
}

pub(super) async fn show(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: GraftRequest,
    Path(conversation_id): Path<String>,
    Query(fields): Query<Vec<(String, String)>>,
) -> AppResult<Response> {
    let Ok((mut query, phases)) = parse_fields::<WorkflowQuery>(fields) else {
        return Ok(axum::http::StatusCode::BAD_REQUEST.into_response());
    };
    query.phase = phases;
    if !matches!(query.stage.as_str(), "" | "choose" | "inputs" | "review") {
        return Ok(axum::http::StatusCode::BAD_REQUEST.into_response());
    }
    let Some(record) = super::load_conversation(&state, &conversation_id) else {
        return Ok(responses::request_navigation(graft, "/conversations"));
    };
    let mut view = launch_view(
        &state,
        &record,
        if query.workflow.is_empty() {
            None
        } else {
            Some(query.workflow.as_str())
        },
        if query.target.is_empty() {
            None
        } else {
            Some(query.target.as_str())
        },
        &query.brief,
        &query.commit_policy,
        &query.plan,
        &query.task_document,
        &query.task_revision,
        &query.task_hash,
        &query.task_index,
        &query.phase,
        "",
    )
    .await;
    if matches!(query.stage.as_str(), "inputs" | "review") && query.workflow.is_empty() {
        view.error = "Choose a workflow before you continue.";
    }
    if query.stage == "choose" {
        view.stage = "choose";
    } else if query.stage == "review" && view.stage == "inputs" && view.error.is_empty() {
        if let Err(error) = workflows::input_context::validate_launch_brief(&view.brief) {
            view.error = error.message();
        } else if (view.requires_plan
            && !view
                .plans
                .iter()
                .any(|p| p.selected && !p.content_hash.is_empty()))
            || (view.requires_task_list
                && !view
                    .task_lists
                    .iter()
                    .any(|p| p.selected && !p.content_hash.is_empty()))
        {
            view.error = "Choose the required saved input before review.";
        } else if !query.plan.trim().is_empty() {
            // An optional plan selection must still name an exact revision before
            // review, so Back and Review never carry a stale plan reference.
            match definition_for_plan(&state, &query.workflow, &query.plan) {
                Ok(Some(definition)) => {
                    if let Err(error) =
                        resolve_selected_plan(&state, &record, &query.plan, &definition)
                    {
                        view.error = error;
                    }
                }
                Ok(None) => {}
                Err(error) => view.error = error,
            }
        }
        if view.error.is_empty()
            && let Some(definition) = WorkflowSelection::parse(&query.workflow)
                .and_then(|selection| state.workflows.resolve(&selection).ok())
        {
            let phases = if query.phase.is_empty() {
                view.phase_models
                    .iter()
                    .flat_map(|phase| &phase.choices)
                    .filter(|choice| choice.selected)
                    .map(|choice| choice.value.clone())
                    .collect()
            } else {
                query.phase.clone()
            };
            match resolve_phase_models(&state, &definition.pinned.definition, &phases) {
                Ok(mut resolved) => {
                    view.stage = "review";
                    if view.directory_launch {
                        let result = preview_phase_access(
                            &state,
                            session.0,
                            &record,
                            &definition.pinned.definition,
                            &mut resolved,
                            &mut view,
                        )
                        .await;
                        if let Err(error) = result {
                            view.error = error;
                            view.stage = "inputs";
                        }
                    }
                }
                Err(error) => view.error = error,
            }
        }
    }
    if graft != GraftRequest::Patch {
        let html = view
            .contents()
            .render()
            .map_err(|error| AppError::new("render workflow companion", error))?;
        if html.len() <= 256 * 1024 {
            let mut workspace = super::detail_view(&state, session.0, &record, &record.title, "")
                .with_companion(
                    format!("<section id=\"workflow-launch\">{html}</section>"),
                    "workflow",
                );
            workspace.document_title = view.document_title.clone();
            return super::render_detail(&state, session.0, graft, PatchStatus::Ok, workspace);
        }
    }
    render(graft, PatchStatus::Ok, &view, &state)
}

pub(super) async fn launch(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path(conversation_id): Path<String>,
    Form(fields): Form<Vec<(String, String)>>,
) -> AppResult<Response> {
    let Ok((mut form, phases)) = parse_fields::<WorkflowLaunchForm>(fields) else {
        return Ok(axum::http::StatusCode::UNPROCESSABLE_ENTITY.into_response());
    };
    form.phase = phases;
    let Some(record) = super::load_conversation(&state, &conversation_id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let error_view = |status, error| {
        let state = state.clone();
        let record = record.clone();
        let workflow = form.workflow.clone();
        let target = form.target.clone();
        let brief = form.brief.clone();
        let commit_policy = form.commit_policy.clone();
        let plan = form.plan.clone();
        let task_document = form.task_document.clone();
        let task_revision = form.task_revision.clone();
        let task_hash = form.task_hash.clone();
        let task_index = form.task_index.clone();
        let phase = form.phase.clone();
        async move {
            let view = launch_view(
                &state,
                &record,
                Some(workflow.as_str()),
                Some(target.as_str()),
                &brief,
                &commit_policy,
                &plan,
                &task_document,
                &task_revision,
                &task_hash,
                &task_index,
                &phase,
                error,
            )
            .await;
            render(graft, status, &view, &state)
        }
    };
    let Some(revision) = super::parse_revision(&form.revision) else {
        return error_view(PatchStatus::UnprocessableEntity, super::REVISION_MESSAGE).await;
    };
    if record.revision != revision {
        return error_view(PatchStatus::Conflict, super::REVISION_MESSAGE).await;
    }
    if record.active_job.is_some() || state.sessions.busy(&session.0) {
        return error_view(
            PatchStatus::Conflict,
            "Wait until the current conversation command finishes.",
        )
        .await;
    }
    let brief = match workflows::input_context::validate_launch_brief(&form.brief) {
        Ok(brief) => brief,
        Err(error) => return error_view(PatchStatus::UnprocessableEntity, error.message()).await,
    };
    let Some(selection) = WorkflowSelection::parse(form.workflow.trim()) else {
        return error_view(
            PatchStatus::UnprocessableEntity,
            "Choose a current workflow from the catalogue.",
        )
        .await;
    };
    let resolved = match state.workflows.resolve(&selection) {
        Ok(resolved) => resolved,
        Err(error) => {
            let status = match error {
                ResolveWorkflowError::Missing | ResolveWorkflowError::Changed => {
                    PatchStatus::Conflict
                }
                ResolveWorkflowError::Invalid => PatchStatus::UnprocessableEntity,
            };
            return error_view(status, error.message()).await;
        }
    };
    if form.workflow != form.preview_workflow
        || form.target != form.preview_target
        || form.commit_policy != form.preview_commit_policy
    {
        return error_view(
            PatchStatus::Conflict,
            "The selection changed. Review its access and environment readiness before you start.",
        )
        .await;
    }
    let commit_policy = if form.commit_policy.trim().is_empty() {
        resolved.pinned.definition.commit_policy()
    } else {
        let Some(policy) = CommitPolicy::parse(form.commit_policy.trim()) else {
            return error_view(
                PatchStatus::UnprocessableEntity,
                "Choose a valid commit policy.",
            )
            .await;
        };
        policy
    };
    let mut pinned = match resolved.pinned.definition.with_commit_policy(commit_policy) {
        Ok(definition) => workflows::definition::PinnedWorkflowDefinition::pin(
            resolved.pinned.workflow_id,
            definition,
        ),
        Err(error) => return error_view(PatchStatus::UnprocessableEntity, error.message()).await,
    };
    // A selected plan for a workflow without a declared saved-plan input pins
    // that input for this run only, so Prepare implementation reaches an
    // existing reference workflow without a seventh starter.
    if !form.plan.trim().is_empty()
        && pinned.definition.execution_mode() == ExecutionMode::Once
        && !pinned
            .definition
            .launch_input_sources()
            .contains(&LaunchInputSource::SavedPlan)
    {
        match pinned.definition.with_saved_plan_input() {
            Ok(definition) => {
                pinned = workflows::definition::PinnedWorkflowDefinition::pin(
                    pinned.workflow_id,
                    definition,
                );
            }
            Err(_) => {
                return error_view(
                    PatchStatus::UnprocessableEntity,
                    "That workflow cannot take a saved plan.",
                )
                .await;
            }
        }
    }
    if form.task_document != form.preview_task_document
        || form.task_revision != form.preview_task_revision
        || form.task_hash != form.preview_task_hash
        || form.task_index != form.preview_task_index
    {
        return error_view(
            PatchStatus::Conflict,
            "The selected task changed. Review it before you start.",
        )
        .await;
    }
    let selected_task = match resolve_launch_task(
        pinned.definition.execution_mode(),
        &state,
        &record,
        &form.task_document,
        &form.task_revision,
        &form.task_hash,
        &form.task_index,
    ) {
        Ok(task) => task,
        Err(error) => return error_view(PatchStatus::UnprocessableEntity, error).await,
    };
    if selected_task.is_some() && !workflows::run::supports_task_execution(&pinned.definition) {
        return error_view(
            PatchStatus::UnprocessableEntity,
            "Choose implementation with optional review, code approval and commit for a selected task.",
        )
        .await;
    }
    if form.plan != form.preview_plan {
        return error_view(
            PatchStatus::Conflict,
            "The plan selection changed. Review the selected plan before you start.",
        )
        .await;
    }
    if let Err(error) = resolve_selected_plan(&state, &record, &form.plan, &pinned.definition) {
        return error_view(PatchStatus::UnprocessableEntity, error).await;
    }
    let directory_launch = uses_conversation_directories(&pinned.definition);
    let settings = super::effective_model(&state, &record).map(|model| model.settings);
    if directory_launch && settings.is_none() {
        return error_view(
            PatchStatus::UnprocessableEntity,
            "Choose a model before you start a workflow.",
        )
        .await;
    }
    let target = ProjectId::parse(form.target.trim());
    let mut target_record = record.clone();
    let authority = if directory_launch {
        None
    } else {
        let Some(target) = target else {
            return error_view(
                PatchStatus::UnprocessableEntity,
                "Choose an available target project.",
            )
            .await;
        };
        if !record.projects.contains(&target)
            || !record.grants.iter().any(|grant| grant.project_id == target)
        {
            return error_view(
                PatchStatus::UnprocessableEntity,
                "Grant access to the target project before launch.",
            )
            .await;
        }
        target_record.execution_target = Some(target);
        match crate::conversations::resolve_workflow_authority(
            &target_record,
            &state.projects,
            &state.agents,
        ) {
            Ok(Some(authority)) => Some(authority.effective),
            Ok(None) => {
                return error_view(
                    PatchStatus::UnprocessableEntity,
                    "Grant access to the target project before launch.",
                )
                .await;
            }
            Err(error) => return error_view(PatchStatus::Conflict, error.message()).await,
        }
    };
    if let Some(authority) = authority.as_ref()
        && !workflows::definition_fits_agent(
            &pinned.definition,
            &authority.tools,
            &authority
                .policy
                .grants()
                .iter()
                .map(|grant| (grant.alias.clone(), grant.access))
                .collect::<Vec<_>>(),
            &authority.grant_alias,
        )
    {
        return error_view(
            PatchStatus::UnprocessableEntity,
            "That workflow needs access outside the conversation ceiling.",
        )
        .await;
    }
    let mut phase_models = match resolve_phase_models(&state, &pinned.definition, &form.phase) {
        Ok(models) => models,
        Err(error) => return error_view(PatchStatus::UnprocessableEntity, error).await,
    };
    if let Some(authority) = authority.as_ref()
        && let Err(error) =
            validate_phase_models(&state, &pinned.definition, authority, &phase_models)
    {
        return error_view(PatchStatus::UnprocessableEntity, error).await;
    }
    if directory_launch {
        let defaults = settings.as_ref().expect("directory settings");
        if let Err(error) =
            resolve_directory_phase_settings(&pinned.definition, defaults, &mut phase_models)
        {
            return error_view(PatchStatus::UnprocessableEntity, error).await;
        }
        let phases: Vec<_> = phase_models
            .iter()
            .filter_map(|phase| {
                phase
                    .settings
                    .as_ref()
                    .map(|settings| (phase.step.clone(), settings.clone()))
            })
            .collect();
        let definition = match pinned.definition.with_phase_settings(defaults, &phases) {
            Ok(definition) => definition,
            Err(error) => {
                return error_view(PatchStatus::UnprocessableEntity, error.message()).await;
            }
        };
        pinned =
            workflows::definition::PinnedWorkflowDefinition::pin(pinned.workflow_id, definition);
    }
    let project_free = if directory_launch {
        let defaults = settings.as_ref().expect("directory settings");
        let merged = merged_phase_settings(defaults, &phase_models);
        match crate::execution::ProjectFreeAuthority::from_settings(record.revision, &merged) {
            Ok(authority) => Some(authority),
            Err(_) => {
                return error_view(
                    PatchStatus::UnprocessableEntity,
                    "A directory is no longer available at its authorised identity.",
                )
                .await;
            }
        }
    } else {
        None
    };
    let selection = phase_models
        .first()
        .map(|phase| phase.selection.clone())
        .or_else(|| super::effective_model(&state, &record).map(|model| model.settings.model));
    let Some(connection) = selection
        .as_ref()
        .and_then(|selection| state.vault.connection_for(selection))
    else {
        return error_view(
            PatchStatus::UnprocessableEntity,
            "Choose a model before you start a workflow.",
        )
        .await;
    };
    let environments = match workflows::resolve_environments(
        &pinned.definition,
        &state.environments,
        &state.environment_snapshots,
    )
    .await
    {
        Ok(environments) => environments,
        Err(error) => return error_view(PatchStatus::UnprocessableEntity, error.message()).await,
    };
    let execution = match state.workflow_execution.acquire() {
        Ok(execution) => execution,
        Err(_) => {
            return error_view(
                PatchStatus::Conflict,
                "Wait until the current workflow finishes.",
            )
            .await;
        }
    };
    let current = if target_record.execution_target != record.execution_target {
        match state.conversations.select_execution_target(
            &record.id,
            revision,
            target.expect("project target"),
        ) {
            Ok(current) => current,
            Err(error) => {
                return error_view(super::status_for(error), error.message()).await;
            }
        }
    } else {
        record.clone()
    };
    let authority = if directory_launch {
        if !state.sessions.contains_live(&session.0) {
            return error_view(PatchStatus::Conflict, "The browser session expired.").await;
        }
        None
    } else {
        match crate::conversations::resolve_workflow_authority(
            &current,
            &state.projects,
            &state.agents,
        ) {
            Ok(Some(authority)) => authority.effective,
            Ok(None) => {
                return error_view(
                    PatchStatus::Conflict,
                    "Project access was lost before launch.",
                )
                .await;
            }
            Err(error) => return error_view(PatchStatus::Conflict, error.message()).await,
        }
        .into()
    };
    // Environment resolution awaits external work. Revalidate the document before reservation.
    let selected_plan =
        match resolve_selected_plan(&state, &current, &form.plan, &pinned.definition) {
            Ok(plan) => plan,
            Err(error) => return error_view(PatchStatus::UnprocessableEntity, error).await,
        };
    let selected_task = match resolve_launch_task(
        pinned.definition.execution_mode(),
        &state,
        &current,
        &form.task_document,
        &form.task_revision,
        &form.task_hash,
        &form.task_index,
    ) {
        Ok(task) => task,
        Err(error) => return error_view(PatchStatus::UnprocessableEntity, error).await,
    };
    if pinned.definition.execution_mode() == ExecutionMode::TaskList {
        if selected_plan.is_some() {
            return error_view(
                PatchStatus::UnprocessableEntity,
                "A task-list workflow does not take a saved plan input.",
            )
            .await;
        }
        let snapshot = match resolve_task_list_snapshot(
            &state,
            &current,
            &form.task_document,
            &form.task_revision,
            &form.task_hash,
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => return error_view(PatchStatus::UnprocessableEntity, error).await,
        };
        let parsed = match workflows::task_list::parse(&snapshot.markdown) {
            Ok(parsed) => parsed,
            Err(_) => {
                return error_view(
                    PatchStatus::UnprocessableEntity,
                    "That task list is invalid.",
                )
                .await;
            }
        };
        let tasks: Vec<_> = parsed
            .eligible_tasks()
            .map(|task| workflows::TaskLoopItem {
                index: task.index,
                markdown: task.markdown.clone(),
                child_id: None,
                previous_child_ids: Vec::new(),
                outcome: workflows::TaskOutcome::Pending,
            })
            .collect();
        if tasks.len() > workflows::task_loop::MAXIMUM_LOOP_TASKS {
            return error_view(
                PatchStatus::UnprocessableEntity,
                workflows::task_loop::TaskLoopError::TaskLimit.message(),
            )
            .await;
        }
        if tasks.is_empty() {
            return error_view(
                PatchStatus::UnprocessableEntity,
                "The task list has no remaining tasks.",
            )
            .await;
        }
        let loop_id = workflows::TaskLoopId::generate()
            .map_err(|error| AppError::new("create task loop identifier", error))?;
        if directory_launch
            && state
                .access_consent
                .approve_loop_launch(
                    &form.confirm_additional_access,
                    loop_id,
                    session.0,
                    current.id,
                    phase_models
                        .iter()
                        .filter_map(|phase| phase.settings.clone())
                        .collect(),
                )
                .is_err()
        {
            return error_view(
                PatchStatus::Conflict,
                "Review and approve the exact phase settings before start.",
            )
            .await;
        }
        return launch_task_loop(
            state,
            session.0,
            current,
            authority,
            project_free,
            connection,
            execution,
            brief,
            pinned,
            environments,
            phase_models,
            snapshot,
            tasks,
            loop_id,
        )
        .await;
    }
    let run_id = workflows::RunId::generate()
        .map_err(|error| AppError::new("create workflow run identifier", error))?;
    if directory_launch
        && state
            .access_consent
            .approve_launch(
                &form.confirm_additional_access,
                run_id,
                session.0,
                current.id,
                phase_models
                    .iter()
                    .filter_map(|phase| phase.settings.clone())
                    .collect(),
            )
            .is_err()
    {
        return error_view(
            PatchStatus::Conflict,
            "Review and approve the exact phase settings before start.",
        )
        .await;
    }
    let mut run = if let Some(authority) = authority.as_ref() {
        WorkflowRun::create_configured_for_conversation(
            run_id,
            workflows::now_ms(),
            authority.project_id,
            current.id,
            brief.clone(),
            pinned,
            environments,
            phase_models.clone(),
        )
    } else {
        let mut run = WorkflowRun::create_source_free_for_conversation(
            run_id,
            workflows::now_ms(),
            current.id,
            pinned,
            environments,
            phase_models.clone(),
        );
        run.kind = workflows::run::RunKind::Configured;
        run.launch_brief = brief.clone();
        run
    };
    if let Some(task) = selected_task
        && run
            .set_task_selection(crate::workflows::TaskSelection {
                document_id: task.document_id,
                revision: task.revision,
                content_hash: task.content_hash,
                index: task.index,
                task_markdown: task.markdown,
                task_list: task.task_list,
            })
            .is_err()
    {
        return error_view(
            PatchStatus::UnprocessableEntity,
            "The selected task is invalid.",
        )
        .await;
    }
    if let Some(plan) = selected_plan {
        let imported = crate::workflows::artefacts::import_saved_plan(
            run_id,
            workflows::now_ms(),
            current.id,
            plan.reference.document_id,
            &plan.reference,
            &plan.content,
            &state.workflow_artefacts,
        );
        if imported
            .ok()
            .is_none_or(|record| run.record_launch_input(record).is_err())
        {
            return error_view(
                PatchStatus::UnprocessableEntity,
                "The selected plan could not be imported.",
            )
            .await;
        }
    }
    let job = match state.sessions.begin_conversation_job(
        &session.0,
        current.id,
        current.messages.len() + 1,
    ) {
        Ok(job) => job,
        Err(_) => {
            return error_view(
                PatchStatus::Conflict,
                "Another command is active in this browser session.",
            )
            .await;
        }
    };
    let started = match state.conversations.begin_message_with_model(
        &current.id,
        current.revision,
        None,
        job.id(),
        brief.clone(),
    ) {
        Ok(started) => started,
        Err(error) => {
            state
                .sessions
                .finish_conversation_job(&session.0, current.id, job.id());
            return error_view(super::status_for(error), error.message()).await;
        }
    };
    if let Err(error) = state.workflow_runs.create(run.clone()) {
        let _ = state.conversations.settle_message(
            &started.id,
            job.id(),
            String::new(),
            crate::conversations::MessageStatus::Failed,
            None,
        );
        let _ = state
            .sessions
            .finish_conversation_job(&session.0, started.id, job.id());
        return Err(AppError::new("store workflow run", error));
    }
    job.set_workflow_name(run.pinned.definition.name().to_owned());
    job.set_step_label("Source capture".to_owned());
    tokio::spawn(workflows::execute_run(
        state.clone(),
        WorkflowJob {
            run_id,
            session_id: session.0,
            project_id: run.project_id,
            agent_id: run.agent_id,
            agent_revision: authority
                .as_ref()
                .map_or(current.revision, |authority| authority.revision),
            conversation_id: Some(started.id),
            authority: authority.clone(),
            project_free_authority: project_free.clone(),
            grant_alias: authority
                .as_ref()
                .map_or_else(String::new, |authority| authority.grant_alias.clone()),
            grant_access: authority
                .as_ref()
                .map_or(AccessMode::ReadWrite, |authority| authority.grant_access),
            connection,
            phase_providers: phase_models
                .iter()
                .map(|phase| phase.selection.provider)
                .collect(),
            active_connection: std::sync::Arc::new(std::sync::Mutex::new(None)),
            host_policy: authority
                .as_ref()
                .map(|authority| authority.policy.clone())
                .or_else(|| {
                    project_free
                        .as_ref()
                        .map(|authority| authority.policy.clone())
                })
                .expect("workflow authority"),
            turns: Vec::new(),
            job: job.clone(),
            eligible_reply: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
            task_loop: None,
        },
        None,
        execution,
    ));
    Ok(responses::command_navigation(&format!(
        "/conversations/{}",
        started.id.as_hex()
    )))
}

#[allow(clippy::too_many_arguments)]
async fn launch_task_loop(
    state: AppState,
    session: crate::sessions::SessionId,
    current: ConversationRecord,
    authority: Option<crate::agents::EffectiveAuthority>,
    project_free: Option<crate::execution::ProjectFreeAuthority>,
    connection: crate::providers::ProviderConnection,
    execution: crate::workflows::ExecutionGuard,
    brief: String,
    pinned: crate::workflows::definition::PinnedWorkflowDefinition,
    environments: workflows::ResolvedEnvironmentSet,
    phase_models: Vec<PhaseModelSelection>,
    snapshot: workflows::TaskListSnapshot,
    tasks: Vec<workflows::TaskLoopItem>,
    loop_id: workflows::TaskLoopId,
) -> AppResult<Response> {
    let record = workflows::TaskLoop::create(
        loop_id,
        workflows::now_ms(),
        current.id,
        authority.as_ref().map(|authority| authority.project_id),
        None,
        brief.clone(),
        pinned,
        phase_models.clone(),
        environments,
        snapshot,
        tasks,
    )
    .map_err(|error| AppError::new("create task loop", error))?;
    let job = match state.sessions.begin_conversation_job(
        &session,
        current.id,
        current.messages.len() + 1,
    ) {
        Ok(job) => job,
        Err(_) => {
            return Ok(responses::command_navigation(&format!(
                "/conversations/{}",
                current.id.as_hex()
            )));
        }
    };
    let started = match state.conversations.begin_message_with_model(
        &current.id,
        current.revision,
        None,
        job.id(),
        brief.clone(),
    ) {
        Ok(started) => started,
        Err(error) => {
            state
                .sessions
                .finish_conversation_job(&session, current.id, job.id());
            return Err(AppError::new("begin task loop", error));
        }
    };
    if let Err(error) = state.task_loops.create(record) {
        let _ = state.conversations.settle_message(
            &started.id,
            job.id(),
            String::new(),
            crate::conversations::MessageStatus::Failed,
            None,
        );
        let _ = state
            .sessions
            .finish_conversation_job(&session, started.id, job.id());
        return Err(AppError::new("store task loop", error));
    }
    let fail_launch = |message: &str| {
        // Keep conversation ownership if the parent cannot reach a durable terminal state.
        if state.task_loops.fail(&loop_id).is_ok() {
            let _ = state.conversations.settle_message(
                &started.id,
                job.id(),
                message.to_owned(),
                crate::conversations::MessageStatus::Failed,
                None,
            );
            state
                .sessions
                .finish_conversation_job(&session, started.id, job.id());
        }
    };
    let (loop_record, child_id, task) = match state.task_loops.reserve_next_child(&loop_id, 0) {
        Ok(reserved) => reserved,
        Err(error) => {
            fail_launch(error.message());
            return Err(AppError::new("reserve task child", error));
        }
    };
    let child =
        match loop_record.child_run(child_id, workflows::now_ms(), task.index, task.markdown) {
            Ok(child) => child,
            Err(error) => {
                fail_launch(error.message());
                return Err(AppError::new("create task child", error));
            }
        };
    if let Err(error) = state.workflow_runs.create(child) {
        fail_launch("Power Plant could not store the task run.");
        return Err(AppError::new("store task child", error));
    }
    if let Err(error) = state.task_loops.mark_dispatched(&loop_id, child_id) {
        fail_launch(error.message());
        return Err(AppError::new("dispatch task child", error));
    }
    job.set_workflow_name(loop_record.pinned.definition.name().to_owned());
    job.set_step_label("Source capture".to_owned());
    tokio::spawn(workflows::execute_run(
        state.clone(),
        WorkflowJob {
            run_id: child_id,
            session_id: session,
            project_id: authority.as_ref().map(|authority| authority.project_id),
            agent_id: loop_record.agent_id,
            agent_revision: authority
                .as_ref()
                .map(|authority| authority.revision)
                .unwrap_or(current.revision),
            conversation_id: Some(started.id),
            authority: authority.clone(),
            project_free_authority: project_free.clone(),
            grant_alias: authority
                .as_ref()
                .map(|authority| authority.grant_alias.clone())
                .unwrap_or_default(),
            grant_access: authority
                .as_ref()
                .map(|authority| authority.grant_access)
                .unwrap_or(AccessMode::ReadWrite),
            connection,
            phase_providers: phase_models
                .iter()
                .map(|phase| phase.selection.provider)
                .collect(),
            active_connection: std::sync::Arc::new(std::sync::Mutex::new(None)),
            host_policy: authority
                .as_ref()
                .map(|authority| authority.policy.clone())
                .or_else(|| {
                    project_free
                        .as_ref()
                        .map(|authority| authority.policy.clone())
                })
                .expect("workflow authority"),
            turns: Vec::new(),
            job: job.clone(),
            eligible_reply: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
            task_loop: Some(loop_id),
        },
        None,
        execution,
    ));
    Ok(responses::command_navigation(&format!(
        "/conversations/{}",
        started.id.as_hex()
    )))
}

#[allow(clippy::too_many_arguments)]
async fn launch_view(
    state: &AppState,
    record: &ConversationRecord,
    workflow_raw: Option<&str>,
    target_raw: Option<&str>,
    brief: &str,
    commit_policy_raw: &str,
    plan_raw: &str,
    task_document: &str,
    task_revision: &str,
    task_hash: &str,
    task_index: &str,
    phase_raw: &[String],
    error: &'static str,
) -> WorkflowLaunchView {
    let records: Vec<_> = state
        .workflows
        .list()
        .into_iter()
        .filter(|record| {
            // The retired saved-plan starter stays resolvable for recorded runs
            // but never appears as a choice beside the six reference workflows.
            !(record.definition.name() == "Implement a saved plan"
                && record
                    .definition
                    .launch_input_sources()
                    .contains(&LaunchInputSource::SavedPlan))
        })
        .filter(|record| {
            task_index.is_empty()
                || (record.definition.execution_mode() == ExecutionMode::Once
                    && workflows::run::supports_task_execution(&record.definition))
        })
        .collect();
    let mut ranked: Vec<_> = records
        .into_iter()
        .map(|record| {
            let name = record.definition.name();
            let rank = starter_rank(name);
            (rank, name.to_lowercase(), record)
        })
        .collect();
    ranked.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.id.cmp(&right.2.id))
    });
    let records: Vec<_> = ranked.into_iter().map(|(_, _, record)| record).collect();
    let selected_workflow = workflow_raw.unwrap_or_default().trim().to_owned();
    let selection_available = records.iter().any(|record| {
        WorkflowSelection {
            workflow_id: record.id,
            definition_version: record.definition_version,
        }
        .as_token()
            == selected_workflow
    });
    let error = if !selected_workflow.is_empty() && !selection_available && error.is_empty() {
        "This workflow selection is no longer available. Choose a current process."
    } else {
        error
    };
    let mode = WorkflowSelection::parse(&selected_workflow)
        .and_then(|selection| state.workflows.resolve(&selection).ok())
        .map(|resolved| resolved.pinned.definition.execution_mode())
        .unwrap_or(ExecutionMode::Once);
    let task_document =
        if mode == ExecutionMode::Once && task_index.is_empty() && task_document.contains('/') {
            ""
        } else {
            task_document
        };
    let preview = if mode == ExecutionMode::TaskList && !task_document.is_empty() {
        resolve_task_list_snapshot(state, record, task_document, task_revision, task_hash)
            .map(|snapshot| snapshot.markdown)
    } else {
        resolve_selected_task(
            state,
            record,
            task_document,
            task_revision,
            task_hash,
            task_index,
        )
        .map(|task| {
            task.map(|task| format!("Task {}\n\n{}", task.index + 1, task.markdown))
                .unwrap_or_default()
        })
    };
    let error = if error.is_empty() {
        preview.as_ref().err().copied().unwrap_or(error)
    } else {
        error
    };
    let task_preview = preview.unwrap_or_default();
    let workflows: Vec<WorkflowOption> = records
        .iter()
        .map(|record| {
            let selection = WorkflowSelection {
                workflow_id: record.id,
                definition_version: record.definition_version,
            };
            let selected = selected_workflow == selection.as_token();
            let resolved_policy = selected
                .then(|| CommitPolicy::parse(commit_policy_raw.trim()))
                .flatten()
                .and_then(|policy| record.definition.with_commit_policy(policy).ok());
            let definition = resolved_policy.as_ref().unwrap_or(&record.definition);
            WorkflowOption {
                token: selection.as_token(),
                name: record.definition.name().to_owned(),
                summary: starter_summary(record.definition.name())
                    .unwrap_or_else(|| workflows::summary::process_summary(definition)),
                process_phases: workflows::summary::process_overview(definition),
                approvals: workflows::summary::approval_stops(definition),
                selected,
            }
        })
        .collect();
    let selected_policy = WorkflowSelection::parse(&selected_workflow)
        .and_then(|selection| state.workflows.resolve(&selection).ok())
        .map(|resolved| resolved.pinned.definition)
        .and_then(|definition| {
            let choices = definition.commit_policy_choices();
            let requested = CommitPolicy::parse(commit_policy_raw.trim());
            requested
                .filter(|policy| choices.contains(policy))
                .or_else(|| choices.first().copied())
        });
    let commit_policies = WorkflowSelection::parse(&selected_workflow)
        .and_then(|selection| state.workflows.resolve(&selection).ok())
        .map(|resolved| resolved.pinned.definition)
        .map(|definition| {
            let selected = selected_policy;
            definition
                .commit_policy_choices()
                .into_iter()
                .filter(|policy| *policy != CommitPolicy::NoCommit)
                .filter(|policy| task_index.is_empty() || *policy == CommitPolicy::HumanApproval)
                .map(|policy| CommitPolicyOption {
                    value: policy.as_str().to_owned(),
                    label: policy.label().to_owned(),
                    detail: match policy {
                        CommitPolicy::NoCommit => "The workflow stops without changing project files.".to_owned(),
                        CommitPolicy::HumanApproval => "The exact candidate waits for a human decision before commit.".to_owned(),
                        CommitPolicy::AutomaticAfterReview => "An approved exact-candidate review permits commit without a human gate.".to_owned(),
                    },
                    selected: selected == Some(policy),
                })
                .collect()
        })
        .unwrap_or_default();
    let directory_launch = WorkflowSelection::parse(&selected_workflow)
        .and_then(|selection| state.workflows.resolve(&selection).ok())
        .is_some_and(|resolved| uses_conversation_directories(&resolved.pinned.definition));
    let requested_target = target_raw.filter(|raw| !directory_launch && !raw.trim().is_empty());
    let selected_target = match requested_target {
        Some(raw) => ProjectId::parse(raw.trim()),
        None => record.execution_target,
    };
    let (plans, requires_plan, plan_optional) =
        selected_plan_options(state, record, &selected_workflow, plan_raw);
    let (task_lists, requires_task_list) =
        selected_task_list_options(state, record, &selected_workflow, task_document);
    let mut targets: Vec<TargetOption> = record
        .grants
        .iter()
        .filter(|_| !directory_launch)
        .filter_map(|grant| {
            let project = state.projects.get(&grant.project_id)?;
            Some(TargetOption {
                id: grant.project_id.as_hex(),
                name: project.name.clone(),
                access: target_access_summary(state, record, grant.project_id),
                selected: selected_target == Some(grant.project_id),
            })
        })
        .collect();
    let target_unavailable =
        requested_target.is_some() && !targets.iter().any(|target| target.selected);
    if target_unavailable {
        targets.push(TargetOption {
            id: requested_target.unwrap_or_default().to_owned(),
            name: "Selected project is unavailable".to_owned(),
            access: "Choose an available granted project".to_owned(),
            selected: true,
        });
    }
    let error = if target_unavailable && error.is_empty() {
        "The selected project is unavailable. Choose an available granted project."
    } else {
        error
    };
    let (model_summary, access_summary, environment_summary) =
        launch_readiness(state, record, selected_target, &selected_workflow).await;
    let phase_models = selected_phase_model_options(state, record, &selected_workflow, phase_raw);
    let available_plans = plans.iter().any(|plan| !plan.content_hash.is_empty());
    let available_task_lists = task_lists.iter().any(|list| !list.content_hash.is_empty());
    let selected_plan = plans
        .iter()
        .any(|plan| plan.selected && !plan.content_hash.is_empty());
    let selected_task_list = task_lists
        .iter()
        .any(|list| list.selected && !list.content_hash.is_empty());
    let input_summary = match mode {
        ExecutionMode::TaskList if !available_task_lists => {
            "This process needs a task list. Save one in this conversation first.".to_owned()
        }
        ExecutionMode::TaskList if !selected_task_list => {
            "Select a task list before you start.".to_owned()
        }
        ExecutionMode::TaskList => {
            "Task list selected. Each remaining task runs the pinned process once.".to_owned()
        }
        ExecutionMode::Once if requires_plan && !available_plans => {
            "This process needs a saved plan. Save one in this conversation first.".to_owned()
        }
        ExecutionMode::Once if requires_plan && !selected_plan => {
            "Select a saved plan before you start.".to_owned()
        }
        ExecutionMode::Once if requires_plan => {
            "Saved plan selected. This process runs once.".to_owned()
        }
        ExecutionMode::Once if plan_optional && selected_plan => {
            "Saved plan selected. The selected plan becomes task direction for this run.".to_owned()
        }
        ExecutionMode::Once => "This process runs once. It does not need a task list.".to_owned(),
    };
    let launch_blocked = workflows.is_empty()
        || target_unavailable
        || (!directory_launch && !targets.iter().any(|target| target.selected))
        || (requires_task_list && !available_task_lists)
        || (requires_plan && !available_plans);
    WorkflowLaunchView {
        stage: if selection_available {
            "inputs"
        } else {
            "choose"
        },
        document_title: format!("Workflows · {}{}", record.title, TITLE_SUFFIX),
        conversation_id: record.id.as_hex(),
        revision: record.revision.to_string(),
        brief: if brief.is_empty() && (requires_task_list || !task_document.is_empty()) {
            "Implement only the assigned task from the selected task list.".to_owned()
        } else if brief.is_empty() {
            default_brief(record)
        } else {
            // The textarea renders flush, but navigation round-trips a leading
            // newline; trim ends like launch validation so Back never grows one.
            brief.trim().to_owned()
        },
        workflows,
        targets,
        directory_launch,
        plans,
        requires_plan,
        plan_optional,
        requires_task_list,
        task_lists,
        task_document: task_document.to_owned(),
        task_revision: task_revision.to_owned(),
        task_hash: task_hash.to_owned(),
        task_index: task_index.to_owned(),
        task_preview,
        commit_policies,
        phase_models,
        model_summary,
        access_summary,
        access_preview: String::new(),
        phase_summaries: Vec::new(),
        environment_summary,
        input_summary,
        launch_blocked,
        error,
    }
}

async fn launch_readiness(
    state: &AppState,
    record: &ConversationRecord,
    target: Option<ProjectId>,
    workflow: &str,
) -> (String, String, String) {
    let model_summary = super::effective_model(state, record).map_or_else(
        || "No model selected".to_owned(),
        |model| {
            let effort = model
                .settings
                .model
                .thinking
                .as_ref()
                .map(|effort| format!(" · Thinking: {}", effort.label()))
                .unwrap_or_default();
            format!(
                "{} · {}{}",
                model.settings.model.provider.label(),
                model.settings.model.model,
                effort
            )
        },
    );
    if WorkflowSelection::parse(workflow)
        .and_then(|selection| state.workflows.resolve(&selection).ok())
        .is_some_and(|resolved| uses_conversation_directories(&resolved.pinned.definition))
    {
        let Some(model) = super::effective_model(state, record) else {
            return (
                model_summary,
                "No execution settings".to_owned(),
                "Choose a model in conversation Settings.".to_owned(),
            );
        };
        let settings = &model.settings;
        let directories = settings
            .directories
            .iter()
            .map(|grant| {
                format!(
                    "{} → {} ({})",
                    grant.host_path.display(),
                    grant.guest_path(),
                    match grant.access {
                        crate::execution::DirectoryAccess::ReadOnly => "Read only",
                        crate::execution::DirectoryAccess::ReviewBeforeApply =>
                            "Review before apply",
                        crate::execution::DirectoryAccess::DirectWrite => "Direct write",
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let access = if settings.location == crate::execution::ToolLocation::Host {
            let locations = settings
                .directories
                .iter()
                .map(|grant| grant.host_path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "This computer · Unrestricted host access · {} · Work locations: {}",
                crate::slices::execution_settings::page::host_approval_label(
                    settings.host_approval
                ),
                if locations.is_empty() {
                    "None selected"
                } else {
                    &locations
                }
            )
        } else {
            format!(
                "{} · Tools: {} · Sandbox network: {}",
                if directories.is_empty() {
                    "Private scratch at /workspace"
                } else {
                    &directories
                },
                settings
                    .tools
                    .iter()
                    .map(|tool| tool.label())
                    .collect::<Vec<_>>()
                    .join(", "),
                network_label(&settings.network)
            )
        };
        let Some(selection) = WorkflowSelection::parse(workflow) else {
            return (
                model_summary,
                access,
                "Choose a workflow to preview its environment.".to_owned(),
            );
        };
        let definition = match state.workflows.resolve(&selection) {
            Ok(resolved) => match resolved
                .pinned
                .definition
                .with_conversation_settings(settings)
            {
                Ok(definition) => definition,
                Err(error) => return (model_summary, access, error.message().to_owned()),
            },
            Err(error) => return (model_summary, access, error.message().to_owned()),
        };
        let captures_source = definition.steps().iter().any(|step| {
            step.inputs.iter().any(|input| {
                matches!(
                    input.source,
                    workflows::definition::ArtefactSource::RunInitialCandidate
                        | workflows::definition::ArtefactSource::RunCurrentCandidate
                )
            })
        });
        let reviewed = settings
            .directories
            .iter()
            .any(|grant| grant.access == crate::execution::DirectoryAccess::ReviewBeforeApply);
        let exclusions = settings
            .directories
            .iter()
            .filter(|grant| {
                captures_source
                    && (!reviewed
                        || grant.access == crate::execution::DirectoryAccess::ReviewBeforeApply)
            })
            .flat_map(|grant| {
                workflows::workspace::reviewed_capture_exclusions(
                    &grant.host_path,
                    state.local_data.root(),
                )
                .into_iter()
                .map(|path| grant.host_path.join(path).display().to_string())
            })
            .collect::<Vec<_>>();
        let access = if exclusions.is_empty() {
            access
        } else {
            format!(
                "{access}. The source snapshot excludes these engine paths: {}",
                exclusions.join(", ")
            )
        };
        let environment = match workflows::preview_environments(
            &definition,
            &state.environments,
            &state.environment_snapshots,
        )
        .await
        {
            Ok(preview) => format!(
                "Ready: {}",
                preview
                    .environments
                    .iter()
                    .map(|item| item.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Err(error) => error.message().to_owned(),
        };
        return (model_summary, access, environment);
    }
    let Some(target) = target else {
        return (
            model_summary,
            "No explicit Git destination".to_owned(),
            "Select a granted project for this process.".to_owned(),
        );
    };
    let mut selected = record.clone();
    selected.execution_target = Some(target);
    let authority = match crate::conversations::resolve_workflow_authority(
        &selected,
        &state.projects,
        &state.agents,
    ) {
        Ok(Some(authority)) => authority.effective,
        Ok(None) => {
            return (
                model_summary,
                "No execution authority".to_owned(),
                "The selected target has no grant.".to_owned(),
            );
        }
        Err(error) => {
            return (
                model_summary,
                error.message().to_owned(),
                "The selected target is not ready.".to_owned(),
            );
        }
    };
    let access_summary = format!(
        "{} · Tools: {} · Network: {}",
        access_label(authority.grant_access),
        authority
            .tools
            .iter()
            .map(|tool| tool.label())
            .collect::<Vec<_>>()
            .join(", "),
        network_label(&authority.network),
    );
    let Some(selection) = WorkflowSelection::parse(workflow) else {
        return (
            model_summary,
            access_summary,
            "Choose a workflow to check its environments.".to_owned(),
        );
    };
    let resolved = match state.workflows.resolve(&selection) {
        Ok(resolved) => resolved,
        Err(error) => return (model_summary, access_summary, error.message().to_owned()),
    };
    if !workflows::definition_fits_agent(
        &resolved.pinned.definition,
        &authority.tools,
        &authority
            .policy
            .grants()
            .iter()
            .map(|grant| (grant.alias.clone(), grant.access))
            .collect::<Vec<_>>(),
        &authority.grant_alias,
    ) {
        return (
            model_summary,
            access_summary,
            "The workflow needs access outside the effective ceiling.".to_owned(),
        );
    }
    match workflows::preview_environments(
        &resolved.pinned.definition,
        &state.environments,
        &state.environment_snapshots,
    )
    .await
    {
        Ok(preview) => {
            let names = preview
                .environments
                .iter()
                .map(|environment| environment.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            (model_summary, access_summary, format!("Ready: {names}"))
        }
        Err(error) => (model_summary, access_summary, error.message().to_owned()),
    }
}

fn phase_choice_token(
    step: &str,
    selection: &ModelSelection,
    preset: Option<&crate::presets::PresetRecord>,
) -> String {
    serde_json::to_string(&PhaseChoiceToken {
        step: step.to_owned(),
        provider: selection.provider.as_str().to_owned(),
        model: selection.model.clone(),
        thinking: selection
            .thinking
            .as_ref()
            .map(|effort| effort.as_str().to_owned()),
        preset: preset.map(|record| record.id.as_hex()),
        preset_revision: preset.map(|record| record.revision),
    })
    .expect("phase model token")
}

fn parse_phase_choice(
    raw: &str,
    step: &str,
    presets: &[crate::presets::PresetRecord],
) -> Result<PhaseModelSelection, &'static str> {
    let token: PhaseChoiceToken =
        serde_json::from_str(raw).map_err(|_| "Choose a model for every model phase.")?;
    if token.step != step {
        return Err("Choose a model for every model phase.");
    }
    let provider = ProviderKind::parse(&token.provider)
        .ok_or("Choose an available provider for every model phase.")?;
    let thinking = token
        .thinking
        .map(|value| ThinkingEffort::new(value).ok_or("Choose an available thinking effort."))
        .transpose()?;
    let selection = ModelSelection::new(provider, token.model, thinking)
        .ok_or("Choose a valid model for every model phase.")?;
    let preset = match (token.preset, token.preset_revision) {
        (None, None) => None,
        (Some(id), Some(revision)) => {
            let id = crate::presets::PresetId::parse(&id).ok_or("Choose an available preset.")?;
            let record = presets
                .iter()
                .find(|record| record.id == id && record.revision == revision)
                .ok_or("That preset changed. Reload the launch sheet.")?;
            if record.settings.model != selection {
                return Err("Use the model selected by that preset.");
            }
            Some(PinnedPreset {
                id: record.id,
                revision: record.revision,
                name: record.name.clone(),
            })
        }
        _ => return Err("Choose an available preset."),
    };
    let settings = preset
        .as_ref()
        .and_then(|pinned| presets.iter().find(|record| record.id == pinned.id))
        .map(|record| record.settings.clone());
    let instructions = settings
        .as_ref()
        .map(|settings| settings.instructions.clone())
        .unwrap_or_default();
    Ok(PhaseModelSelection {
        step: workflows::definition::StepKey::parse(step)
            .map_err(|_| "Choose a valid workflow phase.")?,
        selection,
        instructions,
        preset,
        settings,
    })
}

fn phase_steps(
    definition: &workflows::definition::WorkflowDefinition,
) -> Vec<&workflows::definition::StepDefinition> {
    definition
        .steps()
        .iter()
        .filter(|step| matches!(&step.action, workflows::definition::StepAction::Agent(_)))
        .collect()
}

fn selected_phase_model_options(
    state: &AppState,
    record: &ConversationRecord,
    workflow_raw: &str,
    phase_raw: &[String],
) -> Vec<PhaseModelOption> {
    let Some(selection) = WorkflowSelection::parse(workflow_raw) else {
        return Vec::new();
    };
    let Ok(resolved) = state.workflows.resolve(&selection) else {
        return Vec::new();
    };
    let presets = state.presets.list();
    let selected = super::effective_model(state, record).map(|model| model.settings.model);
    let direct_models: Vec<ModelSelection> = state
        .preferences
        .desk_providers(&state.vault)
        .into_iter()
        .filter_map(|provider| {
            let thinking = state.models_dev.effective_effort(
                provider.kind,
                &provider.model,
                provider.thinking.as_ref(),
            );
            ModelSelection::new(provider.kind, provider.model, thinking)
        })
        .chain(selected.clone())
        .fold(Vec::new(), |mut models, model| {
            if !models.contains(&model) {
                models.push(model);
            }
            models
        });
    phase_steps(&resolved.pinned.definition)
        .into_iter()
        .map(|step| {
            let mut choices = Vec::new();
            if let Some(defaults) = super::effective_model(state, record) {
                let workflows::definition::StepAction::Agent(action) = &step.action else {
                    unreachable!()
                };
                let effective = action.settings.resolve(&defaults.settings);
                choices.push(PhaseChoice {
                    value: phase_choice_token(step.key.as_str(), &effective.model, None),
                    label: if action.settings.is_same_as_defaults() {
                        "Same as run defaults"
                    } else {
                        "Saved phase settings"
                    }
                    .to_owned(),
                    detail: format!(
                        "{} · {}",
                        effective.model.provider.label(),
                        effective.model.model
                    ),
                    selected: true,
                });
            }
            for selection in &direct_models {
                let value = phase_choice_token(step.key.as_str(), selection, None);
                if choices.iter().any(|choice| choice.value == value) {
                    continue;
                }
                choices.push(PhaseChoice {
                    value,
                    label: format!(
                        "Direct model · {} · {}",
                        selection.provider.label(),
                        selection.model
                    ),
                    detail: selection
                        .thinking
                        .as_ref()
                        .map(|effort| format!("Thinking: {}", effort.label()))
                        .unwrap_or_else(|| {
                            "Phase instructions and access stay unchanged".to_owned()
                        }),
                    selected: false,
                });
            }
            for preset in &presets {
                let selection = &preset.settings.model;
                choices.push(PhaseChoice {
                    value: phase_choice_token(step.key.as_str(), selection, Some(preset)),
                    label: format!("Preset · {}", preset.name),
                    detail: format!(
                        "{} · {}{}",
                        selection.provider.label(),
                        selection.model,
                        if preset.settings.instructions.is_empty() {
                            String::new()
                        } else {
                            " · Saved instructions".to_owned()
                        }
                    ),
                    selected: false,
                });
            }
            let raw = phase_raw.iter().find(|raw| {
                serde_json::from_str::<PhaseChoiceToken>(raw)
                    .is_ok_and(|token| token.step == step.key.as_str())
            });
            if let Some(raw) = raw {
                for choice in &mut choices {
                    choice.selected = choice.value == *raw;
                }
                if !choices.iter().any(|choice| choice.selected) {
                    choices.push(PhaseChoice {
                        value: raw.clone(),
                        label: "Selected model or preset is unavailable".to_owned(),
                        detail: "Choose an available model or reload workflow setup".to_owned(),
                        selected: true,
                    });
                }
            } else if !choices.iter().any(|choice| choice.selected)
                && let Some(first) = choices.first_mut()
            {
                first.selected = true;
            }
            PhaseModelOption {
                step: step.key.as_str().to_owned(),
                name: step.name.clone(),
                choices,
            }
        })
        .collect()
}

fn resolve_directory_phase_settings(
    definition: &workflows::definition::WorkflowDefinition,
    defaults: &crate::execution::ExecutionSettings,
    phases: &mut [PhaseModelSelection],
) -> Result<(), &'static str> {
    let mut identities: Vec<crate::execution::DirectoryGrant> = Vec::new();
    for phase in phases.iter_mut() {
        let step = definition
            .step(&phase.step)
            .ok_or("Choose a valid workflow phase.")?;
        let workflows::definition::StepAction::Agent(action) = &step.action else {
            return Err("Choose a model only for model phases.");
        };
        let mut resolved = action.settings.resolve(defaults);
        if let Some(launch) = phase.settings.take() {
            resolved = launch;
        }
        resolved.model = phase.selection.clone();
        if phase.instructions.is_empty() {
            phase.instructions = resolved.instructions.clone();
        } else {
            resolved.instructions = phase.instructions.clone();
        }
        if resolved
            .directories
            .iter()
            .any(|grant| grant.revalidate().is_err())
        {
            return Err("A phase directory is unavailable at its saved identity.");
        }
        if crate::execution::validate_directories(&resolved.directories).is_err() {
            return Err("Choose existing, distinct directories without overlapping roots.");
        }
        for grant in &mut resolved.directories {
            if let Some(existing) = identities
                .iter()
                .find(|existing| existing.identity == grant.identity)
            {
                grant.id = existing.id;
                grant.alias.clone_from(&existing.alias);
            } else {
                let alias = grant.alias.clone();
                let mut suffix = 2;
                while identities
                    .iter()
                    .any(|existing| existing.alias == grant.alias)
                {
                    grant.alias = format!("{}-{suffix}", &alias[..alias.len().min(28)]);
                    suffix += 1;
                }
                identities.push(grant.clone());
            }
        }
        crate::execution::validate_directories(&identities)
            .map_err(|_| "Choose distinct directories without overlapping roots across phases.")?;
        phase.settings = Some(resolved);
    }
    Ok(())
}

fn merged_phase_settings(
    defaults: &crate::execution::ExecutionSettings,
    phases: &[PhaseModelSelection],
) -> crate::execution::ExecutionSettings {
    crate::execution::ExecutionSettings::combined(
        phases.iter().filter_map(|phase| phase.settings.as_ref()),
    )
    .unwrap_or_else(|| defaults.clone())
}

async fn preview_phase_access(
    state: &AppState,
    session: crate::sessions::SessionId,
    record: &ConversationRecord,
    definition: &workflows::definition::WorkflowDefinition,
    phases: &mut [PhaseModelSelection],
    view: &mut WorkflowLaunchView,
) -> Result<(), &'static str> {
    let defaults = super::effective_model(state, record)
        .ok_or("Choose a conversation model before review.")?
        .settings;
    resolve_directory_phase_settings(definition, &defaults, phases)?;
    let snapshots: Vec<_> = phases
        .iter()
        .filter_map(|phase| phase.settings.clone())
        .collect();
    let keyed: Vec<_> = phases
        .iter()
        .zip(&snapshots)
        .map(|(phase, settings)| (phase.step.clone(), settings.clone()))
        .collect();
    let pinned = definition
        .with_phase_settings(&defaults, &keyed)
        .map_err(|error| error.message())?;
    let environments =
        workflows::preview_environments(&pinned, &state.environments, &state.environment_snapshots)
            .await;
    for (phase, settings) in phases.iter().zip(&snapshots) {
        let name = definition
            .step(&phase.step)
            .map(|step| step.name.as_str())
            .unwrap_or(phase.step.as_str());
        let host = settings.location == crate::execution::ToolLocation::Host;
        view.phase_summaries.push(format!(
            "{name}: {} · {} · Tools: {} · {}{}",
            settings.model.provider.label(),
            settings.model.model,
            settings
                .tools
                .iter()
                .map(|tool| tool.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            if host {
                format!(
                    "This computer · {}",
                    crate::slices::execution_settings::page::host_approval_label(
                        settings.host_approval,
                    )
                )
            } else {
                format!(
                    "Network: {} · Environment: {}",
                    network_label(&settings.network),
                    settings.environment.as_hex()
                )
            },
            if workflows::definition::additional_access(&defaults, settings) {
                " · Additional access"
            } else {
                ""
            }
        ));
        if host {
            view.phase_summaries
                .push(crate::execution::HostIdentity::current().authority_summary());
            view.phase_summaries.push(
                "Unrestricted host access. Work locations do not confine commands. Approval covers the submitted shell request, not script internals. Output is sent to the hosted model. Host commands can alter live configuration and credentials. Failure and cancellation do not undo host changes.".to_owned(),
            );
            if settings.host_approval.automatic() {
                view.phase_summaries.push(
                    "This run authorises Run without approval for host commands. Copied conversation consent does not apply.".to_owned(),
                );
            } else {
                view.phase_summaries.push(
                    "Each host command waits for approval bound to this run, step and attempt."
                        .to_owned(),
                );
            }
        }
        let writes = definition
            .step(&phase.step)
            .is_some_and(|step| step.writes_primary_source());
        for grant in &settings.directories {
            if host {
                view.phase_summaries
                    .push(format!("{} · Work location", grant.host_path.display()));
                continue;
            }
            view.phase_summaries.push(format!(
                "{} → {} · {}",
                grant.host_path.display(),
                grant.guest_path(),
                match grant.access {
                    crate::execution::DirectoryAccess::ReadOnly => "Read only",
                    crate::execution::DirectoryAccess::DirectWrite => "Direct write",
                    crate::execution::DirectoryAccess::ReviewBeforeApply if writes => {
                        "Review before apply"
                    }
                    crate::execution::DirectoryAccess::ReviewBeforeApply => {
                        "Read only · isolated reviewed copy"
                    }
                }
            ));
            if grant.access == crate::execution::DirectoryAccess::DirectWrite {
                view.phase_summaries.push("Direct write changes host files immediately. Candidate approval does not cover these changes. Failure, discard and cancellation leave them intact.".to_owned());
            }
            if grant.access == crate::execution::DirectoryAccess::ReviewBeforeApply {
                for excluded in crate::workflows::workspace::reviewed_capture_exclusions(
                    &grant.host_path,
                    state.local_data.root(),
                ) {
                    view.phase_summaries.push(format!(
                        "Excluded from capture and application: {}",
                        grant.host_path.join(excluded).display()
                    ));
                }
            }
            if crate::execution::authority::sensitive_directory(
                &grant.host_path,
                state.local_data.root(),
            ) {
                view.phase_summaries.push(format!("Sensitive access: this root can expose provider credentials and private conversations, even with Network off. Power Plant data: {}.", state.local_data.root().display()));
                if grant.access == crate::execution::DirectoryAccess::DirectWrite {
                    view.phase_summaries.push("Direct write can alter or corrupt live configuration, permissions and execution evidence.".to_owned());
                }
                if grant.access == crate::execution::DirectoryAccess::ReviewBeforeApply {
                    view.phase_summaries.push(
                        "Reviewed access can propose changes to Power Plant configuration."
                            .to_owned(),
                    );
                }
            }
        }
    }
    view.environment_summary = if snapshots
        .iter()
        .all(|settings| settings.location == crate::execution::ToolLocation::Host)
    {
        "Host steps need no sandbox environment.".to_owned()
    } else {
        match environments {
            Ok(_) => "Selected phase environments are ready.".to_owned(),
            Err(error) => error.message().to_owned(),
        }
    };
    view.access_preview = state
        .access_consent
        .request_launch(session, record.id, snapshots)
        .map_err(|_| "The access preview is unavailable. Reload workflow setup.")?;
    Ok(())
}

fn starter_rank(name: &str) -> usize {
    match name {
        "Plan a change" => 1,
        "Review current code" => 2,
        "Implement with approval" => 3,
        "Implement and review" => 4,
        "Plan then implement" => 5,
        "Task loop" => 6,
        _ => usize::MAX,
    }
}

fn starter_summary(name: &str) -> Option<String> {
    match name {
        "Plan a change" => Some("A focused plan for the proposed change."),
        "Review current code" => Some("An assessment with actionable feedback."),
        "Implement with approval" => Some("Prepare a change for your review."),
        "Implement and review" => Some("A fresh reviewer inspects the change before you decide."),
        "Plan then implement" => Some("Approve a plan before implementation starts."),
        "Task loop" => Some("Complete each remaining task with a fresh context."),
        _ => None,
    }
    .map(str::to_owned)
}

fn uses_conversation_directories(definition: &workflows::definition::WorkflowDefinition) -> bool {
    !definition.steps().iter().any(|step| {
        matches!(&step.action,
            workflows::definition::StepAction::SystemCommand(action)
                if action.command != workflows::commands::SystemCommandId::ApplyChanges)
    })
}

fn resolve_phase_models(
    state: &AppState,
    definition: &workflows::definition::WorkflowDefinition,
    phase_raw: &[String],
) -> Result<Vec<PhaseModelSelection>, &'static str> {
    let presets = state.presets.list();
    let mut models = Vec::new();
    for step in phase_steps(definition) {
        let mut matches = phase_raw.iter().filter(|raw| {
            serde_json::from_str::<PhaseChoiceToken>(raw)
                .is_ok_and(|token| token.step == step.key.as_str())
        });
        let Some(raw) = matches.next() else {
            return Err("Choose a model for every model phase.");
        };
        if matches.next().is_some() {
            return Err("Choose one model for every model phase.");
        }
        let model = parse_phase_choice(raw, step.key.as_str(), &presets)?;
        super::valid_selection(state, &model.selection)?;
        models.push(model);
    }
    if models.len() != phase_raw.len() {
        return Err("Choose a model only for the selected workflow phases.");
    }
    Ok(models)
}

fn validate_phase_models(
    _state: &AppState,
    definition: &workflows::definition::WorkflowDefinition,
    base: &crate::agents::EffectiveAuthority,
    models: &[PhaseModelSelection],
) -> Result<(), &'static str> {
    for model in models {
        let step = definition
            .step(&model.step)
            .ok_or("Choose a valid workflow phase.")?;
        let authority = if let Some(settings) = &model.settings {
            if settings.environment != definition.effective_environment(step) {
                return Err(
                    "That preset requests a different environment. Choose the workflow environment before launch.",
                );
            }
            crate::conversations::apply_settings_ceiling(base, settings)
                .map_err(|_| "That preset requests access outside the conversation settings.")?
        } else {
            base.clone()
        };
        let workflows::definition::StepAction::Agent(action) = &step.action else {
            return Err("Choose a model only for model phases.");
        };
        if action.candidate_authority.access().is_writable()
            && !authority.grant_access.is_writable()
            || !action
                .authority
                .allowed_by(&authority.tools, authority.directories())
        {
            return Err("That phase needs access outside the selected model ceiling.");
        }
    }
    Ok(())
}

fn selected_plan_options(
    state: &AppState,
    record: &ConversationRecord,
    workflow_raw: &str,
    plan_raw: &str,
) -> (Vec<PlanOption>, bool, bool) {
    let resolved = WorkflowSelection::parse(workflow_raw)
        .and_then(|selection| state.workflows.resolve(&selection).ok());
    let Some(resolved) = resolved else {
        return (Vec::new(), false, false);
    };
    if resolved.pinned.definition.execution_mode() == ExecutionMode::TaskList {
        return (Vec::new(), false, false);
    }
    // Workflows that declare a saved-plan input require one. Every other
    // single-run workflow offers the conversation plans as an optional input:
    // the launch pins the input per run, so the stored definition is unchanged.
    let requires_plan = resolved
        .pinned
        .definition
        .launch_input_sources()
        .contains(&LaunchInputSource::SavedPlan);
    let mut plans: Vec<_> = state
        .documents
        .list_for_conversation(record.id)
        .into_iter()
        .filter(|document| document.kind == crate::conversations::DocumentKind::Plan)
        .map(|document| {
            let revision = document
                .revisions
                .iter()
                .find(|revision| plan_choice_token(&document.id, revision) == plan_raw)
                .unwrap_or_else(|| document.current());
            let title = document.revision_title(revision.revision).to_owned();
            PlanOption {
                value: plan_choice_token(&document.id, revision),
                title,
                revision: revision.revision.to_string(),
                content_hash: revision.content_hash.as_str(),
                content_bytes: revision.content_bytes.to_string(),
                selected: false,
            }
        })
        .collect();
    if let Some(selected) = plans.iter_mut().find(|plan| plan.value == plan_raw) {
        selected.selected = true;
    } else if !plan_raw.is_empty() {
        plans.push(PlanOption {
            value: plan_raw.to_owned(),
            title: "Selected plan is unavailable".to_owned(),
            revision: String::new(),
            content_hash: String::new(),
            content_bytes: String::new(),
            selected: true,
        });
    }
    (plans, requires_plan, !requires_plan)
}

fn default_brief(record: &ConversationRecord) -> String {
    record
        .messages
        .iter()
        .find(|message| message.role == crate::conversations::MessageRole::User)
        .map(|message| truncate_to_brief(message.text.trim()))
        .unwrap_or_default()
}

fn truncate_to_brief(text: &str) -> String {
    const LIMIT: usize = workflows::input_context::MAXIMUM_LAUNCH_BRIEF_BYTES;
    if text.len() <= LIMIT {
        return text.to_owned();
    }
    let mut end = LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].trim_end().to_owned()
}

/// Resolve the definition a plan selection launches against. A selected plan
/// for a workflow without a declared saved-plan input pins that input per run,
/// so Prepare implementation reaches an existing reference workflow and a
/// launch without a plan keeps the stored definition unchanged.
fn definition_for_plan(
    state: &AppState,
    workflow_raw: &str,
    plan_raw: &str,
) -> Result<Option<workflows::definition::WorkflowDefinition>, &'static str> {
    if plan_raw.trim().is_empty() {
        return Ok(None);
    }
    let Some(selection) = WorkflowSelection::parse(workflow_raw.trim()) else {
        return Err("Choose a current workflow from the catalogue.");
    };
    let resolved = match state.workflows.resolve(&selection) {
        Ok(resolved) => resolved,
        Err(error) => return Err(error.message()),
    };
    if resolved.pinned.definition.execution_mode() == ExecutionMode::TaskList {
        return Err("This workflow does not declare a saved plan input.");
    }
    if resolved
        .pinned
        .definition
        .launch_input_sources()
        .contains(&LaunchInputSource::SavedPlan)
    {
        return Ok(Some(resolved.pinned.definition.clone()));
    }
    match resolved.pinned.definition.with_saved_plan_input() {
        Ok(definition) => Ok(Some(definition)),
        Err(_) => Err("That workflow cannot take a saved plan."),
    }
}

fn selected_task_list_options(
    state: &AppState,
    record: &ConversationRecord,
    workflow_raw: &str,
    selected_document: &str,
) -> (Vec<PlanOption>, bool) {
    let requires = WorkflowSelection::parse(workflow_raw)
        .and_then(|selection| state.workflows.resolve(&selection).ok())
        .is_some_and(|resolved| {
            resolved.pinned.definition.execution_mode() == ExecutionMode::TaskList
        });
    if !requires {
        return (Vec::new(), false);
    }
    let mut lists: Vec<_> = state
        .documents
        .list_for_conversation(record.id)
        .into_iter()
        .filter(|document| document.kind == crate::conversations::DocumentKind::TaskList)
        .map(|document| {
            let revision = document.current();
            let value = format!(
                "{}/{}/{}",
                document.id.as_hex(),
                revision.revision,
                revision.content_hash.as_str()
            );
            PlanOption {
                value: value.clone(),
                title: document.title.clone(),
                revision: revision.revision.to_string(),
                content_hash: revision.content_hash.as_str(),
                content_bytes: revision.content_bytes.to_string(),
                selected: value == selected_document.trim()
                    || document.id.as_hex() == selected_document.trim(),
            }
        })
        .collect();
    if !selected_document.trim().is_empty() && !lists.iter().any(|list| list.selected) {
        lists.push(PlanOption {
            value: selected_document.to_owned(),
            title: "Selected task list is unavailable".to_owned(),
            revision: String::new(),
            content_hash: String::new(),
            content_bytes: String::new(),
            selected: true,
        });
    }
    (lists, true)
}

pub(super) fn implementation_href(
    state: &AppState,
    document: &crate::conversations::PlanDocument,
    revision: &crate::conversations::PlanRevision,
) -> String {
    let Some(conversation) = document.associated_conversation else {
        return String::new();
    };
    // Prepare implementation hands the saved plan to the Implement and review
    // reference workflow. A retired saved-plan starter stays out of the
    // chooser but remains a fallback so older records keep their handoff.
    let workflows = state.workflows.list();
    let workflow = workflows
        .iter()
        .find(|workflow| {
            workflow.definition.execution_mode() == ExecutionMode::Once
                && workflow.definition.name() == "Implement and review"
        })
        .or_else(|| {
            workflows.iter().find(|workflow| {
                workflow.definition.execution_mode() == ExecutionMode::Once
                    && workflow
                        .definition
                        .launch_input_sources()
                        .contains(&LaunchInputSource::SavedPlan)
            })
        });
    let Some(workflow) = workflow else {
        return String::new();
    };
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            (
                "workflow",
                WorkflowSelection {
                    workflow_id: workflow.id,
                    definition_version: workflow.definition_version,
                }
                .as_token(),
            ),
            ("plan", plan_choice_token(&document.id, revision)),
            (
                "brief",
                format!(
                    "Implement the selected plan: {}",
                    document.revision_title(revision.revision)
                ),
            ),
        ])
        .finish();
    format!("/conversations/{conversation}/workflow?{query}")
}

fn plan_choice_token(
    document_id: &DocumentId,
    revision: &crate::conversations::PlanRevision,
) -> String {
    serde_json::to_string(&PlanChoiceToken {
        document: document_id.as_hex(),
        revision: revision.revision,
        content_hash: revision.content_hash.as_str(),
        object_hash: revision.object_hash.as_str(),
        artefact_hash: revision.artefact_hash.as_str(),
    })
    .expect("plan choice token")
}

fn resolve_selected_plan(
    state: &AppState,
    record: &ConversationRecord,
    raw: &str,
    definition: &workflows::definition::WorkflowDefinition,
) -> Result<Option<SelectedPlan>, &'static str> {
    let requires_plan = definition
        .launch_input_sources()
        .contains(&LaunchInputSource::SavedPlan);
    if !requires_plan {
        return if raw.trim().is_empty() {
            Ok(None)
        } else {
            Err("This workflow does not declare a saved plan input.")
        };
    }
    if raw.trim().is_empty() {
        return Err("Choose a saved plan before launch.");
    }
    let token: PlanChoiceToken =
        serde_json::from_str(raw).map_err(|_| "Choose an available saved plan.")?;
    let document_id =
        DocumentId::parse(&token.document).ok_or("Choose an available saved plan.")?;
    let content_hash = crate::workflows::artefacts::ObjectHash::parse(&token.content_hash)
        .ok_or("Choose an available saved plan.")?;
    let object_hash = crate::workflows::artefacts::ObjectHash::parse(&token.object_hash)
        .ok_or("Choose an available saved plan.")?;
    let artefact_hash = crate::workflows::artefacts::ArtefactHash::parse(&token.artefact_hash)
        .ok_or("Choose an available saved plan.")?;
    let document = state
        .documents
        .get(&document_id)
        .ok_or("That saved plan is no longer available.")?;
    if document.associated_conversation != Some(record.id) {
        return Err("That saved plan is not associated with this conversation.");
    }
    if document.kind != crate::conversations::DocumentKind::Plan {
        return Err("Select a plan document, not a task list.");
    }
    let revision = document
        .revision(token.revision)
        .ok_or("That saved plan revision is no longer available.")?;
    if revision.content_hash != content_hash
        || revision.object_hash != object_hash
        || revision.artefact_hash != artefact_hash
    {
        return Err("That saved plan changed. Reload the launch sheet.");
    }
    let content = state
        .documents
        .content(&document, revision.revision)
        .map_err(|_| "Power Plant could not read the selected plan.")?;
    if content.len() > workflows::input_context::MAXIMUM_IMPORTED_TEXT_BYTES
        || crate::workflows::artefacts::ObjectHash::of(content.as_bytes()) != revision.content_hash
    {
        return Err("That saved plan is too large or changed. Reload the launch sheet.");
    }
    Ok(Some(SelectedPlan {
        reference: PlanRevisionReference {
            document_id,
            revision: revision.revision,
            content_hash: revision.content_hash,
            object_hash: revision.object_hash,
            artefact_hash: revision.artefact_hash,
        },
        content,
    }))
}

#[allow(clippy::too_many_arguments)]
fn resolve_launch_task(
    mode: ExecutionMode,
    state: &AppState,
    record: &ConversationRecord,
    document: &str,
    revision: &str,
    hash: &str,
    index: &str,
) -> Result<Option<SelectedTask>, &'static str> {
    if mode == ExecutionMode::TaskList {
        if !index.trim().is_empty() {
            return Err("A task loop runs all remaining tasks, not one selected task.");
        }
        resolve_task_list_snapshot(state, record, document, revision, hash)?;
        Ok(None)
    } else {
        resolve_selected_task(state, record, document, revision, hash, index)
    }
}

fn resolve_selected_task(
    state: &AppState,
    record: &ConversationRecord,
    document_raw: &str,
    revision_raw: &str,
    hash_raw: &str,
    index_raw: &str,
) -> Result<Option<SelectedTask>, &'static str> {
    if document_raw.trim().is_empty()
        && revision_raw.trim().is_empty()
        && hash_raw.trim().is_empty()
        && index_raw.trim().is_empty()
    {
        return Ok(None);
    }
    let document_id = DocumentId::parse(document_raw.trim()).ok_or("Choose an available task.")?;
    let revision = revision_raw
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or("Choose an available task.")?;
    let index = index_raw
        .parse::<u32>()
        .ok()
        .ok_or("Choose an available task.")?;
    let expected_hash = crate::workflows::artefacts::ObjectHash::parse(hash_raw.trim())
        .ok_or("Choose an available task.")?;
    let document = state
        .documents
        .get(&document_id)
        .ok_or("That task list is no longer available.")?;
    if document.associated_conversation != Some(record.id)
        || document.kind != crate::conversations::DocumentKind::TaskList
    {
        return Err("That task list is not associated with this conversation.");
    }
    let stored = document
        .revision(revision)
        .ok_or("That task-list revision is no longer available.")?;
    if stored.content_hash != expected_hash {
        return Err("That task list changed. Reload the task preview.");
    }
    let task_list = state
        .documents
        .content(&document, revision)
        .map_err(|_| "Power Plant could not read the selected task list.")?;
    if crate::workflows::artefacts::ObjectHash::of(task_list.as_bytes()) != expected_hash {
        return Err("That task list changed. Reload the task preview.");
    }
    let list = workflows::task_list::parse(&task_list).map_err(|_| "That task list is invalid.")?;
    let task = list
        .tasks
        .get(index as usize)
        .ok_or("That task is no longer available.")?;
    if task.checked {
        return Err("Only an unchecked task can run.");
    }
    Ok(Some(SelectedTask {
        document_id,
        revision,
        content_hash: expected_hash.as_str(),
        index,
        markdown: task.markdown.clone(),
        task_list,
    }))
}

fn resolve_task_list_snapshot(
    state: &AppState,
    record: &ConversationRecord,
    document_raw: &str,
    revision_raw: &str,
    hash_raw: &str,
) -> Result<workflows::TaskListSnapshot, &'static str> {
    let packed = if revision_raw.trim().is_empty()
        && hash_raw.trim().is_empty()
        && document_raw.contains('/')
    {
        let mut parts = document_raw.trim().splitn(3, '/');
        (
            parts.next().unwrap_or_default().to_owned(),
            parts.next().unwrap_or_default().to_owned(),
            parts.next().unwrap_or_default().to_owned(),
        )
    } else {
        (
            document_raw.trim().to_owned(),
            revision_raw.trim().to_owned(),
            hash_raw.trim().to_owned(),
        )
    };
    let document_id =
        DocumentId::parse(&packed.0).ok_or("Choose a task list for this workflow.")?;
    let revision = packed
        .1
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or("Choose a task list for this workflow.")?;
    let expected_hash = crate::workflows::artefacts::ObjectHash::parse(&packed.2)
        .ok_or("Choose a task list for this workflow.")?;
    let document = state
        .documents
        .get(&document_id)
        .ok_or("That task list is no longer available.")?;
    if document.associated_conversation != Some(record.id)
        || document.kind != crate::conversations::DocumentKind::TaskList
    {
        return Err("That task list is not associated with this conversation.");
    }
    let stored = document
        .revision(revision)
        .ok_or("That task-list revision is no longer available.")?;
    if stored.content_hash != expected_hash {
        return Err("That task list changed. Reload the launch sheet.");
    }
    let markdown = state
        .documents
        .content(&document, revision)
        .map_err(|_| "Power Plant could not read the selected task list.")?;
    if crate::workflows::artefacts::ObjectHash::of(markdown.as_bytes()) != expected_hash {
        return Err("That task list changed. Reload the launch sheet.");
    }
    Ok(workflows::TaskListSnapshot {
        document_id,
        revision,
        content_hash: expected_hash.as_str(),
        markdown,
    })
}

fn target_access_summary(
    state: &AppState,
    record: &ConversationRecord,
    target: ProjectId,
) -> String {
    let mut selected = record.clone();
    selected.execution_target = Some(target);
    match crate::conversations::resolve_workflow_authority(
        &selected,
        &state.projects,
        &state.agents,
    ) {
        Ok(Some(authority)) => format!(
            "{} · {} tools",
            access_label(authority.effective.grant_access),
            authority.effective.tools.len()
        ),
        Ok(None) => "No access".to_owned(),
        Err(error) => error.message().to_owned(),
    }
}

fn access_label(access: AccessMode) -> &'static str {
    match access {
        AccessMode::ReadOnly => "Read-only target",
        AccessMode::ReadWrite => "Writable target",
    }
}

fn network_label(network: &crate::agents::NetworkAccess) -> String {
    match network {
        crate::agents::NetworkAccess::None => "No network".to_owned(),
        crate::agents::NetworkAccess::Restricted(domains) => {
            format!("Restricted domains ({})", domains.len())
        }
        crate::agents::NetworkAccess::Public => "Public internet".to_owned(),
    }
}

fn render(
    graft: impl Into<GraftRequest>,
    status: PatchStatus,
    view: &WorkflowLaunchView,
    state: &AppState,
) -> AppResult<Response> {
    match graft.into() {
        GraftRequest::Document => {
            let mut response = responses::chat_page_response(&view.document_title, state, view)?;
            responses::apply_patch_status(&mut response, status);
            Ok(response)
        }
        GraftRequest::Navigation => Ok(hypergraft::outcome::page_patch(
            &view.document_title,
            "chat-main",
            view,
        )?),
        GraftRequest::Patch => Ok(hypergraft::outcome::children_patch(
            status,
            "workflow-launch",
            &view.contents(),
        )?),
    }
}

#[cfg(test)]
mod tests;
