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
    conversations::ConversationRecord,
    error::{AppError, AppResult},
    projects::ProjectId,
    providers::{ModelSelection, ProviderKind, ThinkingEffort},
    responses,
    sessions::RequiredSession,
    state::AppState,
    workflows::{
        self, PhaseModelSelection, PinnedPreset, ResolveWorkflowError, WorkflowJob, WorkflowRun,
        WorkflowSelection, definition::CommitPolicy,
    },
};

const TITLE_SUFFIX: &str = " | Power Plant";

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct WorkflowQuery {
    stage: String,
    workflow: String,
    target: String,
    brief: String,
    commit_policy: String,
    #[serde(default)]
    phase: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkflowLaunchForm {
    revision: String,
    workflow: String,
    brief: String,
    #[serde(default)]
    target: String,
    #[serde(default)]
    commit_policy: String,
    #[serde(default)]
    preview_workflow: String,
    #[serde(default)]
    preview_target: String,
    #[serde(default)]
    preview_commit_policy: String,
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
    // GET navigation shares the launch form but supplies no launch approval.
    let fields = fields
        .into_iter()
        .filter(|(key, _)| {
            !matches!(
                key.as_str(),
                "revision"
                    | "preview_workflow"
                    | "preview_target"
                    | "preview_commit_policy"
                    | "confirm_additional_access"
            )
        })
        .collect();
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
        let phase = form.phase.clone();
        async move {
            let view = launch_view(
                &state,
                &record,
                Some(workflow.as_str()),
                Some(target.as_str()),
                &brief,
                &commit_policy,
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
    if record.active_job.is_some()
        || super::has_pending_review(&state, record.id)
        || state.sessions.busy(&session.0)
    {
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
    if let Some(authority) = authority.as_ref() {
        let Some(settings) = settings else {
            return error_view(
                PatchStatus::UnprocessableEntity,
                "Choose conversation settings before launch.",
            )
            .await;
        };
        run.project_authority =
            match workflows::handoff::project::ProjectAuthority::capture(authority, settings) {
                Ok(snapshot) => Some(snapshot),
                Err(error) => return error_view(PatchStatus::Conflict, error).await,
            };
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
    phase_raw: &[String],
    error: &'static str,
) -> WorkflowLaunchView {
    let records = state.workflows.list();
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
    let input_summary = "This workflow runs once with the supplied brief.".to_owned();
    let launch_blocked = workflows.is_empty()
        || target_unavailable
        || (!directory_launch && !targets.iter().any(|target| target.selected));
    WorkflowLaunchView {
        stage: if selection_available {
            "inputs"
        } else {
            "choose"
        },
        document_title: format!("Workflows · {}{}", record.title, TITLE_SUFFIX),
        conversation_id: record.id.as_hex(),
        revision: record.revision.to_string(),
        brief: if brief.is_empty() {
            default_brief(record)
        } else {
            // The textarea renders flush, but navigation round-trips a leading
            // newline; trim ends like launch validation so Back never grows one.
            brief.trim().to_owned()
        },
        workflows,
        targets,
        directory_launch,
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
