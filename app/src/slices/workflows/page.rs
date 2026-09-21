use askama::Template;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::workflows::definition::{
    MAXIMUM_DIRECTORIES, MAXIMUM_INPUTS, MAXIMUM_OUTPUTS, MAXIMUM_ROLES, MAXIMUM_STEPS,
};
use crate::workflows::summary::ProcessPhase;
use crate::workflows::{WorkflowRecord, summary};

use super::forms::{
    FormErrors, PhasePurpose, RoleDraft, StepDraft, WorkflowFormState, can_move_step,
    can_remove_step,
};

pub(super) struct EnvironmentOption {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) context: String,
    pub(super) selected: bool,
}

pub(super) struct InheritedField {
    pub(super) name: &'static str,
    pub(super) label: &'static str,
    pub(super) selected: bool,
}

pub(super) struct PresetChoice {
    pub(super) id: String,
    pub(super) name: String,
}

pub(super) struct ProviderChoice {
    pub(super) value: &'static str,
    pub(super) label: &'static str,
}

pub(super) const INDEX_TITLE: &str = "Workflows | Power Plant";
pub(super) const NEW_TITLE: &str = "New workflow | Power Plant";
pub(super) const CONFIG_TITLE: &str = "Configure workflow | Power Plant";

pub(super) struct CatalogueItem {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) summary: String,
    pub(super) effects: String,
    pub(super) inputs: String,
    pub(super) approvals: String,
    pub(super) roles: usize,
    pub(super) steps: usize,
    pub(super) updated: String,
    pub(super) use_href: String,
    pub(super) process_phases: Vec<ProcessPhase>,
}

#[derive(Template)]
#[template(path = "workflows/templates/index.html")]
pub(super) struct CatalogueView {
    pub(super) workflows: Vec<CatalogueItem>,
    pub(super) unavailable_starters: Vec<String>,
    context: Option<(String, String)>,
    selected_workflow: String,
    destinations: Vec<(String, String)>,
    error: &'static str,
}

impl CatalogueView {
    pub(super) fn new(state: &crate::state::AppState, query: &super::Selection) -> Self {
        let context = crate::conversations::ConversationId::parse(&query.conversation)
            .and_then(|id| state.conversations.metadata_for(&id))
            .map(|record| (record.id.as_hex(), record.title));
        let selection = crate::workflows::WorkflowSelection::parse(query.workflow.trim());
        let selected = selection
            .as_ref()
            .and_then(|selection| state.workflows.resolve(selection).ok());
        let error = if !query.workflow.is_empty() && selected.is_none() {
            "That workflow is no longer available. Choose another."
        } else {
            ""
        };
        let selected_workflow = selected
            .as_ref()
            .map(|record| record.pinned.definition.name().to_owned())
            .unwrap_or_default();
        let mut conversations = state.conversations.metadata();
        conversations.sort_by_key(|record| std::cmp::Reverse(record.updated_at_ms));
        let destinations =
            selection
                .filter(|_| selected.is_some())
                .map_or_else(Vec::new, |selection| {
                    conversations
                        .into_iter()
                        .filter(|record| {
                            context
                                .as_ref()
                                .is_none_or(|(id, _)| record.id.as_hex() == *id)
                        })
                        .map(|record| {
                            (
                                record.title,
                                format!(
                                    "/conversations/{}/workflow?workflow={}",
                                    record.id.as_hex(),
                                    selection.as_token()
                                ),
                            )
                        })
                        .collect()
                });
        Self {
            workflows: state
                .workflows
                .list()
                .iter()
                .map(|record| CatalogueItem {
                    id: record.id.as_hex(),
                    name: record.definition.name().to_owned(),
                    summary: summary::process_summary(&record.definition),
                    effects: summary::code_effects(&record.definition),
                    inputs: summary::required_inputs(&record.definition).to_owned(),
                    approvals: summary::approval_stops(&record.definition),
                    roles: record.definition.roles().len(),
                    steps: record.definition.steps().len(),
                    updated: format_time(record.updated_at_ms),
                    use_href: {
                        let token = crate::workflows::WorkflowSelection {
                            workflow_id: record.id,
                            definition_version: record.definition_version,
                        }
                        .as_token();
                        context.as_ref().map_or_else(
                            || format!("/workflows?workflow={token}"),
                            |(id, _)| format!("/conversations/{id}/workflow?workflow={token}"),
                        )
                    },
                    process_phases: summary::process_overview(&record.definition),
                })
                .collect(),
            unavailable_starters: state.workflows.unavailable_starters(),
            context,
            selected_workflow,
            destinations,
            error,
        }
    }
}

pub(super) struct DirectoryRow {
    pub(super) index: usize,
    pub(super) alias: String,
    pub(super) alias_error: &'static str,
}

pub(super) struct InputRow {
    pub(super) index: usize,
    pub(super) key: String,
    pub(super) key_error: &'static str,
    pub(super) kind: String,
    pub(super) kind_error: &'static str,
    pub(super) source_error: &'static str,
    pub(super) sources: Vec<SourceOption>,
    pub(super) can_remove: bool,
}

pub(super) struct SourceOption {
    pub(super) value: String,
    pub(super) label: String,
    pub(super) selected: bool,
}

pub(super) struct OutputRow {
    pub(super) index: usize,
    pub(super) key: String,
    pub(super) key_error: &'static str,
    pub(super) kind: String,
    pub(super) kind_error: &'static str,
    pub(super) can_move_up: bool,
    pub(super) can_move_down: bool,
    pub(super) can_remove: bool,
}

pub(super) struct ReviewPolicyRow {
    pub(super) report_output: String,
    pub(super) report_output_error: &'static str,
    pub(super) revision_target: String,
    pub(super) revision_target_error: &'static str,
    pub(super) attempt_limit: String,
    pub(super) attempt_limit_error: &'static str,
    pub(super) revision_options: Vec<SourceOption>,
}

pub(super) struct HumanRevisionRow {
    pub(super) revision_target_error: &'static str,
    pub(super) attempt_limit: String,
    pub(super) attempt_limit_error: &'static str,
}

pub(super) struct RoleRow {
    pub(super) index: usize,
    pub(super) position: usize,
    pub(super) key: String,
    pub(super) key_error: &'static str,
    pub(super) name: String,
    pub(super) name_error: &'static str,
    pub(super) expertise: String,
    pub(super) expertise_error: &'static str,
    pub(super) prompt: String,
    pub(super) prompt_error: &'static str,
    pub(super) can_move_up: bool,
    pub(super) can_move_down: bool,
    pub(super) can_remove: bool,
}

pub(super) struct CommandChoice {
    pub(super) value: &'static str,
    pub(super) label: &'static str,
    pub(super) selected: bool,
}

pub(super) struct PurposeChoice {
    pub(super) value: &'static str,
    pub(super) label: &'static str,
    pub(super) selected: bool,
}

pub(super) struct StepRow {
    pub(super) has_error: bool,
    pub(super) index: usize,
    pub(super) position: usize,
    pub(super) key: String,
    pub(super) key_error: &'static str,
    pub(super) name: String,
    pub(super) name_error: &'static str,
    pub(super) shared_role: bool,
    pub(super) purpose_label: &'static str,
    pub(super) purpose_error: &'static str,
    pub(super) purpose_choices: Vec<PurposeChoice>,
    pub(super) expertise: String,
    pub(super) expertise_error: &'static str,
    pub(super) instructions: String,
    pub(super) instructions_error: &'static str,
    pub(super) is_agent: bool,
    pub(super) is_gate: bool,
    pub(super) plan_checkpoint: bool,
    pub(super) environment: String,
    pub(super) environment_error: &'static str,
    pub(super) environment_hint: String,
    pub(super) action_error: &'static str,
    pub(super) role: String,
    pub(super) role_error: &'static str,
    pub(super) candidate_access: String,
    pub(super) candidate_access_error: &'static str,
    pub(super) settings_source: String,
    pub(super) settings_error: &'static str,
    pub(super) field_prefix: String,
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) thinking: String,
    pub(super) settings_instructions: String,
    pub(super) network: String,
    pub(super) network_domains: String,
    pub(super) settings_read_only: String,
    pub(super) settings_reviewed: String,
    pub(super) settings_direct: String,
    pub(super) location_host: bool,
    pub(super) host_approval_automatic: bool,
    pub(super) settings_preset: String,
    pub(super) settings_grants: String,
    pub(super) inherited_fields: Vec<InheritedField>,
    pub(super) settings_tool_options: Vec<crate::slices::execution_settings::page::ToolOption>,
    pub(super) command_choices: Vec<CommandChoice>,
    pub(super) command_error: &'static str,
    pub(super) command_consequence: &'static str,
    pub(super) show_outputs: bool,
    pub(super) directories: Vec<DirectoryRow>,
    pub(super) can_add_directory: bool,
    pub(super) inputs: Vec<InputRow>,
    pub(super) can_add_input: bool,
    pub(super) outputs: Vec<OutputRow>,
    pub(super) can_add_output: bool,
    pub(super) review_policy: Option<ReviewPolicyRow>,
    pub(super) human_revision: Option<HumanRevisionRow>,
    pub(super) human_revision_options: Vec<SourceOption>,
    pub(super) human_revision_policy_error: &'static str,
    pub(super) review_policy_error: &'static str,
    pub(super) can_review_gate: bool,
    pub(super) phase: usize,
    pub(super) can_move_up: bool,
    pub(super) can_move_down: bool,
    pub(super) can_remove: bool,
}

#[derive(Template)]
#[template(path = "workflows/templates/form.html")]
pub(super) struct WorkflowFormView {
    pub(super) title: &'static str,
    pub(super) action: String,
    pub(super) submit: &'static str,
    pub(super) name: String,
    pub(super) name_error: &'static str,
    pub(super) default_environment: String,
    pub(super) default_environment_error: &'static str,

    pub(super) environment_options: Vec<EnvironmentOption>,
    pub(super) no_ready_environment: bool,
    pub(super) presets: Vec<PresetChoice>,
    pub(super) providers: Vec<ProviderChoice>,
    pub(super) revision: String,
    pub(super) summary_error: &'static str,
    pub(super) roles: Vec<RoleRow>,
    pub(super) steps: Vec<StepRow>,
    pub(super) can_add_role: bool,
    pub(super) can_add_step: bool,
    pub(super) workflow_id: String,
    pub(super) show_delete: bool,
    pub(super) delete_error: &'static str,
    pub(super) process_phases: Vec<ProcessPhase>,
}

#[derive(Template)]
#[template(path = "workflows/templates/form.html", block = "workflow_form")]
pub(super) struct WorkflowFormContents<'a> {
    pub(super) action: &'a str,
    pub(super) submit: &'static str,
    pub(super) name: &'a str,
    pub(super) name_error: &'static str,
    pub(super) default_environment: &'a str,
    pub(super) default_environment_error: &'static str,

    pub(super) environment_options: &'a [EnvironmentOption],
    pub(super) no_ready_environment: bool,
    pub(super) presets: &'a [PresetChoice],
    pub(super) providers: &'a [ProviderChoice],
    pub(super) revision: &'a str,
    pub(super) summary_error: &'static str,
    pub(super) roles: &'a [RoleRow],
    pub(super) steps: &'a [StepRow],
    pub(super) can_add_role: bool,
    pub(super) can_add_step: bool,
    pub(super) workflow_id: &'a str,
    pub(super) show_delete: bool,
    pub(super) delete_error: &'static str,
    pub(super) process_phases: &'a [ProcessPhase],
}

impl WorkflowFormView {
    pub(super) fn create(state: WorkflowFormState, errors: FormErrors) -> Self {
        let process_phases = draft_process_overview(&state.steps);
        Self::from_state(
            "New workflow",
            "/workflows",
            "Create workflow",
            state,
            errors,
            "",
            false,
            "",
            process_phases,
        )
    }

    pub(super) fn edit(
        record: &WorkflowRecord,
        state: WorkflowFormState,
        errors: FormErrors,
        delete_error: &'static str,
    ) -> Self {
        let process_phases = draft_process_overview(&state.steps);
        Self::from_state(
            "Configure workflow",
            &format!("/workflows/{}/configuration", record.id.as_hex()),
            "Save",
            state,
            errors,
            &record.revision.to_string(),
            true,
            delete_error,
            process_phases,
        )
    }

    pub(super) fn edit_state(
        record: &WorkflowRecord,
        errors: FormErrors,
        delete_error: &'static str,
    ) -> Self {
        Self::edit(
            record,
            WorkflowFormState::from_record(record),
            errors,
            delete_error,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn from_state(
        title: &'static str,
        action: &str,
        submit: &'static str,
        state: WorkflowFormState,
        errors: FormErrors,
        revision: &str,
        show_delete: bool,
        delete_error: &'static str,
        process_phases: Vec<ProcessPhase>,
    ) -> Self {
        let role_count = state.roles.len();
        let step_count = state.steps.len();
        let revision = state
            .revision
            .map(|value| value.to_string())
            .unwrap_or_else(|| revision.to_owned());
        Self {
            title,
            action: action.to_owned(),
            submit,
            name: state.name.clone(),
            name_error: errors.name,
            default_environment: state.default_environment.clone(),
            default_environment_error: errors.default_environment,

            environment_options: Vec::new(),
            no_ready_environment: false,
            presets: Vec::new(),
            providers: Vec::new(),
            revision,
            summary_error: errors.summary,
            roles: state
                .roles
                .iter()
                .cloned()
                .enumerate()
                .map(|(index, role)| role_row(index, role_count, role, errors.roles.get(index)))
                .collect(),
            steps: state
                .steps
                .iter()
                .enumerate()
                .map(|(index, step)| {
                    step_row(
                        index,
                        step,
                        &state.steps,
                        state.roles.iter().any(|role| role.key == step.role),
                        errors.steps.get(index),
                    )
                })
                .collect(),
            can_add_role: role_count < MAXIMUM_ROLES,
            can_add_step: step_count < MAXIMUM_STEPS,
            workflow_id: if show_delete {
                action
                    .trim_start_matches("/workflows/")
                    .trim_end_matches("/configuration")
                    .to_owned()
            } else {
                String::new()
            },
            show_delete,
            delete_error,
            process_phases,
        }
    }

    pub(super) fn with_environments(
        mut self,
        mut options: Vec<EnvironmentOption>,
        no_ready: bool,
    ) -> Self {
        for id in std::iter::once(&self.default_environment)
            .chain(self.steps.iter().map(|step| &step.environment))
        {
            if !id.is_empty() && !options.iter().any(|option| option.id == *id) {
                options.push(EnvironmentOption {
                    id: id.clone(),
                    name: format!("Unavailable environment · {id}"),
                    context: "No ready snapshot".to_owned(),
                    selected: *id == self.default_environment,
                });
            }
        }
        let default_name = options
            .iter()
            .find(|option| option.id == self.default_environment)
            .map(|option| option.name.clone())
            .unwrap_or_default();
        for step in &mut self.steps {
            if step.environment.is_empty() {
                step.environment_hint = if default_name.is_empty() {
                    String::new()
                } else {
                    format!("Uses {default_name}")
                };
            } else if let Some(option) = options.iter().find(|option| option.id == step.environment)
            {
                step.environment_hint = option.context.clone();
            }
        }
        self.environment_options = options;
        self.no_ready_environment = no_ready;
        self
    }

    pub(super) fn with_catalogues(
        mut self,
        presets: Vec<PresetChoice>,
        providers: Vec<ProviderChoice>,
    ) -> Self {
        self.presets = presets;
        self.providers = providers;
        self
    }

    pub(super) fn contents(&self) -> WorkflowFormContents<'_> {
        WorkflowFormContents {
            action: &self.action,
            submit: self.submit,
            name: &self.name,
            name_error: self.name_error,
            default_environment: &self.default_environment,
            default_environment_error: self.default_environment_error,

            environment_options: &self.environment_options,
            no_ready_environment: self.no_ready_environment,
            presets: &self.presets,
            providers: &self.providers,
            revision: &self.revision,
            summary_error: self.summary_error,
            roles: &self.roles,
            steps: &self.steps,
            can_add_role: self.can_add_role,
            can_add_step: self.can_add_step,
            workflow_id: &self.workflow_id,
            show_delete: self.show_delete,
            delete_error: self.delete_error,
            process_phases: &self.process_phases,
        }
    }
}

fn role_row(
    index: usize,
    count: usize,
    role: RoleDraft,
    errors: Option<&super::forms::RoleErrors>,
) -> RoleRow {
    let errors = errors.cloned().unwrap_or_default();
    RoleRow {
        index,
        position: index + 1,
        key: role.key,
        key_error: errors.key,
        name: role.name,
        name_error: errors.name,
        expertise: role.expertise,
        expertise_error: errors.expertise,
        prompt: role.prompt,
        prompt_error: errors.prompt,
        can_move_up: index > 0,
        can_move_down: index + 1 < count,
        can_remove: true,
    }
}

fn step_row(
    index: usize,
    step: &StepDraft,
    steps: &[StepDraft],
    shared_role: bool,
    errors: Option<&super::forms::StepErrors>,
) -> StepRow {
    let errors = errors.cloned().unwrap_or_default();
    let earlier = &steps[..index];
    let output_count = step.outputs.len();
    let input_count = step.inputs.len();
    let is_agent = step.action == "agent";
    let is_gate = step.action == "human-gate";
    let plan_checkpoint = PhasePurpose::parse(&step.purpose) == Some(PhasePurpose::PlanCheckpoint);
    let command_id = crate::workflows::definition::SystemCommandId::parse(&step.command);
    let show_outputs = is_agent
        || is_gate
        || command_id.is_some_and(|command| !command.contract().required_outputs.is_empty());
    let lock_outputs = !is_agent;
    StepRow {
        shared_role,
        has_error: errors.has_error(),
        index,
        position: index + 1,
        key: step.key.clone(),
        key_error: errors.key,
        name: step.name.clone(),
        name_error: errors.name,
        purpose_label: PhasePurpose::parse(&step.purpose)
            .unwrap_or(PhasePurpose::Custom)
            .label(),
        purpose_error: errors.purpose,
        purpose_choices: [
            PhasePurpose::Planning,
            PhasePurpose::PlanReview,
            PhasePurpose::PlanCheckpoint,
            PhasePurpose::Implementation,
            PhasePurpose::ReadOnlyReview,
            PhasePurpose::ReviewAndFix,
            PhasePurpose::CodeApproval,
            PhasePurpose::Commit,
            PhasePurpose::Custom,
        ]
        .into_iter()
        .map(|purpose| PurposeChoice {
            value: purpose.as_str(),
            label: purpose.label(),
            selected: purpose.as_str() == step.purpose,
        })
        .collect(),
        expertise: step.expertise.clone(),
        expertise_error: errors.expertise,
        instructions: step.instructions.clone(),
        instructions_error: errors.instructions,
        is_agent,
        is_gate,
        plan_checkpoint,
        environment: step.environment.clone(),
        environment_error: errors.environment,
        environment_hint: String::new(),
        action_error: errors.action,
        role: step.role.clone(),
        role_error: errors.role,
        candidate_access: step.candidate_access.clone(),
        candidate_access_error: errors.candidate_access,
        settings_source: if step.settings_source.is_empty() {
            "defaults".to_owned()
        } else {
            step.settings_source.clone()
        },
        settings_error: errors.settings,
        field_prefix: format!("step_{index}_"),
        provider: step.provider.clone(),
        model: step.model.clone(),
        thinking: step.thinking.clone(),
        settings_instructions: step.settings_instructions.clone(),
        network: if step.network.is_empty() {
            "none".to_owned()
        } else {
            step.network.clone()
        },
        network_domains: step.network_domains.clone(),
        settings_read_only: step.settings_read_only.clone(),
        settings_reviewed: step.settings_reviewed.clone(),
        settings_direct: step.settings_direct.clone(),
        location_host: step.location == crate::execution::ToolLocation::Host.as_str(),
        host_approval_automatic: crate::execution::HostApprovalPolicy::parse(
            if step.host_approval.trim().is_empty() {
                crate::execution::HostApprovalPolicy::AskEachTime.as_str()
            } else {
                step.host_approval.trim()
            },
        )
        .is_some_and(|policy| policy.automatic()),
        settings_preset: step.settings_preset.clone(),
        inherited_fields: [
            ("model", "Model and reasoning effort"),
            ("instructions", "Instructions"),
            ("tools", "Tools"),
            ("network", "Network"),
            ("directories", "Directories"),
            ("environment", "Environment"),
            ("location", "Where tools run"),
            ("host_approval", "Host command approval"),
        ]
        .into_iter()
        .map(|(name, label)| InheritedField {
            name,
            label,
            selected: step.settings_inherit.iter().any(|value| value == name),
        })
        .collect(),
        settings_grants: serde_json::to_string(
            &step
                .settings_grants
                .iter()
                .map(|grant| grant.form_value())
                .collect::<Vec<_>>(),
        )
        .expect("directory form values"),
        settings_tool_options: crate::slices::execution_settings::page::tool_options(
            &step
                .tools
                .iter()
                .map(|tool| tool.as_str().to_owned())
                .collect::<Vec<_>>(),
        ),
        command_choices: crate::workflows::definition::SystemCommandId::all()
            .into_iter()
            .map(|command| CommandChoice {
                value: command.as_str(),
                label: command.label(),
                selected: command_id == Some(command),
            })
            .collect(),
        command_error: errors.command,
        command_consequence: command_id
            .map(crate::workflows::definition::SystemCommandId::consequence)
            .unwrap_or(""),
        show_outputs,
        directories: step
            .directories
            .iter()
            .enumerate()
            .map(|(dir_index, directory)| DirectoryRow {
                index: dir_index,
                alias: directory.alias.clone(),
                alias_error: errors
                    .directories
                    .get(dir_index)
                    .map(|item| item.alias)
                    .unwrap_or(""),
            })
            .collect(),
        can_add_directory: step.directories.len() < MAXIMUM_DIRECTORIES,
        inputs: step
            .inputs
            .iter()
            .enumerate()
            .map(|(input_index, input)| InputRow {
                index: input_index,
                key: input.key.clone(),
                key_error: errors
                    .inputs
                    .get(input_index)
                    .map(|item| item.key)
                    .unwrap_or(""),
                kind: input.kind.clone(),
                kind_error: errors
                    .inputs
                    .get(input_index)
                    .map(|item| item.kind)
                    .unwrap_or(""),
                source_error: errors
                    .inputs
                    .get(input_index)
                    .map(|item| item.source)
                    .unwrap_or(""),
                sources: source_options(earlier, &input.kind, &input.source),
                can_remove: input_count > 0
                    && !super::forms::input_removal_breaks_contract(step, input_index),
            })
            .collect(),
        can_add_input: input_count < MAXIMUM_INPUTS,
        outputs: step
            .outputs
            .iter()
            .enumerate()
            .map(|(output_index, output)| OutputRow {
                index: output_index,
                key: output.key.clone(),
                key_error: errors
                    .outputs
                    .get(output_index)
                    .map(|item| item.key)
                    .unwrap_or(""),
                kind: output.kind.clone(),
                kind_error: errors
                    .outputs
                    .get(output_index)
                    .map(|item| item.kind)
                    .unwrap_or(""),
                can_move_up: !lock_outputs && output_index > 0,
                can_move_down: !lock_outputs && output_index + 1 < output_count,
                can_remove: !lock_outputs
                    && output_count > 1
                    && !super::forms::output_is_required(step, output_index),
            })
            .collect(),
        can_add_output: !lock_outputs && output_count < MAXIMUM_OUTPUTS,
        review_policy: step.review_policy.as_ref().map(|policy| ReviewPolicyRow {
            report_output: policy.report_output.clone(),
            report_output_error: errors.report_output,
            revision_target: policy.revision_target.clone(),
            revision_target_error: errors.revision_target,
            attempt_limit: policy.attempt_limit.clone(),
            attempt_limit_error: errors.attempt_limit,
            revision_options: earlier
                .iter()
                .map(|item| SourceOption {
                    value: item.key.clone(),
                    label: item.name.clone(),
                    selected: item.key == policy.revision_target,
                })
                .collect(),
        }),
        human_revision: step.human_revision.as_ref().map(|policy| HumanRevisionRow {
            revision_target_error: errors.human_revision_target,
            attempt_limit: policy.attempt_limit.clone(),
            attempt_limit_error: errors.human_attempt_limit,
        }),
        human_revision_options: earlier
            .iter()
            .map(|item| SourceOption {
                value: item.key.clone(),
                label: item.name.clone(),
                selected: step
                    .human_revision
                    .as_ref()
                    .is_some_and(|policy| item.key == policy.revision_target),
            })
            .collect(),
        human_revision_policy_error: errors.human_revision_policy,
        review_policy_error: errors.review_policy,
        can_review_gate: is_agent && !earlier.is_empty(),
        phase: earlier
            .iter()
            .filter(|item| item.review_policy.is_some())
            .count()
            + 1,
        can_move_up: can_move_step(steps, index, true),
        can_move_down: can_move_step(steps, index, false),
        can_remove: can_remove_step(steps, index),
    }
}

fn draft_process_overview(steps: &[StepDraft]) -> Vec<ProcessPhase> {
    let phases: Vec<_> = steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let name = if step.name.trim().is_empty() {
                format!("Phase {}", index + 1)
            } else {
                step.name.clone()
            };
            use crate::workflows::definition::{CandidateAuthority, SystemCommandId};
            use summary::ProcessAction;
            let action = match step.action.as_str() {
                "agent" => match step.candidate_access.as_str() {
                    "edit-candidate" => ProcessAction::Model(CandidateAuthority::Edit),
                    "read-only" => ProcessAction::Model(CandidateAuthority::ReadOnly),
                    _ => ProcessAction::Invalid,
                },
                "human-gate" if step.outputs.iter().any(|output| {
                    output.kind == "plan-decision"
                }) => ProcessAction::PlanApproval,
                "human-gate" => ProcessAction::Approval,
                "system-command" => SystemCommandId::parse(&step.command)
                    .map(ProcessAction::Command)
                    .unwrap_or(ProcessAction::Invalid),
                _ => ProcessAction::Invalid,
            };
            let mut phase = ProcessPhase::new(index + 1, name, action);
            let independent_review = step.action == "agent"
                && step.candidate_access == "read-only"
                && step.outputs.iter().any(|output| output.kind == "review-report")
                && steps[..index].iter().any(|earlier| {
                    earlier.action == "agent" && earlier.candidate_access == "edit-candidate"
                });
            let review_and_fix = step.action == "agent"
                && step.candidate_access == "edit-candidate"
                && step.outputs.iter().any(|output| output.kind == "review-report");
            phase.annotate_review(independent_review, review_and_fix);
            let route = if step.action == "human-gate" {
                step.human_revision
                    .as_ref()
                    .map(|policy| (true, &policy.revision_target, &policy.attempt_limit))
            } else {
                step.review_policy
                    .as_ref()
                    .map(|policy| (false, &policy.revision_target, &policy.attempt_limit))
            };
            if let Some((human, target, limit)) = route {
                let destination = steps
                    .iter()
                    .position(|step| step.key == *target)
                    .map(|index| format!("phase {} ({})", index + 1, steps[index].name))
                    .unwrap_or_else(|| "an unavailable phase".to_owned());
                phase.approval = if step.outputs.iter().any(|output| {
                    output.kind == "plan-decision"
                }) {
                    format!(
                        "Plan checkpoint. Request changes returns to {destination}. Maximum {limit} attempts, including the first attempt."
                    )
                } else {
                    summary::revision_summary(human, &destination, limit)
                };
            }
            if step.action == "agent"
                && step.settings_source == "override"
                && !step.settings_inherit.iter().any(|field| field == "directories")
                && !step.settings_direct.trim().is_empty()
            {
                phase.annotate_direct(&step.settings_direct.lines().collect::<Vec<_>>().join(", "));
            }
            phase
        })
        .collect();

    phases
}

fn source_options(earlier: &[StepDraft], kind: &str, current: &str) -> Vec<SourceOption> {
    let mut options = Vec::new();
    if kind == "plan" {
        options.push(SourceOption {
            value: "launch-input:saved-plan".to_owned(),
            label: "Saved plan selected at launch".to_owned(),
            selected: current == "launch-input:saved-plan",
        });
        if super::forms::source_is_valid(
            "run-current-plan",
            crate::workflows::definition::ArtefactKind::Plan,
            earlier,
        ) {
            options.push(SourceOption {
                value: "run-current-plan".to_owned(),
                label: "Current plan from this workflow".to_owned(),
                selected: current == "run-current-plan",
            });
        }
    }
    if kind == "candidate-revision" {
        options.push(SourceOption {
            value: "run-initial-candidate".to_owned(),
            label: "Task source candidate".to_owned(),
            selected: current == "run-initial-candidate",
        });
        options.push(SourceOption {
            value: "run-current-candidate".to_owned(),
            label: "Current candidate".to_owned(),
            selected: current == "run-current-candidate",
        });
    }
    for step in earlier {
        for output in &step.outputs {
            if output.kind != kind || output.kind == "assistant-reply" {
                continue;
            }
            let value = format!("step-output:{}:{}", step.key, output.key);
            let selected = current == value;
            options.push(SourceOption {
                value,
                label: format!("{} · {}", step.key, output.key),
                selected,
            });
        }
    }
    options.retain(|option| {
        crate::workflows::definition::ArtefactKind::parse(kind)
            .is_some_and(|kind| super::forms::source_is_valid(&option.value, kind, earlier))
    });
    if !current.is_empty() && !options.iter().any(|option| option.selected) {
        options.push(SourceOption {
            value: current.to_owned(),
            label: current.to_owned(),
            selected: true,
        });
    }
    options
}

fn format_time(ms: u64) -> String {
    let seconds = i64::try_from(ms / 1000).unwrap_or(0);
    OffsetDateTime::from_unix_timestamp(seconds)
        .ok()
        .and_then(|time| time.format(&Rfc3339).ok())
        .unwrap_or_else(|| "unknown".to_owned())
}
