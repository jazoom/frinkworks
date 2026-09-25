use crate::agents::{AccessMode, ToolId};
use crate::execution::{DirectoryAccess, DirectoryGrant, SettingsOverrides};
use crate::workflows::definition::{
    AgentAuthority, AgentStep, ArtefactKind, ArtefactSource, GuestDirectoryAccess, HumanGateStep,
    HumanRevisionPolicy, InputKey, MAXIMUM_DIRECTORIES, MAXIMUM_INPUTS, MAXIMUM_OUTPUTS,
    MAXIMUM_ROLES, MAXIMUM_STEPS, ModelStepSettings, OutputKey, OutputKind, RequiredInput,
    RequiredOutput, ReviewPolicy, RoleDefinition, RoleKey, StepAccess, StepAction, StepDefinition,
    StepEnvironment, StepKey, SystemCommandId, SystemCommandStep, WorkflowDefinition,
};
use crate::workflows::{CatalogueError, WorkflowRecord};

pub(super) const MAXIMUM_FORM_BYTES: usize = 768 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FormError {
    Intent,
    Index,
    UnknownField,
    DuplicateField,
    MissingField,
    Sparse,
    Excessive,
    Revision,
    ReviewPolicy,
    ReviewTarget,
    HumanRevisionPolicy,
    Purpose,
    Connection,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PhasePurpose {
    Planning,
    PlanReview,
    PlanCheckpoint,
    Implementation,
    ReadOnlyReview,
    ReviewAndFix,
    Custom,
}

impl PhasePurpose {
    pub(super) fn parse(value: &str) -> Option<Self> {
        match value {
            "planning" | "plan" => Some(Self::Planning),
            "plan-review" | "review-plan" => Some(Self::PlanReview),
            "plan-checkpoint" | "plan-approval" => Some(Self::PlanCheckpoint),
            "implementation" | "implement" => Some(Self::Implementation),
            "read-only-review" | "review" => Some(Self::ReadOnlyReview),
            "review-and-fix" | "fixing-review" => Some(Self::ReviewAndFix),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Planning => "planning",
            Self::PlanReview => "plan-review",
            Self::PlanCheckpoint => "plan-checkpoint",
            Self::Implementation => "implementation",
            Self::ReadOnlyReview => "read-only-review",
            Self::ReviewAndFix => "review-and-fix",
            Self::Custom => "custom",
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Planning => "Planning",
            Self::PlanReview => "Plan review",
            Self::PlanCheckpoint => "Plan checkpoint",
            Self::Implementation => "Implementation",
            Self::ReadOnlyReview => "Read-only review",
            Self::ReviewAndFix => "Review and fix",
            Self::Custom => "Custom phase",
        }
    }

    fn default_tools(self) -> Vec<ToolId> {
        match self {
            Self::Planning | Self::PlanReview | Self::ReadOnlyReview => {
                vec![ToolId::List, ToolId::Read, ToolId::Run]
            }
            Self::Implementation | Self::ReviewAndFix => ToolId::ALL.to_vec(),
            Self::PlanCheckpoint | Self::Custom => Vec::new(),
        }
    }

    fn default_name(self) -> &'static str {
        match self {
            Self::Planning => "Plan the change",
            Self::PlanReview => "Review and correct the plan",
            Self::PlanCheckpoint => "Accept the plan",
            Self::Implementation => "Implement the change",
            Self::ReadOnlyReview => "Review the current code",
            Self::ReviewAndFix => "Review and fix the change",
            Self::Custom => "Custom phase",
        }
    }

    fn default_expertise(self) -> &'static str {
        match self {
            Self::Planning => "Inspects the project and explains a safe implementation sequence.",
            Self::PlanReview => "Reviews the plan and produces a corrected plan.",
            Self::PlanCheckpoint => "Presents the exact plan for human acceptance.",
            Self::Implementation => "Changes the original files with Write access.",
            Self::ReadOnlyReview => {
                "Assesses the current files for correctness, security and regressions."
            }
            Self::ReviewAndFix => "Reviews the current files and fixes safe issues immediately.",
            Self::Custom => "Runs the configured phase.",
        }
    }

    fn default_instructions(self) -> &'static str {
        match self {
            Self::Planning => {
                "Inspect the project and produce a plan. Do not change project files."
            }
            Self::PlanReview => {
                "Review the exact plan. Correct it when needed and explain the result. Do not change project files."
            }
            Self::PlanCheckpoint => {
                "Review the exact plan and accept it or request plan changes. This decision does not approve code."
            }
            Self::Implementation => {
                "Implement the task in the authorised directories. Write access changes the original files immediately."
            }
            Self::ReadOnlyReview => {
                "Review the current files. Do not change them. Submit a structured review."
            }
            Self::ReviewAndFix => {
                "Fix every safe issue that you find in the authorised directories. Submit a structured verdict for the current files."
            }
            Self::Custom => "Complete the assigned phase.",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FormIntent {
    PreviewExecution,
    Save,

    AddRole,
    AddPhase(PhasePurpose),

    SetPhasePurpose { step: usize, purpose: PhasePurpose },
    RemoveRole(usize),
    MoveRoleUp(usize),
    MoveRoleDown(usize),
    AddStep,
    RemoveStep(usize),
    MoveStepUp(usize),
    MoveStepDown(usize),
    UpdateReviewPolicy(usize),
    AddDirectory { step: usize },
    RemoveDirectory { step: usize, directory: usize },
    AddOutput(usize),
    RemoveOutput { step: usize, output: usize },
    MoveOutputUp { step: usize, output: usize },
    MoveOutputDown { step: usize, output: usize },
    AddInput(usize),
    RemoveInput { step: usize, input: usize },
    ApplyPreset { step: usize },
}

#[derive(Clone, Debug)]
pub(super) struct RoleDraft {
    pub(super) key: String,
    pub(super) name: String,
    pub(super) expertise: String,
    pub(super) prompt: String,
}

#[derive(Clone, Debug)]
pub(super) struct DirectoryDraft {
    pub(super) alias: String,
    pub(super) access: String,
}

#[derive(Clone, Debug)]
pub(super) struct OutputDraft {
    pub(super) key: String,
    pub(super) kind: String,
}

#[derive(Clone, Debug)]
pub(super) struct InputDraft {
    pub(super) key: String,
    pub(super) kind: String,
    pub(super) source: String,
}

#[derive(Clone, Debug)]
pub(super) struct ReviewPolicyDraft {
    pub(super) report_output: String,
    pub(super) revision_target: String,
    pub(super) attempt_limit: String,
}

#[derive(Clone, Debug)]
pub(super) struct HumanRevisionDraft {
    pub(super) revision_target: String,
    pub(super) attempt_limit: String,
}

#[derive(Clone, Debug)]
pub(super) struct StepDraft {
    pub(super) key: String,
    pub(super) name: String,
    pub(super) purpose: String,
    pub(super) expertise: String,
    pub(super) instructions: String,
    pub(super) action: String,
    pub(super) environment: String,
    pub(super) role: String,
    pub(super) directory_access: String,
    pub(super) command: String,
    pub(super) tools: Vec<ToolId>,
    pub(super) settings_source: String,
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) thinking: String,
    pub(super) settings_instructions: String,
    pub(super) network: String,
    pub(super) network_domains: String,
    pub(super) settings_read_only: String,

    pub(super) settings_direct: String,
    pub(super) location: String,
    pub(super) host_approval: String,
    pub(super) settings_preset: String,
    pub(super) settings_grants: Vec<DirectoryGrant>,
    pub(super) settings_inherit: Vec<String>,
    pub(super) directories: Vec<DirectoryDraft>,
    pub(super) inputs: Vec<InputDraft>,
    pub(super) outputs: Vec<OutputDraft>,
    pub(super) review_policy: Option<ReviewPolicyDraft>,
    pub(super) human_revision: Option<HumanRevisionDraft>,
}

#[derive(Clone, Debug)]
pub(super) struct WorkflowFormState {
    pub(super) name: String,
    pub(super) default_environment: String,
    pub(super) revision: Option<u64>,

    pub(super) roles: Vec<RoleDraft>,
    pub(super) steps: Vec<StepDraft>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct RoleErrors {
    pub(super) key: &'static str,
    pub(super) name: &'static str,
    pub(super) expertise: &'static str,
    pub(super) prompt: &'static str,
}

#[derive(Clone, Debug, Default)]
pub(super) struct DirectoryErrors {
    pub(super) alias: &'static str,
}

#[derive(Clone, Debug, Default)]
pub(super) struct OutputErrors {
    pub(super) key: &'static str,
    pub(super) kind: &'static str,
}

#[derive(Clone, Debug, Default)]
pub(super) struct InputErrors {
    pub(super) key: &'static str,
    pub(super) kind: &'static str,
    pub(super) source: &'static str,
}

#[derive(Clone, Debug, Default)]
pub(super) struct StepErrors {
    pub(super) key: &'static str,
    pub(super) name: &'static str,
    pub(super) purpose: &'static str,
    pub(super) expertise: &'static str,
    pub(super) instructions: &'static str,
    pub(super) action: &'static str,
    pub(super) environment: &'static str,
    pub(super) role: &'static str,
    pub(super) directory_access: &'static str,
    pub(super) command: &'static str,
    pub(super) settings: &'static str,
    pub(super) review_policy: &'static str,
    pub(super) report_output: &'static str,
    pub(super) revision_target: &'static str,
    pub(super) attempt_limit: &'static str,
    pub(super) human_revision_policy: &'static str,
    pub(super) human_revision_target: &'static str,
    pub(super) human_attempt_limit: &'static str,
    pub(super) directories: Vec<DirectoryErrors>,
    pub(super) inputs: Vec<InputErrors>,
    pub(super) outputs: Vec<OutputErrors>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct FormErrors {
    pub(super) summary: &'static str,
    pub(super) name: &'static str,
    pub(super) default_environment: &'static str,

    pub(super) roles: Vec<RoleErrors>,
    pub(super) steps: Vec<StepErrors>,
}

impl FormError {
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::Intent => "That form action is not valid.",
            Self::ReviewPolicy => "That review policy is not valid.",
            Self::HumanRevisionPolicy => "That human revision route is not valid.",
            Self::Index => "That form row is not valid.",
            Self::UnknownField => "That form includes an unknown field.",
            Self::DuplicateField => "That form includes a duplicate field.",
            Self::MissingField => "That form omits a required field.",
            Self::Sparse => "That form row is not valid.",
            Self::Excessive => "That form has too many rows.",
            Self::Revision => "Reload the workflow and try again.",
            Self::ReviewTarget => "That action would invalidate a review revision target.",
            Self::Purpose => "Choose a supported phase purpose.",
            Self::Connection => "That action would break a phase connection.",
        }
    }
}

impl FormErrors {
    pub(super) fn summary(message: &'static str) -> Self {
        Self {
            summary: message,
            ..Self::default()
        }
    }

    pub(super) fn for_state(state: &WorkflowFormState) -> Self {
        Self::sized(state.roles.len(), &state.steps)
    }

    fn sized(roles: usize, steps: &[StepDraft]) -> Self {
        Self {
            summary: "",
            name: "",
            default_environment: "",

            roles: vec![RoleErrors::default(); roles],
            steps: steps
                .iter()
                .map(|step| StepErrors {
                    directories: vec![DirectoryErrors::default(); step.directories.len()],
                    inputs: vec![InputErrors::default(); step.inputs.len()],
                    outputs: vec![OutputErrors::default(); step.outputs.len()],
                    ..StepErrors::default()
                })
                .collect(),
        }
    }

    fn has_field_error(&self) -> bool {
        !self.name.is_empty()
            || !self.default_environment.is_empty()
            || self.roles.iter().any(|role| {
                !role.key.is_empty()
                    || !role.name.is_empty()
                    || !role.expertise.is_empty()
                    || !role.prompt.is_empty()
            })
            || self.steps.iter().any(StepErrors::has_error)
    }
}

impl StepErrors {
    pub(super) fn has_error(&self) -> bool {
        let step = self;
        !step.key.is_empty()
            || !step.name.is_empty()
            || !step.purpose.is_empty()
            || !step.expertise.is_empty()
            || !step.instructions.is_empty()
            || !step.action.is_empty()
            || !step.environment.is_empty()
            || !step.role.is_empty()
            || !step.directory_access.is_empty()
            || !step.command.is_empty()
            || !step.settings.is_empty()
            || !step.review_policy.is_empty()
            || !step.report_output.is_empty()
            || !step.revision_target.is_empty()
            || !step.attempt_limit.is_empty()
            || !step.human_revision_policy.is_empty()
            || !step.human_revision_target.is_empty()
            || !step.human_attempt_limit.is_empty()
            || step
                .directories
                .iter()
                .any(|directory| !directory.alias.is_empty())
            || step.inputs.iter().any(|input| {
                !input.key.is_empty() || !input.kind.is_empty() || !input.source.is_empty()
            })
            || step
                .outputs
                .iter()
                .any(|output| !output.key.is_empty() || !output.kind.is_empty())
    }
}

impl WorkflowFormState {
    pub(super) fn blank() -> Self {
        Self {
            name: String::new(),
            default_environment: String::new(),
            revision: None,

            roles: Vec::new(),
            steps: vec![phase_draft("phase-1", PhasePurpose::Implementation, "")],
        }
    }

    pub(super) fn from_record(record: &WorkflowRecord) -> Self {
        let stored_roles: Vec<RoleDraft> = record
            .definition
            .roles()
            .iter()
            .map(|role| RoleDraft {
                key: role.key.as_str().to_owned(),
                name: role.name.clone(),
                expertise: role.expertise.clone(),
                prompt: role.prompt_defaults.clone(),
            })
            .collect();
        let mut steps: Vec<_> = record
            .definition
            .steps()
            .iter()
            .map(step_from_definition)
            .collect();
        sync_phase_text_from_roles(&mut steps, &stored_roles);
        let roles = stored_roles
            .into_iter()
            .filter(|role| steps.iter().filter(|step| step.role == role.key).count() > 1)
            .collect();
        Self {
            name: record.definition.name().to_owned(),
            default_environment: record.definition.default_environment().as_hex(),
            revision: Some(record.revision),

            roles,
            steps,
        }
    }

    pub(super) fn parse(pairs: Vec<(String, String)>) -> Result<(Self, FormIntent), FormError> {
        let mut seen = Vec::new();
        let mut name = String::new();
        let mut default_environment = String::new();
        let mut revision = None;
        let mut intent = None;
        let mut role_fields: Vec<(usize, RolePart, String)> = Vec::new();
        let mut step_fields: Vec<(usize, StepPart, String)> = Vec::new();
        for (key, value) in pairs {
            if seen.iter().any(|item: &String| item == &key) {
                return Err(FormError::DuplicateField);
            }
            seen.push(key.clone());
            match parse_field(&key)? {
                Field::Name => name = value,
                Field::DefaultEnvironment => default_environment = value,

                Field::Revision => {
                    if !value.trim().is_empty() {
                        revision = Some(parse_revision(&value)?);
                    }
                }
                Field::Intent => intent = Some(parse_intent(&value)?),
                Field::Role { index, part } => role_fields.push((index, part, value)),
                Field::Step { index, part } => step_fields.push((index, part, value)),
            }
        }
        let intent = intent.ok_or(FormError::Intent)?;
        let roles = collect_roles(role_fields)?;
        let mut steps = collect_steps(step_fields)?;
        if roles.len() > MAXIMUM_ROLES || steps.len() > MAXIMUM_STEPS {
            return Err(FormError::Excessive);
        }
        sync_phase_text_from_roles(&mut steps, &roles);
        ensure_action_defaults(&mut steps);
        normalise_phase_contracts(&mut steps);
        Ok((
            Self {
                name,
                default_environment,
                revision,
                roles,
                steps,
            },
            intent,
        ))
    }

    pub(super) fn apply(&mut self, intent: FormIntent) -> Result<(), FormError> {
        match intent {
            FormIntent::Save | FormIntent::PreviewExecution => Ok(()),

            FormIntent::AddRole => {
                if self.roles.len() >= MAXIMUM_ROLES {
                    return Err(FormError::Excessive);
                }
                let key = next_key(
                    "role",
                    &self
                        .roles
                        .iter()
                        .map(|role| role.key.as_str())
                        .chain(self.steps.iter().map(|step| step.role.as_str()))
                        .collect::<Vec<_>>(),
                );
                self.roles.push(RoleDraft {
                    key,
                    name: String::new(),
                    expertise: String::new(),
                    prompt: String::new(),
                });
                Ok(())
            }
            FormIntent::RemoveRole(index) => {
                if index >= self.roles.len() {
                    return Err(FormError::Index);
                }
                if self
                    .steps
                    .iter()
                    .any(|step| step.role == self.roles[index].key)
                {
                    return Err(FormError::Connection);
                }
                self.roles.remove(index);
                Ok(())
            }
            FormIntent::MoveRoleUp(index) => move_item(&mut self.roles, index, true),
            FormIntent::MoveRoleDown(index) => move_item(&mut self.roles, index, false),
            FormIntent::AddPhase(purpose) => self.add_phase(purpose),

            FormIntent::AddStep => self.add_phase(PhasePurpose::Implementation),
            FormIntent::SetPhasePurpose { step, purpose } => {
                if step >= self.steps.len() {
                    return Err(FormError::Index);
                }
                self.steps[step].purpose = purpose.as_str().to_owned();
                normalise_phase_contracts(&mut self.steps);
                Ok(())
            }
            FormIntent::RemoveStep(index) => {
                if self.steps.len() <= 1 || index >= self.steps.len() {
                    return Err(FormError::Index);
                }
                if !can_remove_step(&self.steps, index) {
                    return Err(FormError::ReviewTarget);
                }
                self.steps.remove(index);
                Ok(())
            }
            FormIntent::MoveStepUp(index) => move_step(&mut self.steps, index, true),
            FormIntent::MoveStepDown(index) => move_step(&mut self.steps, index, false),
            FormIntent::UpdateReviewPolicy(index) => {
                if index >= self.steps.len() {
                    return Err(FormError::Index);
                }
                Ok(())
            }
            FormIntent::AddDirectory { step } => {
                let row = self.steps.get_mut(step).ok_or(FormError::Index)?;
                if row.directories.len() >= MAXIMUM_DIRECTORIES {
                    return Err(FormError::Excessive);
                }
                row.directories.push(DirectoryDraft {
                    alias: String::new(),
                    access: AccessMode::ReadOnly.as_str().to_owned(),
                });
                Ok(())
            }
            FormIntent::RemoveDirectory { step, directory } => {
                let row = self.steps.get_mut(step).ok_or(FormError::Index)?;
                if directory >= row.directories.len() {
                    return Err(FormError::Index);
                }
                row.directories.remove(directory);
                Ok(())
            }
            FormIntent::AddInput(step) => {
                if step >= self.steps.len() {
                    return Err(FormError::Index);
                }
                if self.steps[step].inputs.len() >= MAXIMUM_INPUTS {
                    return Err(FormError::Excessive);
                }
                let source =
                    latest_output_source(&self.steps[..step], OutputKind::Plan).unwrap_or_default();
                self.steps[step].inputs.push(InputDraft {
                    key: String::new(),
                    kind: ArtefactKind::Plan.as_str().to_owned(),
                    source,
                });
                Ok(())
            }
            FormIntent::RemoveInput { step, input } => {
                let row = self.steps.get(step).ok_or(FormError::Index)?;
                if input >= row.inputs.len() {
                    return Err(FormError::Index);
                }
                if input_removal_breaks_contract(row, input) {
                    return Err(FormError::Connection);
                }
                self.steps[step].inputs.remove(input);
                Ok(())
            }
            FormIntent::AddOutput(step) => {
                let row = self.steps.get_mut(step).ok_or(FormError::Index)?;
                if row.outputs.len() >= MAXIMUM_OUTPUTS {
                    return Err(FormError::Excessive);
                }
                row.outputs.push(OutputDraft {
                    key: String::new(),
                    kind: OutputKind::AssistantReply.as_str().to_owned(),
                });
                Ok(())
            }
            FormIntent::RemoveOutput { step, output } => {
                let row = self.steps.get(step).ok_or(FormError::Index)?;
                if row.outputs.len() <= 1 || output >= row.outputs.len() {
                    return Err(FormError::Index);
                }
                if output_is_required(row, output)
                    || output_is_referenced(&self.steps, step, output)
                {
                    return Err(FormError::Connection);
                }
                self.steps[step].outputs.remove(output);
                Ok(())
            }
            FormIntent::MoveOutputUp { step, output } => {
                let row = self.steps.get_mut(step).ok_or(FormError::Index)?;
                move_item(&mut row.outputs, output, true)
            }
            FormIntent::MoveOutputDown { step, output } => {
                let row = self.steps.get_mut(step).ok_or(FormError::Index)?;
                move_item(&mut row.outputs, output, false)
            }
            FormIntent::ApplyPreset { step } => {
                if step >= self.steps.len() {
                    return Err(FormError::Index);
                }
                self.steps[step].settings_source = "override".to_owned();
                Ok(())
            }
        }
    }

    fn add_phase(&mut self, purpose: PhasePurpose) -> Result<(), FormError> {
        if self.steps.len() >= MAXIMUM_STEPS {
            return Err(FormError::Excessive);
        }
        let key = next_key(
            "phase",
            &self
                .steps
                .iter()
                .map(|step| step.key.as_str())
                .collect::<Vec<_>>(),
        );
        let existing: Vec<_> = self
            .steps
            .iter()
            .map(|step| step.role.as_str())
            .chain(self.roles.iter().map(|role| role.key.as_str()))
            .collect();
        let role = next_key("role", &existing);
        self.steps.push(phase_draft(&key, purpose, &role));
        normalise_phase_contracts(&mut self.steps);
        Ok(())
    }

    pub(super) fn to_definition(&self) -> Result<WorkflowDefinition, FormErrors> {
        let mut errors = FormErrors::sized(self.roles.len(), &self.steps);
        let mut roles = Vec::new();
        {
            for (index, step) in self.steps.iter().enumerate() {
                if step.action != "agent" || self.roles.iter().any(|role| role.key == step.role) {
                    continue;
                }
                let key = match RoleKey::parse(&step.role) {
                    Ok(key) => key,
                    Err(error) => {
                        errors.steps[index].role = error.message();
                        continue;
                    }
                };
                let name = format!("{} executor", phase_purpose(step).label());
                match RoleDefinition::new(
                    key,
                    name,
                    step.expertise.clone(),
                    step.instructions.clone(),
                ) {
                    Ok(role) => roles.push(role),
                    Err(error) => match error {
                        crate::workflows::definition::DefinitionError::Expertise => {
                            errors.steps[index].expertise = error.message();
                        }
                        crate::workflows::definition::DefinitionError::PromptDefaults => {
                            errors.steps[index].instructions = error.message();
                        }
                        _ => errors.steps[index].role = error.message(),
                    },
                }
            }
        }
        for (index, role) in self.roles.iter().enumerate() {
            match RoleKey::parse(&role.key) {
                Ok(key) => match RoleDefinition::new(
                    key,
                    role.name.clone(),
                    role.expertise.clone(),
                    role.prompt.clone(),
                ) {
                    Ok(role) => roles.push(role),
                    Err(error) => match error {
                        crate::workflows::definition::DefinitionError::Name => {
                            errors.roles[index].name = error.message();
                        }
                        crate::workflows::definition::DefinitionError::Expertise => {
                            errors.roles[index].expertise = error.message();
                        }
                        crate::workflows::definition::DefinitionError::PromptDefaults => {
                            errors.roles[index].prompt = error.message();
                        }
                        _ => errors.roles[index].key = error.message(),
                    },
                },
                Err(error) => errors.roles[index].key = error.message(),
            }
        }
        let mut steps = Vec::new();
        for (index, step) in self.steps.iter().enumerate() {
            if let Some(built) = build_step(step, &mut errors.steps[index]) {
                steps.push(built);
            }
        }
        if !errors.summary.is_empty() || errors.has_field_error() {
            if errors.summary.is_empty() {
                errors.summary = "Fix the highlighted fields.";
            }
            return Err(errors);
        }
        if steps.is_empty() {
            errors.summary = crate::workflows::definition::DefinitionError::StepCount.message();
            return Err(errors);
        }
        for (index, (step, draft)) in steps.iter_mut().zip(&self.steps).enumerate() {
            step.review = if let Some(policy) = &draft.review_policy {
                let report_output = match OutputKey::parse(&policy.report_output) {
                    Ok(key) => key,
                    Err(error) => {
                        errors.steps[index].report_output = error.message();
                        errors.summary = "Fix the highlighted fields.";
                        return Err(errors);
                    }
                };
                let revision_target = match StepKey::parse(&policy.revision_target) {
                    Ok(key) => key,
                    Err(error) => {
                        errors.steps[index].revision_target = error.message();
                        errors.summary = "Fix the highlighted fields.";
                        return Err(errors);
                    }
                };
                let attempt_limit = match policy.attempt_limit.parse::<u8>() {
                    Ok(limit)
                        if (crate::workflows::definition::MINIMUM_REVIEW_ATTEMPTS
                            ..=crate::workflows::definition::MAXIMUM_REVIEW_ATTEMPTS)
                            .contains(&limit) =>
                    {
                        limit
                    }
                    _ => {
                        errors.steps[index].attempt_limit =
                            crate::workflows::definition::DefinitionError::AttemptLimit.message();
                        errors.summary = "Fix the highlighted fields.";
                        return Err(errors);
                    }
                };
                Some(ReviewPolicy {
                    report_output,
                    revision_target,
                    attempt_limit,
                })
            } else {
                None
            };
        }
        let default_environment =
            match crate::environments::EnvironmentId::parse(self.default_environment.trim()) {
                Some(id) => id,
                None => {
                    errors.default_environment =
                        crate::workflows::definition::DefinitionError::Environment.message();
                    errors.summary = "Fix the highlighted fields.";
                    return Err(errors);
                }
            };
        match WorkflowDefinition::from_parts(self.name.clone(), default_environment, roles, steps) {
            Ok(definition) => Ok(definition),
            Err(error) => {
                relate_definition_error(self, error, &mut errors);
                if errors.summary.is_empty() {
                    errors.summary = error.message();
                }
                Err(errors)
            }
        }
    }
}

impl From<CatalogueError> for FormErrors {
    fn from(error: CatalogueError) -> Self {
        let mut errors = FormErrors::summary(error.message());
        if error == CatalogueError::DuplicateName {
            errors.name = error.message();
        }
        errors
    }
}

#[derive(Clone, Copy)]
enum Field {
    Name,
    DefaultEnvironment,

    Revision,
    Intent,
    Role { index: usize, part: RolePart },
    Step { index: usize, part: StepPart },
}

#[derive(Clone, Copy)]
enum RolePart {
    Key,
    Name,
    Expertise,
    Prompt,
}

#[derive(Clone, Copy)]
enum StepPart {
    Key,
    Name,
    Purpose,
    Expertise,
    Instructions,
    Action,
    Environment,
    Role,
    CandidateAccess,
    Command,
    ReviewPolicy,
    HumanRevisionPolicy,
    HumanRevisionTarget,
    HumanAttemptLimit,
    ReportOutput,
    RevisionTarget,
    AttemptLimit,
    Tool(ToolId),
    SettingsSource,
    Provider,
    Model,
    Thinking,
    SettingsInstructions,
    Network,
    NetworkDomains,
    SettingsReadOnly,
    SettingsDirect,
    Location,
    HostApproval,
    SettingsPreset,
    SettingsGrants,
    SettingsInherit(&'static str),
    Dir { index: usize, part: DirPart },
    Input { index: usize, part: InputPart },
    Output { index: usize, part: OutputPart },
}

#[derive(Clone, Copy)]
enum DirPart {
    Alias,
    Access,
}

#[derive(Clone, Copy)]
enum OutputPart {
    Key,
    Kind,
}

#[derive(Clone, Copy)]
enum InputPart {
    Key,
    Kind,
    Source,
}

fn parse_field(name: &str) -> Result<Field, FormError> {
    match name {
        "name" => Ok(Field::Name),
        "default-environment" => Ok(Field::DefaultEnvironment),

        "revision" => Ok(Field::Revision),
        "intent" => Ok(Field::Intent),
        _ => parse_row_field(name),
    }
}

fn parse_row_field(name: &str) -> Result<Field, FormError> {
    let mut parts = name.split('_');
    let prefix = parts.next().ok_or(FormError::UnknownField)?;
    match prefix {
        "role" => {
            let index = parse_index(parts.next().ok_or(FormError::UnknownField)?)?;
            let part = match parts.next() {
                Some("key") => RolePart::Key,
                Some("name") => RolePart::Name,
                Some("expertise") => RolePart::Expertise,
                Some("prompt") => RolePart::Prompt,
                _ => return Err(FormError::UnknownField),
            };
            if parts.next().is_some() {
                return Err(FormError::UnknownField);
            }
            Ok(Field::Role { index, part })
        }
        "step" => {
            let index = parse_index(parts.next().ok_or(FormError::UnknownField)?)?;
            let part = match parts.next() {
                Some("key") => StepPart::Key,
                Some("name") => StepPart::Name,
                Some("purpose") => StepPart::Purpose,
                Some("expertise") => StepPart::Expertise,
                Some("instructions") => StepPart::Instructions,
                Some("action") => StepPart::Action,
                Some("environment") => StepPart::Environment,
                Some("role") => StepPart::Role,
                Some("directory-access") => StepPart::CandidateAccess,
                Some("command") => StepPart::Command,
                Some("review-policy") => StepPart::ReviewPolicy,
                Some("human-revision-policy") => StepPart::HumanRevisionPolicy,
                Some("human-revision-target") => StepPart::HumanRevisionTarget,
                Some("human-attempt-limit") => StepPart::HumanAttemptLimit,
                Some("report-output") => StepPart::ReportOutput,
                Some("revision-target") => StepPart::RevisionTarget,
                Some("attempt-limit") => StepPart::AttemptLimit,
                Some("tool") => {
                    let tool = parts.next().ok_or(FormError::UnknownField)?;
                    let tool = ToolId::parse(tool).ok_or(FormError::UnknownField)?;
                    if parts.next().is_some() {
                        return Err(FormError::UnknownField);
                    }
                    return Ok(Field::Step {
                        index,
                        part: StepPart::Tool(tool),
                    });
                }
                Some("settings-source") => StepPart::SettingsSource,
                Some("provider") => StepPart::Provider,
                Some("model") => StepPart::Model,
                Some("thinking") => StepPart::Thinking,
                Some("settings-instructions") => StepPart::SettingsInstructions,
                Some("network") => StepPart::Network,
                Some("network-domains") => StepPart::NetworkDomains,
                Some("read-only") => StepPart::SettingsReadOnly,
                Some("direct") => StepPart::SettingsDirect,
                Some("location") => StepPart::Location,
                Some("settings-host-approval") => StepPart::HostApproval,
                Some("settings-preset") => StepPart::SettingsPreset,
                Some("settings-grants") => StepPart::SettingsGrants,
                Some("inherit-model") => StepPart::SettingsInherit("model"),
                Some("inherit-instructions") => StepPart::SettingsInherit("instructions"),
                Some("inherit-tools") => StepPart::SettingsInherit("tools"),
                Some("inherit-network") => StepPart::SettingsInherit("network"),
                Some("inherit-environment") => StepPart::SettingsInherit("environment"),
                Some("inherit-directories") => StepPart::SettingsInherit("directories"),
                Some("inherit-location") => StepPart::SettingsInherit("location"),
                Some("inherit-host-approval") => StepPart::SettingsInherit("host_approval"),
                Some("dir") => {
                    let dir = parse_index(parts.next().ok_or(FormError::UnknownField)?)?;
                    let dir_part = match parts.next() {
                        Some("alias") => DirPart::Alias,
                        Some("access") => DirPart::Access,
                        _ => return Err(FormError::UnknownField),
                    };
                    if parts.next().is_some() {
                        return Err(FormError::UnknownField);
                    }
                    StepPart::Dir {
                        index: dir,
                        part: dir_part,
                    }
                }
                Some("input") => {
                    let input = parse_index(parts.next().ok_or(FormError::UnknownField)?)?;
                    let input_part = match parts.next() {
                        Some("key") => InputPart::Key,
                        Some("kind") => InputPart::Kind,
                        Some("source") => InputPart::Source,
                        _ => return Err(FormError::UnknownField),
                    };
                    if parts.next().is_some() {
                        return Err(FormError::UnknownField);
                    }
                    StepPart::Input {
                        index: input,
                        part: input_part,
                    }
                }
                Some("output") => {
                    let output = parse_index(parts.next().ok_or(FormError::UnknownField)?)?;
                    let output_part = match parts.next() {
                        Some("key") => OutputPart::Key,
                        Some("kind") => OutputPart::Kind,
                        _ => return Err(FormError::UnknownField),
                    };
                    if parts.next().is_some() {
                        return Err(FormError::UnknownField);
                    }
                    StepPart::Output {
                        index: output,
                        part: output_part,
                    }
                }
                _ => return Err(FormError::UnknownField),
            };
            if !matches!(
                part,
                StepPart::Dir { .. } | StepPart::Input { .. } | StepPart::Output { .. }
            ) && parts.next().is_some()
            {
                return Err(FormError::UnknownField);
            }
            Ok(Field::Step { index, part })
        }
        _ => Err(FormError::UnknownField),
    }
}

fn parse_index(raw: &str) -> Result<usize, FormError> {
    if raw.is_empty() || (raw.len() > 1 && raw.starts_with('0')) {
        return Err(FormError::Index);
    }
    if !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(FormError::Index);
    }
    raw.parse().map_err(|_| FormError::Index)
}

fn parse_revision(raw: &str) -> Result<u64, FormError> {
    let raw = raw.trim();
    if raw.is_empty() || (raw.len() > 1 && raw.starts_with('0')) {
        return Err(FormError::Revision);
    }
    if !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(FormError::Revision);
    }
    raw.parse().map_err(|_| FormError::Revision)
}

fn parse_review_policy(raw: &str) -> Result<bool, FormError> {
    match raw {
        "none" => Ok(false),
        "review-verdict" => Ok(true),
        _ => Err(FormError::ReviewPolicy),
    }
}

fn parse_human_revision_policy(raw: &str) -> Result<bool, FormError> {
    match raw {
        "none" => Ok(false),
        "request-changes" | "human-revision" => Ok(true),
        _ => Err(FormError::HumanRevisionPolicy),
    }
}

fn parse_intent(raw: &str) -> Result<FormIntent, FormError> {
    match raw {
        "save" => Ok(FormIntent::Save),
        "preview-execution" => Ok(FormIntent::PreviewExecution),

        "add-role" => Ok(FormIntent::AddRole),
        "add-step" => Ok(FormIntent::AddStep),

        value if value.starts_with("add-phase:") => {
            let purpose = value
                .strip_prefix("add-phase:")
                .and_then(PhasePurpose::parse)
                .ok_or(FormError::Purpose)?;
            Ok(FormIntent::AddPhase(purpose))
        }
        other => parse_indexed_intent(other),
    }
}

fn parse_indexed_intent(raw: &str) -> Result<FormIntent, FormError> {
    let mut parts = raw.split(':');
    let action = parts.next().ok_or(FormError::Intent)?;
    let first = parse_index(parts.next().ok_or(FormError::Intent)?)?;
    match action {
        "remove-role" if parts.next().is_none() => Ok(FormIntent::RemoveRole(first)),
        "move-role-up" if parts.next().is_none() => Ok(FormIntent::MoveRoleUp(first)),
        "move-role-down" if parts.next().is_none() => Ok(FormIntent::MoveRoleDown(first)),
        "remove-step" if parts.next().is_none() => Ok(FormIntent::RemoveStep(first)),
        "move-step-up" if parts.next().is_none() => Ok(FormIntent::MoveStepUp(first)),
        "move-step-down" if parts.next().is_none() => Ok(FormIntent::MoveStepDown(first)),
        "update-review-policy" if parts.next().is_none() => {
            Ok(FormIntent::UpdateReviewPolicy(first))
        }
        "apply-preset" if parts.next().is_none() => Ok(FormIntent::ApplyPreset { step: first }),
        "set-purpose" => {
            let purpose = parts
                .next()
                .and_then(PhasePurpose::parse)
                .ok_or(FormError::Purpose)?;
            if parts.next().is_some() {
                return Err(FormError::Intent);
            }
            Ok(FormIntent::SetPhasePurpose {
                step: first,
                purpose,
            })
        }
        "add-directory" if parts.next().is_none() => Ok(FormIntent::AddDirectory { step: first }),
        "remove-directory" => {
            let directory = parse_index(parts.next().ok_or(FormError::Intent)?)?;
            if parts.next().is_some() {
                return Err(FormError::Intent);
            }
            Ok(FormIntent::RemoveDirectory {
                step: first,
                directory,
            })
        }
        "add-input" if parts.next().is_none() => Ok(FormIntent::AddInput(first)),
        "remove-input" => {
            let input = parse_index(parts.next().ok_or(FormError::Intent)?)?;
            if parts.next().is_some() {
                return Err(FormError::Intent);
            }
            Ok(FormIntent::RemoveInput { step: first, input })
        }
        "add-output" if parts.next().is_none() => Ok(FormIntent::AddOutput(first)),
        "remove-output" => {
            let output = parse_index(parts.next().ok_or(FormError::Intent)?)?;
            if parts.next().is_some() {
                return Err(FormError::Intent);
            }
            Ok(FormIntent::RemoveOutput {
                step: first,
                output,
            })
        }
        "move-output-up" => {
            let output = parse_index(parts.next().ok_or(FormError::Intent)?)?;
            if parts.next().is_some() {
                return Err(FormError::Intent);
            }
            Ok(FormIntent::MoveOutputUp {
                step: first,
                output,
            })
        }
        "move-output-down" => {
            let output = parse_index(parts.next().ok_or(FormError::Intent)?)?;
            if parts.next().is_some() {
                return Err(FormError::Intent);
            }
            Ok(FormIntent::MoveOutputDown {
                step: first,
                output,
            })
        }
        _ => Err(FormError::Intent),
    }
}

fn collect_roles(fields: Vec<(usize, RolePart, String)>) -> Result<Vec<RoleDraft>, FormError> {
    let count = dense_count(fields.iter().map(|(index, _, _)| *index))?;
    if count > MAXIMUM_ROLES {
        return Err(FormError::Excessive);
    }
    let mut roles = vec![
        RoleDraft {
            key: String::new(),
            name: String::new(),
            expertise: String::new(),
            prompt: String::new(),
        };
        count
    ];
    for (index, part, value) in fields {
        let role = &mut roles[index];
        match part {
            RolePart::Key => role.key = value,
            RolePart::Name => role.name = value,
            RolePart::Expertise => role.expertise = value,
            RolePart::Prompt => role.prompt = value,
        }
    }
    Ok(roles)
}

fn collect_steps(fields: Vec<(usize, StepPart, String)>) -> Result<Vec<StepDraft>, FormError> {
    let count = dense_count(fields.iter().map(|(index, _, _)| *index))?;
    if count > MAXIMUM_STEPS {
        return Err(FormError::Excessive);
    }
    let mut steps = vec![empty_step(); count];
    let mut dir_seen = vec![Vec::<usize>::new(); count];
    let mut input_seen = vec![Vec::<usize>::new(); count];
    let mut output_seen = vec![Vec::<usize>::new(); count];
    let mut review_policy = vec![None; count];
    let mut review_drafts = vec![empty_review_policy(); count];
    let mut review_detail_seen = vec![[false; 3]; count];
    let mut human_revision_policy = vec![None; count];
    let mut human_revision_drafts = vec![empty_human_revision(); count];
    let mut human_revision_detail_seen = vec![[false; 2]; count];
    for (index, part, value) in fields {
        let step = &mut steps[index];
        match part {
            StepPart::Key => step.key = value,
            StepPart::Name => step.name = value,
            StepPart::Purpose => step.purpose = value,
            StepPart::Expertise => step.expertise = value,
            StepPart::Instructions => step.instructions = value,
            StepPart::Action => step.action = value,
            StepPart::Environment => step.environment = value,
            StepPart::Role => step.role = value,
            StepPart::CandidateAccess => step.directory_access = value,
            StepPart::Command => step.command = value,
            StepPart::ReviewPolicy => {
                review_policy[index] = Some(parse_review_policy(&value)?);
            }
            StepPart::HumanRevisionPolicy => {
                human_revision_policy[index] = Some(parse_human_revision_policy(&value)?);
            }
            StepPart::HumanRevisionTarget => {
                human_revision_drafts[index].revision_target = value;
                human_revision_detail_seen[index][0] = true;
            }
            StepPart::HumanAttemptLimit => {
                human_revision_drafts[index].attempt_limit = value;
                human_revision_detail_seen[index][1] = true;
            }
            StepPart::ReportOutput => {
                review_drafts[index].report_output = value;
                review_detail_seen[index][0] = true;
            }
            StepPart::RevisionTarget => {
                review_drafts[index].revision_target = value;
                review_detail_seen[index][1] = true;
            }
            StepPart::AttemptLimit => {
                review_drafts[index].attempt_limit = value;
                review_detail_seen[index][2] = true;
            }
            StepPart::Tool(tool) => {
                if !is_checked(&value) && value != tool.as_str() {
                    return Err(FormError::UnknownField);
                }
                if !step.tools.contains(&tool) {
                    step.tools.push(tool);
                }
            }
            StepPart::SettingsSource => step.settings_source = value,
            StepPart::Provider => step.provider = value,
            StepPart::Model => step.model = value,
            StepPart::Thinking => step.thinking = value,
            StepPart::SettingsInstructions => step.settings_instructions = value,
            StepPart::Network => step.network = value,
            StepPart::NetworkDomains => step.network_domains = value,
            StepPart::SettingsReadOnly => step.settings_read_only = value,
            StepPart::SettingsDirect => step.settings_direct = value,
            StepPart::Location => step.location = value,
            StepPart::HostApproval => step.host_approval = value,
            StepPart::SettingsPreset => step.settings_preset = value,
            StepPart::SettingsInherit(field) => {
                if value != "1" {
                    return Err(FormError::UnknownField);
                }
                step.settings_inherit.push(field.to_owned());
            }
            StepPart::SettingsGrants => {
                let values: Vec<String> =
                    serde_json::from_str(&value).map_err(|_| FormError::UnknownField)?;
                step.settings_grants = values
                    .iter()
                    .map(|value| DirectoryGrant::parse_form(value).ok_or(FormError::UnknownField))
                    .collect::<Result<_, _>>()?;
                crate::execution::validate_directories(&step.settings_grants)
                    .map_err(|_| FormError::Excessive)?;
            }
            StepPart::Dir {
                index: dir,
                part: dir_part,
            } => {
                ensure_row(&mut step.directories, dir, || DirectoryDraft {
                    alias: String::new(),
                    access: AccessMode::ReadOnly.as_str().to_owned(),
                })?;
                dir_seen[index].push(dir);
                match dir_part {
                    DirPart::Alias => step.directories[dir].alias = value,
                    DirPart::Access => step.directories[dir].access = value,
                }
            }
            StepPart::Input {
                index: input,
                part: input_part,
            } => {
                ensure_row(&mut step.inputs, input, || InputDraft {
                    key: String::new(),
                    kind: ArtefactKind::Plan.as_str().to_owned(),
                    source: String::new(),
                })?;
                input_seen[index].push(input);
                match input_part {
                    InputPart::Key => step.inputs[input].key = value,
                    InputPart::Kind => step.inputs[input].kind = value,
                    InputPart::Source => step.inputs[input].source = value,
                }
            }
            StepPart::Output {
                index: output,
                part: output_part,
            } => {
                ensure_row(&mut step.outputs, output, || OutputDraft {
                    key: String::new(),
                    kind: OutputKind::AssistantReply.as_str().to_owned(),
                })?;
                output_seen[index].push(output);
                match output_part {
                    OutputPart::Key => step.outputs[output].key = value,
                    OutputPart::Kind => step.outputs[output].kind = value,
                }
            }
        }
    }
    for index in 0..steps.len() {
        let Some(has_review_policy) = review_policy[index] else {
            return Err(FormError::MissingField);
        };
        let details_present = review_detail_seen[index].iter().any(|seen| *seen);
        let details_complete = review_detail_seen[index].iter().all(|seen| *seen);
        if details_present && !details_complete {
            return Err(FormError::MissingField);
        }
        if has_review_policy {
            steps[index].review_policy = Some(review_drafts[index].clone());
        }
        if let Some(has_human_revision) = human_revision_policy[index] {
            let details_present = human_revision_detail_seen[index].iter().any(|seen| *seen);
            let details_complete = human_revision_detail_seen[index].iter().all(|seen| *seen);
            if details_present && !details_complete {
                return Err(FormError::MissingField);
            }
            if has_human_revision {
                steps[index].human_revision = Some(human_revision_drafts[index].clone());
            }
        }
        let step = &steps[index];
        if !dir_seen[index].is_empty() {
            dense_count(dir_seen[index].iter().copied())?;
            if step.directories.len() > MAXIMUM_DIRECTORIES {
                return Err(FormError::Excessive);
            }
        }
        if !input_seen[index].is_empty() {
            dense_count(input_seen[index].iter().copied())?;
            if step.inputs.len() > MAXIMUM_INPUTS {
                return Err(FormError::Excessive);
            }
        }
        if !output_seen[index].is_empty() {
            dense_count(output_seen[index].iter().copied())?;
            if step.outputs.len() > MAXIMUM_OUTPUTS {
                return Err(FormError::Excessive);
            }
        }
    }
    Ok(steps)
}

fn dense_count(indices: impl Iterator<Item = usize>) -> Result<usize, FormError> {
    let mut max = None;
    let mut seen = Vec::new();
    for index in indices {
        if !seen.contains(&index) {
            seen.push(index);
        }
        max = Some(max.map_or(index, |current: usize| current.max(index)));
    }
    let Some(max) = max else {
        return Ok(0);
    };
    let count = max.checked_add(1).ok_or(FormError::Excessive)?;
    if seen.len() != count {
        return Err(FormError::Sparse);
    }
    Ok(count)
}

fn ensure_row<T>(rows: &mut Vec<T>, index: usize, make: impl Fn() -> T) -> Result<(), FormError> {
    if index >= MAXIMUM_DIRECTORIES.max(MAXIMUM_OUTPUTS).max(MAXIMUM_INPUTS) {
        return Err(FormError::Excessive);
    }
    while rows.len() <= index {
        rows.push(make());
    }
    Ok(())
}

fn is_checked(value: &str) -> bool {
    matches!(value, "on" | "true" | "1")
}

fn ensure_action_defaults(steps: &mut [StepDraft]) {
    for step in steps {
        if step.action == "agent" && step.outputs.is_empty() {
            step.outputs.push(OutputDraft {
                key: crate::workflows::definition::ASSISTANT_REPLY.to_owned(),
                kind: OutputKind::AssistantReply.as_str().to_owned(),
            });
        }
    }
}

fn normalise_phase_contracts(steps: &mut [StepDraft]) {
    for index in 0..steps.len() {
        let new_phase = steps[index].key.is_empty();
        let key = if steps[index].key.trim().is_empty() {
            let existing: Vec<_> = steps.iter().map(|step| step.key.as_str()).collect();
            next_key("phase", &existing)
        } else {
            steps[index].key.clone()
        };
        steps[index].key = key;
        let explicit_purpose = PhasePurpose::parse(&steps[index].purpose).is_some();
        let purpose = phase_purpose(&steps[index]);
        if steps[index].purpose.is_empty() {
            steps[index].purpose = purpose.as_str().to_owned();
        }
        if matches!(
            purpose,
            PhasePurpose::Planning
                | PhasePurpose::PlanReview
                | PhasePurpose::Implementation
                | PhasePurpose::ReadOnlyReview
                | PhasePurpose::ReviewAndFix
        ) && steps[index].role.trim().is_empty()
        {
            let existing: Vec<_> = steps.iter().map(|step| step.role.as_str()).collect();
            steps[index].role = next_key("role", &existing);
        }
        if new_phase && steps[index].name.is_empty() {
            steps[index].name = purpose.default_name().to_owned();
        }
        if new_phase && steps[index].expertise.is_empty() {
            steps[index].expertise = purpose.default_expertise().to_owned();
        }
        if new_phase && steps[index].instructions.is_empty() {
            steps[index].instructions = purpose.default_instructions().to_owned();
        }
        if explicit_purpose && purpose != PhasePurpose::Custom {
            apply_phase_contract(steps, index, purpose);
        }
    }
}

fn inferred_purpose(step: &StepDefinition) -> PhasePurpose {
    match &step.action {
        StepAction::Agent(action) => {
            if action.directory_access == StepAccess::Write {
                if action
                    .required_outputs
                    .iter()
                    .any(|output| output.kind == OutputKind::ReviewReport)
                {
                    PhasePurpose::ReviewAndFix
                } else {
                    PhasePurpose::Implementation
                }
            } else if action
                .required_outputs
                .iter()
                .any(|output| output.kind == OutputKind::Plan)
            {
                if step
                    .inputs
                    .iter()
                    .any(|input| input.kind == ArtefactKind::Plan)
                {
                    PhasePurpose::PlanReview
                } else {
                    PhasePurpose::Planning
                }
            } else {
                PhasePurpose::ReadOnlyReview
            }
        }
        StepAction::SystemCommand(_) => PhasePurpose::Custom,
        StepAction::HumanGate(_) => PhasePurpose::PlanCheckpoint,
    }
}

fn phase_purpose(step: &StepDraft) -> PhasePurpose {
    PhasePurpose::parse(&step.purpose).unwrap_or_else(|| {
        if step.action == "human-gate" {
            PhasePurpose::PlanCheckpoint
        } else if step.action == "agent" {
            let writes = step.directory_access == StepAccess::Write.as_str();
            let has_plan = step
                .outputs
                .iter()
                .any(|output| output.kind == OutputKind::Plan.as_str());
            let has_review = step
                .outputs
                .iter()
                .any(|output| output.kind == OutputKind::ReviewReport.as_str());
            match (writes, has_plan, has_review) {
                (false, true, _) => PhasePurpose::Planning,
                (true, _, true) => PhasePurpose::ReviewAndFix,
                (true, _, _) => PhasePurpose::Implementation,
                _ => PhasePurpose::ReadOnlyReview,
            }
        } else {
            PhasePurpose::Custom
        }
    })
}

fn apply_phase_contract(steps: &mut [StepDraft], index: usize, purpose: PhasePurpose) {
    let earlier = steps[..index].to_vec();
    let current = &mut steps[index];
    current.purpose = purpose.as_str().to_owned();
    match purpose {
        PhasePurpose::Planning
        | PhasePurpose::PlanReview
        | PhasePurpose::ReadOnlyReview
        | PhasePurpose::Implementation
        | PhasePurpose::ReviewAndFix => {
            current.action = "agent".to_owned();
            current.directory_access = if matches!(
                purpose,
                PhasePurpose::Implementation | PhasePurpose::ReviewAndFix
            ) {
                StepAccess::Write
            } else {
                StepAccess::Read
            }
            .as_str()
            .to_owned();
            if current.role.is_empty() {
                current.role = next_key("role", &[]);
            }
            let kinds = match purpose {
                PhasePurpose::Planning | PhasePurpose::PlanReview => {
                    vec![OutputKind::AssistantReply, OutputKind::Plan]
                }
                PhasePurpose::ReadOnlyReview | PhasePurpose::ReviewAndFix => {
                    vec![OutputKind::AssistantReply, OutputKind::ReviewReport]
                }
                _ => vec![OutputKind::AssistantReply],
            };
            current
                .outputs
                .retain(|output| output.kind != OutputKind::PlanDecision.as_str());
            current.outputs = preserve_outputs(&current.outputs, &kinds, &[]);
        }
        PhasePurpose::PlanCheckpoint => {
            current.action = "human-gate".to_owned();
            current.environment.clear();
            current.role.clear();
            current.tools.clear();
            current.directories.clear();
            current
                .outputs
                .retain(|output| output.kind == OutputKind::PlanDecision.as_str());
            current.outputs = preserve_outputs(
                &current.outputs,
                &[OutputKind::PlanDecision],
                &["plan-decision"],
            );
        }
        _ => {}
    }
    current.inputs = preserve_inputs(&current.inputs, &contract_inputs(purpose, &earlier));
}

fn preserve_outputs(
    existing: &[OutputDraft],
    kinds: &[OutputKind],
    fallback: &[&str],
) -> Vec<OutputDraft> {
    let mut outputs = existing.to_vec();
    for (index, kind) in kinds.iter().enumerate() {
        if !outputs
            .iter()
            .any(|output| OutputKind::parse(&output.kind) == Some(*kind))
        {
            let base = fallback
                .get(index)
                .copied()
                .unwrap_or_else(|| default_output_key(*kind));
            let keys: Vec<_> = outputs.iter().map(|output| output.key.as_str()).collect();
            let key = if keys.contains(&base) {
                next_key(base, &keys)
            } else {
                base.to_owned()
            };
            outputs.push(OutputDraft {
                key,
                kind: kind.as_str().to_owned(),
            });
        }
    }
    outputs
}

fn default_output_key(kind: OutputKind) -> &'static str {
    match kind {
        OutputKind::AssistantReply => "assistant-reply",
        OutputKind::Plan => "plan",
        OutputKind::ReviewReport => "review",
        OutputKind::TestReport => "test",
        OutputKind::PlanDecision => "plan-decision",
    }
}

fn contract_inputs(purpose: PhasePurpose, earlier: &[StepDraft]) -> Vec<(ArtefactKind, String)> {
    let mut inputs = Vec::new();
    if matches!(
        purpose,
        PhasePurpose::PlanReview
            | PhasePurpose::PlanCheckpoint
            | PhasePurpose::Implementation
            | PhasePurpose::ReviewAndFix
    ) && latest_output_source(earlier, OutputKind::Plan).is_some()
    {
        inputs.push((ArtefactKind::Plan, "run-current-plan".to_owned()));
    }
    if matches!(
        purpose,
        PhasePurpose::Implementation | PhasePurpose::ReviewAndFix
    ) && let Some(source) = latest_output_source(earlier, OutputKind::PlanDecision)
    {
        inputs.push((ArtefactKind::PlanDecision, source));
    }
    inputs
}

fn latest_output_source(steps: &[StepDraft], kind: OutputKind) -> Option<String> {
    steps.iter().rev().find_map(|step| {
        step.outputs.iter().rev().find_map(|output| {
            (OutputKind::parse(&output.kind) == Some(kind))
                .then(|| format!("step-output:{}:{}", step.key, output.key))
        })
    })
}

fn preserve_inputs(existing: &[InputDraft], specs: &[(ArtefactKind, String)]) -> Vec<InputDraft> {
    let mut inputs = existing.to_vec();
    for (index, (kind, source)) in specs.iter().enumerate() {
        let required = specs[..=index]
            .iter()
            .filter(|item| item.0 == *kind)
            .count();
        if inputs
            .iter()
            .filter(|input| ArtefactKind::parse(&input.kind) == Some(*kind))
            .count()
            < required
        {
            let keys: Vec<_> = inputs.iter().map(|input| input.key.as_str()).collect();
            let base = default_input_key(*kind, required - 1);
            let key = if keys.contains(&base.as_str()) {
                next_key(&base, &keys)
            } else {
                base
            };
            inputs.push(InputDraft {
                key,
                kind: kind.as_str().to_owned(),
                source: source.clone(),
            });
        }
    }
    inputs
}

fn default_input_key(kind: ArtefactKind, occurrence: usize) -> String {
    let base = match kind {
        ArtefactKind::Plan => "plan",
        ArtefactKind::ReviewReport => "review",
        ArtefactKind::TestReport => "test",
        ArtefactKind::PlanDecision => "plan-decision",
    };
    if occurrence == 0 {
        base.to_owned()
    } else {
        format!("{base}-{}", occurrence + 1)
    }
}

pub(super) fn source_is_valid(raw: &str, kind: ArtefactKind, earlier: &[StepDraft]) -> bool {
    if raw == "run-current-plan" {
        return kind == ArtefactKind::Plan
            && earlier.iter().any(|step| {
                step.outputs
                    .iter()
                    .any(|output| output.kind == OutputKind::Plan.as_str())
            });
    }
    let Some(rest) = raw.strip_prefix("step-output:") else {
        return false;
    };
    let Some((step, output)) = rest.split_once(':') else {
        return false;
    };
    earlier.iter().any(|candidate| {
        candidate.key == step
            && candidate.outputs.iter().any(|item| {
                item.key == output
                    && OutputKind::parse(&item.kind).and_then(OutputKind::as_artefact_kind)
                        == Some(kind)
            })
    })
}

pub(super) fn input_removal_breaks_contract(step: &StepDraft, input: usize) -> bool {
    step.action == "human-gate"
        && step
            .inputs
            .get(input)
            .is_some_and(|input| input.kind == ArtefactKind::Plan.as_str())
}

pub(super) fn output_is_required(step: &StepDraft, output: usize) -> bool {
    step.outputs.get(output).is_some_and(|output| {
        let kinds = match phase_purpose(step) {
            PhasePurpose::Planning | PhasePurpose::PlanReview => {
                vec![OutputKind::AssistantReply, OutputKind::Plan]
            }
            PhasePurpose::Implementation => vec![OutputKind::AssistantReply],
            PhasePurpose::ReadOnlyReview | PhasePurpose::ReviewAndFix => {
                vec![OutputKind::AssistantReply, OutputKind::ReviewReport]
            }
            PhasePurpose::PlanCheckpoint => vec![OutputKind::PlanDecision],
            _ => vec![],
        };
        OutputKind::parse(&output.kind).is_some_and(|kind| kinds.contains(&kind))
    })
}

fn output_is_referenced(steps: &[StepDraft], step: usize, output: usize) -> bool {
    let Some(source) = steps
        .get(step)
        .and_then(|row| row.outputs.get(output))
        .map(|item| format!("step-output:{}:{}", steps[step].key, item.key))
    else {
        return false;
    };
    steps.iter().enumerate().any(|(index, row)| {
        (index != step && row.inputs.iter().any(|input| input.source == source))
            || row.review_policy.as_ref().is_some_and(|policy| {
                index == step && policy.report_output == steps[step].outputs[output].key
            })
    })
}

fn sync_phase_text_from_roles(steps: &mut [StepDraft], roles: &[RoleDraft]) {
    for step in steps.iter_mut().filter(|step| step.action == "agent") {
        let Some(role) = roles.iter().find(|role| role.key == step.role) else {
            continue;
        };
        step.expertise = role.expertise.clone();
        step.instructions = role.prompt.clone();
    }
}

fn empty_review_policy() -> ReviewPolicyDraft {
    ReviewPolicyDraft {
        report_output: String::new(),
        revision_target: String::new(),
        attempt_limit: "3".to_owned(),
    }
}

fn empty_human_revision() -> HumanRevisionDraft {
    HumanRevisionDraft {
        revision_target: String::new(),
        attempt_limit: "3".to_owned(),
    }
}

fn with_empty_settings(mut step: StepDraft) -> StepDraft {
    step.settings_source = "defaults".to_owned();
    step.network = "none".to_owned();
    step
}

fn empty_step() -> StepDraft {
    with_empty_settings(StepDraft {
        key: String::new(),
        name: String::new(),
        purpose: String::new(),
        expertise: String::new(),
        instructions: String::new(),
        action: "agent".to_owned(),
        environment: String::new(),
        role: String::new(),
        directory_access: StepAccess::Write.as_str().to_owned(),
        command: SystemCommandId::RepositoryStatus.as_str().to_owned(),
        tools: Vec::new(),
        settings_direct: String::new(),
        location: String::new(),
        host_approval: String::new(),
        settings_source: String::new(),
        provider: String::new(),
        model: String::new(),
        thinking: String::new(),
        settings_instructions: String::new(),
        network: String::new(),
        network_domains: String::new(),
        settings_read_only: String::new(),

        settings_preset: String::new(),
        settings_grants: Vec::new(),
        settings_inherit: Vec::new(),
        directories: Vec::new(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        review_policy: None,
        human_revision: None,
    })
}

fn blank_agent_step(key: &str, role: &str) -> StepDraft {
    with_empty_settings(StepDraft {
        key: key.to_owned(),
        name: String::new(),
        purpose: PhasePurpose::Implementation.as_str().to_owned(),
        expertise: PhasePurpose::Implementation.default_expertise().to_owned(),
        instructions: PhasePurpose::Implementation
            .default_instructions()
            .to_owned(),
        action: "agent".to_owned(),
        environment: String::new(),
        role: role.to_owned(),
        directory_access: StepAccess::Write.as_str().to_owned(),
        command: SystemCommandId::RepositoryStatus.as_str().to_owned(),
        tools: ToolId::ALL.to_vec(),
        settings_direct: String::new(),
        location: String::new(),
        host_approval: String::new(),
        settings_source: String::new(),
        provider: String::new(),
        model: String::new(),
        thinking: String::new(),
        settings_instructions: String::new(),
        network: String::new(),
        network_domains: String::new(),
        settings_read_only: String::new(),

        settings_preset: String::new(),
        settings_grants: Vec::new(),
        settings_inherit: Vec::new(),
        directories: Vec::new(),
        inputs: Vec::new(),
        review_policy: None,
        human_revision: None,
        outputs: vec![OutputDraft {
            key: "assistant-reply".to_owned(),
            kind: OutputKind::AssistantReply.as_str().to_owned(),
        }],
    })
}

fn phase_draft(key: &str, purpose: PhasePurpose, role: &str) -> StepDraft {
    let mut draft = blank_agent_step(key, if role.is_empty() { "role-1" } else { role });
    draft.name = purpose.default_name().to_owned();
    draft.purpose = purpose.as_str().to_owned();
    draft.tools = purpose.default_tools();
    draft.expertise = purpose.default_expertise().to_owned();
    draft.instructions = purpose.default_instructions().to_owned();
    draft
}

fn input_from_required(input: &RequiredInput) -> InputDraft {
    InputDraft {
        key: input.key.as_str().to_owned(),
        kind: input.kind.as_str().to_owned(),
        source: source_token(&input.source),
    }
}

fn output_from_required(output: &RequiredOutput) -> OutputDraft {
    OutputDraft {
        key: output.key.as_str().to_owned(),
        kind: output.kind.as_str().to_owned(),
    }
}

fn source_token(source: &ArtefactSource) -> String {
    match source {
        ArtefactSource::RunCurrentPlan => "run-current-plan".to_owned(),

        ArtefactSource::StepOutput { step, output } => {
            format!("step-output:{}:{}", step.as_str(), output.as_str())
        }
    }
}

fn parse_source(
    raw: &str,
) -> Result<ArtefactSource, crate::workflows::definition::DefinitionError> {
    if raw == "run-current-plan" {
        return Ok(ArtefactSource::RunCurrentPlan);
    }

    let Some(rest) = raw.strip_prefix("step-output:") else {
        return Err(crate::workflows::definition::DefinitionError::Format);
    };
    let Some((step, output)) = rest.split_once(':') else {
        return Err(crate::workflows::definition::DefinitionError::Format);
    };
    Ok(ArtefactSource::StepOutput {
        step: StepKey::parse(step)?,
        output: OutputKey::parse(output)?,
    })
}

fn step_from_definition(step: &StepDefinition) -> StepDraft {
    let review_policy = step.review.as_ref().map(|policy| ReviewPolicyDraft {
        report_output: policy.report_output.as_str().to_owned(),
        revision_target: policy.revision_target.as_str().to_owned(),
        attempt_limit: policy.attempt_limit.to_string(),
    });
    match &step.action {
        StepAction::Agent(action) => {
            let directories: Vec<DirectoryDraft> = action
                .authority
                .directories
                .iter()
                .map(|directory| DirectoryDraft {
                    alias: directory.alias.clone(),
                    access: directory.access.as_str().to_owned(),
                })
                .collect();
            StepDraft {
                key: step.key.as_str().to_owned(),
                name: step.name.clone(),
                purpose: inferred_purpose(step).as_str().to_owned(),
                expertise: String::new(),
                instructions: String::new(),
                action: "agent".to_owned(),
                environment: match &action.settings {
                    ModelStepSettings::Override(settings) => settings
                        .environment
                        .map(|value| value.as_hex())
                        .unwrap_or_default(),
                    ModelStepSettings::SameAsRunDefaults => {
                        step_environment_token(action.environment)
                    }
                },
                role: action.role.as_str().to_owned(),
                directory_access: action.directory_access.as_str().to_owned(),
                command: SystemCommandId::RepositoryStatus.as_str().to_owned(),
                tools: match &action.settings {
                    ModelStepSettings::Override(settings) => {
                        settings.tools.clone().unwrap_or_default()
                    }
                    ModelStepSettings::SameAsRunDefaults => action.authority.tools.clone(),
                },
                settings_source: if action.settings.is_same_as_defaults() {
                    "defaults".to_owned()
                } else {
                    "override".to_owned()
                },
                provider: settings_provider(&action.settings),
                model: settings_model(&action.settings),
                thinking: settings_thinking(&action.settings),
                settings_instructions: settings_instructions_value(&action.settings),
                network: settings_network(&action.settings),
                network_domains: settings_network_domains(&action.settings),
                settings_read_only: settings_paths(&action.settings, DirectoryAccess::Read),

                settings_direct: settings_paths(&action.settings, DirectoryAccess::Write),
                location: settings_location(&action.settings),
                host_approval: settings_host_approval(&action.settings),
                settings_preset: String::new(),
                settings_grants: settings_grants(&action.settings),
                settings_inherit: inherited_fields(&action.settings),
                directories,
                inputs: step.inputs.iter().map(input_from_required).collect(),
                outputs: action
                    .required_outputs
                    .iter()
                    .map(output_from_required)
                    .collect(),
                review_policy: review_policy.clone(),
                human_revision: None,
            }
        }
        StepAction::SystemCommand(action) => StepDraft {
            key: step.key.as_str().to_owned(),
            name: step.name.clone(),
            purpose: inferred_purpose(step).as_str().to_owned(),
            expertise: String::new(),
            instructions: String::new(),
            action: "system-command".to_owned(),
            environment: step_environment_token(action.environment),
            role: String::new(),
            directory_access: String::new(),
            command: action.command.as_str().to_owned(),
            settings_direct: String::new(),
            location: String::new(),
            host_approval: String::new(),
            tools: Vec::new(),
            settings_source: "defaults".to_owned(),
            provider: String::new(),
            model: String::new(),
            thinking: String::new(),
            settings_instructions: String::new(),
            network: "none".to_owned(),
            network_domains: String::new(),
            settings_read_only: String::new(),

            settings_preset: String::new(),
            settings_grants: Vec::new(),
            settings_inherit: Vec::new(),
            directories: Vec::new(),
            inputs: step.inputs.iter().map(input_from_required).collect(),
            outputs: action
                .required_outputs
                .iter()
                .map(output_from_required)
                .collect(),
            review_policy: review_policy.clone(),
            human_revision: None,
        },
        StepAction::HumanGate(action) => {
            let purpose = inferred_purpose(step);
            StepDraft {
                key: step.key.as_str().to_owned(),
                name: step.name.clone(),
                purpose: purpose.as_str().to_owned(),
                expertise: purpose.default_expertise().to_owned(),
                instructions: purpose.default_instructions().to_owned(),
                action: "human-gate".to_owned(),
                settings_direct: String::new(),
                location: String::new(),
                host_approval: String::new(),
                environment: String::new(),
                role: String::new(),
                directory_access: String::new(),
                command: String::new(),
                tools: Vec::new(),
                settings_source: "defaults".to_owned(),
                provider: String::new(),
                model: String::new(),
                thinking: String::new(),
                settings_instructions: String::new(),
                network: "none".to_owned(),
                network_domains: String::new(),
                settings_read_only: String::new(),

                settings_preset: String::new(),
                settings_grants: Vec::new(),
                settings_inherit: Vec::new(),
                directories: Vec::new(),
                inputs: step.inputs.iter().map(input_from_required).collect(),
                outputs: vec![output_from_required(&action.required_output)],
                review_policy,
                human_revision: action.revision.as_ref().map(|revision| HumanRevisionDraft {
                    revision_target: revision.revision_target.as_str().to_owned(),
                    attempt_limit: revision.attempt_limit.to_string(),
                }),
            }
        }
    }
}

fn build_step(step: &StepDraft, errors: &mut StepErrors) -> Option<StepDefinition> {
    if PhasePurpose::parse(&step.purpose).is_none() {
        errors.purpose = FormError::Purpose.message();
        return None;
    }
    let key = match StepKey::parse(&step.key) {
        Ok(key) => key,
        Err(error) => {
            errors.key = error.message();
            return None;
        }
    };
    let display_name = step.name.trim().to_owned();
    if display_name.is_empty()
        || display_name.len() > 80
        || display_name.chars().any(char::is_control)
    {
        errors.name = crate::workflows::definition::DefinitionError::Name.message();
        return None;
    }
    let action = match step.action.as_str() {
        "agent" => build_agent_action(step, errors)?,
        "system-command" => build_command_action(step, errors)?,
        "human-gate" => build_gate_action(step, errors)?,
        _ => {
            errors.action = "Choose an action type.";
            return None;
        }
    };
    let mut inputs = Vec::new();
    for (index, input) in step.inputs.iter().enumerate() {
        let key = match InputKey::parse(&input.key) {
            Ok(key) => key,
            Err(error) => {
                errors.inputs[index].key = error.message();
                return None;
            }
        };
        let Some(kind) = ArtefactKind::parse(&input.kind) else {
            errors.inputs[index].kind =
                crate::workflows::definition::DefinitionError::Format.message();
            return None;
        };
        let source = match parse_source(&input.source) {
            Ok(source) => source,
            Err(error) => {
                errors.inputs[index].source = error.message();
                return None;
            }
        };
        inputs.push(RequiredInput { key, kind, source });
    }
    Some(StepDefinition {
        key,
        name: display_name,
        inputs,
        action,
        review: None,
    })
}

fn inherited_fields(settings: &ModelStepSettings) -> Vec<String> {
    let ModelStepSettings::Override(settings) = settings else {
        return Vec::new();
    };
    [
        ("model", settings.model.is_none()),
        ("instructions", settings.instructions.is_none()),
        ("tools", settings.tools.is_none()),
        ("network", settings.network.is_none()),
        ("directories", settings.directories.is_none()),
        ("environment", settings.environment.is_none()),
        ("location", settings.location.is_none()),
        ("host_approval", settings.host_approval.is_none()),
    ]
    .into_iter()
    .filter(|(_, inherit)| *inherit)
    .map(|(name, _)| name.to_owned())
    .collect()
}

fn settings_provider(settings: &ModelStepSettings) -> String {
    match settings {
        ModelStepSettings::Override(value) => value
            .model
            .as_ref()
            .map(|model| model.provider.as_str().to_owned())
            .unwrap_or_default(),
        ModelStepSettings::SameAsRunDefaults => String::new(),
    }
}

fn settings_model(settings: &ModelStepSettings) -> String {
    match settings {
        ModelStepSettings::Override(value) => value
            .model
            .as_ref()
            .map(|model| model.model.clone())
            .unwrap_or_default(),
        ModelStepSettings::SameAsRunDefaults => String::new(),
    }
}

fn settings_thinking(settings: &ModelStepSettings) -> String {
    match settings {
        ModelStepSettings::Override(value) => value
            .model
            .as_ref()
            .and_then(|model| model.thinking.as_ref())
            .map(|effort| effort.as_str().to_owned())
            .unwrap_or_default(),
        ModelStepSettings::SameAsRunDefaults => String::new(),
    }
}

fn settings_instructions_value(settings: &ModelStepSettings) -> String {
    match settings {
        ModelStepSettings::Override(value) => value.instructions.clone().unwrap_or_default(),
        ModelStepSettings::SameAsRunDefaults => String::new(),
    }
}

fn settings_network(settings: &ModelStepSettings) -> String {
    match settings {
        ModelStepSettings::Override(value) => value
            .network
            .as_ref()
            .map(|network| network.as_str().to_owned())
            .unwrap_or_else(|| "none".to_owned()),
        ModelStepSettings::SameAsRunDefaults => "none".to_owned(),
    }
}

fn settings_network_domains(settings: &ModelStepSettings) -> String {
    match settings {
        ModelStepSettings::Override(value) => value
            .network
            .as_ref()
            .map(|network| network.domains().join("\n"))
            .unwrap_or_default(),
        ModelStepSettings::SameAsRunDefaults => String::new(),
    }
}

fn settings_paths(settings: &ModelStepSettings, access: DirectoryAccess) -> String {
    match settings {
        ModelStepSettings::Override(value) => value
            .directories
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|grant| grant.access == access)
            .map(|grant| grant.host_path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n"),
        ModelStepSettings::SameAsRunDefaults => String::new(),
    }
}

fn settings_location(settings: &ModelStepSettings) -> String {
    match settings {
        ModelStepSettings::Override(value) => value
            .location
            .map(|location| location.as_str().to_owned())
            .unwrap_or_default(),
        ModelStepSettings::SameAsRunDefaults => String::new(),
    }
}

fn settings_host_approval(settings: &ModelStepSettings) -> String {
    match settings {
        ModelStepSettings::Override(value) => value
            .host_approval
            .map(|policy| policy.as_str().to_owned())
            .unwrap_or_default(),
        ModelStepSettings::SameAsRunDefaults => String::new(),
    }
}

fn settings_grants(settings: &ModelStepSettings) -> Vec<DirectoryGrant> {
    match settings {
        ModelStepSettings::Override(value) => value.directories.clone().unwrap_or_default(),
        ModelStepSettings::SameAsRunDefaults => Vec::new(),
    }
}

pub(super) fn fill_step_from_preset(step: &mut StepDraft, preset: &crate::presets::PresetRecord) {
    let settings = &preset.settings;
    step.settings_source = "override".to_owned();
    step.settings_inherit.clear();
    step.settings_preset = preset.id.as_hex();
    step.provider = settings.model.provider.as_str().to_owned();
    step.model = settings.model.model.clone();
    step.thinking = settings
        .model
        .thinking
        .as_ref()
        .map(|effort| effort.as_str().to_owned())
        .unwrap_or_default();
    step.settings_instructions = settings.instructions.clone();
    step.tools.clone_from(&settings.tools);
    step.network = settings.network.as_str().to_owned();
    step.network_domains = settings.network.domains().join("\n");
    step.settings_read_only = settings
        .directories
        .iter()
        .filter(|grant| grant.access == DirectoryAccess::Read)
        .map(|grant| grant.host_path.display().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    step.settings_direct = settings
        .directories
        .iter()
        .filter(|grant| grant.access == DirectoryAccess::Write)
        .map(|grant| grant.host_path.display().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    step.settings_grants = settings.directories.clone();
    step.environment = settings.environment.as_hex();
    step.location = settings.location.as_str().to_owned();
    step.host_approval = settings.host_approval.as_str().to_owned();
}

fn parse_override_settings(step: &StepDraft, errors: &mut StepErrors) -> Option<SettingsOverrides> {
    let inherits = |field: &str| step.settings_inherit.iter().any(|value| value == field);
    use crate::agents::NetworkAccess;
    use crate::environments::EnvironmentId;
    use crate::providers::{ModelSelection, ProviderKind, ThinkingEffort};
    let model = if inherits("model") {
        None
    } else {
        let provider = match ProviderKind::parse(step.provider.trim()) {
            Some(provider) => provider,
            None => {
                errors.settings = "Choose a listed provider.";
                return None;
            }
        };
        let thinking = if step.thinking.trim().is_empty() {
            None
        } else {
            match ThinkingEffort::new(step.thinking.clone()) {
                Some(effort) => Some(effort),
                None => {
                    errors.settings = "Enter a valid reasoning effort.";
                    return None;
                }
            }
        };
        let model = match ModelSelection::new(provider, step.model.clone(), thinking) {
            Some(model) => model,
            None => {
                errors.settings = "Enter a valid model name.";
                return None;
            }
        };
        Some(model)
    };
    let environment = if inherits("environment") {
        None
    } else {
        Some(match EnvironmentId::parse(&step.environment) {
            Some(id) => id,
            None if step.environment.is_empty() => {
                errors.environment = "Choose an environment.";
                return None;
            }
            None => {
                errors.environment = "Choose a ready environment.";
                return None;
            }
        })
    };
    let network = if inherits("network") {
        None
    } else {
        let network_mode = if step.network.trim().is_empty() {
            "none"
        } else {
            step.network.as_str()
        };
        let network = match NetworkAccess::parse_form(network_mode, &step.network_domains) {
            Ok(network) => network,
            Err(_) => {
                errors.settings =
                    "Choose valid network access. Restricted access needs 1 to 32 domains.";
                return None;
            }
        };
        Some(network)
    };
    let mut reserved = step.settings_grants.clone();
    let mut directories = Vec::new();
    for (text, access) in [
        (&step.settings_read_only, DirectoryAccess::Read),
        (&step.settings_direct, DirectoryAccess::Write),
    ]
    .into_iter()
    .filter(|_| !inherits("directories"))
    {
        for path in text.lines().filter(|line| !line.trim().is_empty()) {
            let existing = step
                .settings_grants
                .iter()
                .find(|grant| grant.host_path == std::path::Path::new(path));
            let mut grant = match existing {
                Some(grant) => grant.clone(),
                None => {
                    match DirectoryGrant::from_selected(std::path::Path::new(path), &reserved) {
                        Ok(grant) => {
                            reserved.push(grant.clone());
                            grant
                        }
                        Err(_) => {
                            errors.settings =
                                "Choose existing, distinct directories without overlapping roots.";
                            return None;
                        }
                    }
                }
            };
            grant.access = access;
            directories.push(grant);
        }
    }
    let location = if inherits("location") {
        None
    } else {
        Some(
            crate::execution::ToolLocation::parse(if step.location.trim().is_empty() {
                crate::execution::ToolLocation::Sandbox.as_str()
            } else {
                step.location.trim()
            })
            .ok_or_else(|| {
                errors.settings = "Choose Sandbox or This computer.";
            })
            .ok()?,
        )
    };
    if location == Some(crate::execution::ToolLocation::Host)
        && directories
            .iter()
            .any(|grant| grant.access == DirectoryAccess::Read)
    {
        errors.settings = "Read access requires a sandbox. Host tools need Write access.";
        return None;
    }
    let host_approval = if inherits("host_approval") {
        None
    } else {
        Some(
            crate::execution::HostApprovalPolicy::parse(if step.host_approval.trim().is_empty() {
                crate::execution::HostApprovalPolicy::AskEachTime.as_str()
            } else {
                step.host_approval.trim()
            })
            .ok_or_else(|| {
                errors.settings = "Choose Ask each time or Automatic (YOLO).";
            })
            .ok()?,
        )
    };
    let settings = SettingsOverrides {
        model,
        environment,
        network,
        instructions: (!inherits("instructions")).then(|| step.settings_instructions.clone()),
        tools: (!inherits("tools")).then(|| step.tools.clone()),
        directories: (!inherits("directories")).then_some(directories),
        location,
        host_approval,
    };
    if settings.validate().is_none() {
        errors.settings =
            "Use bounded instructions, unique tools and at most eight distinct directory roots.";
        return None;
    }
    Some(settings)
}

fn step_settings(step: &StepDraft, errors: &mut StepErrors) -> Option<ModelStepSettings> {
    match step.settings_source.as_str() {
        "" | "defaults" => Some(ModelStepSettings::SameAsRunDefaults),
        "override" => Some(ModelStepSettings::Override(Box::new(
            parse_override_settings(step, errors)?,
        ))),
        _ => {
            errors.settings = "Choose Same as run defaults or custom settings.";
            None
        }
    }
}

fn build_agent_action(step: &StepDraft, errors: &mut StepErrors) -> Option<StepAction> {
    let role = match RoleKey::parse(&step.role) {
        Ok(role) => role,
        Err(error) => {
            errors.role = error.message();
            return None;
        }
    };
    let directory_access = match StepAccess::parse(&step.directory_access) {
        Some(authority) => authority,
        None => {
            errors.directory_access = "Choose Read or Write access.";
            return None;
        }
    };
    let mut directories = Vec::new();
    for directory in &step.directories {
        if directory.alias.trim().is_empty() {
            continue;
        }
        directories.push(GuestDirectoryAccess {
            alias: directory.alias.clone(),
            access: AccessMode::ReadOnly,
        });
    }
    let authority = match AgentAuthority::new(step.tools.clone(), directories) {
        Ok(authority) => authority,
        Err(error) => {
            errors.action = error.message();
            return None;
        }
    };
    let mut outputs = Vec::new();
    for (index, output) in step.outputs.iter().enumerate() {
        let key = match OutputKey::parse(&output.key) {
            Ok(key) => key,
            Err(error) => {
                errors.outputs[index].key = error.message();
                return None;
            }
        };
        let Some(kind) = OutputKind::parse(&output.kind) else {
            errors.outputs[index].kind =
                crate::workflows::definition::DefinitionError::Format.message();
            return None;
        };
        outputs.push(RequiredOutput { key, kind });
    }
    Some(StepAction::Agent(AgentStep {
        environment: parse_step_environment(&step.environment, errors)?,
        role,
        directory_access,
        authority,
        required_outputs: outputs,
        settings: step_settings(step, errors)?,
    }))
}

fn build_gate_action(step: &StepDraft, errors: &mut StepErrors) -> Option<StepAction> {
    if step.outputs.len() != 1 {
        errors.action = crate::workflows::definition::DefinitionError::HumanGate.message();
        return None;
    }
    let output = &step.outputs[0];
    let key = match OutputKey::parse(&output.key) {
        Ok(key) => key,
        Err(error) => {
            errors.outputs[0].key = error.message();
            return None;
        }
    };
    let kind = OutputKind::parse(&output.kind);
    if !matches!(kind, Some(OutputKind::PlanDecision)) {
        errors.outputs[0].kind = crate::workflows::definition::DefinitionError::HumanGate.message();
        return None;
    }
    let revision = if let Some(policy) = &step.human_revision {
        let revision_target = match StepKey::parse(&policy.revision_target) {
            Ok(target) => target,
            Err(error) => {
                errors.human_revision_target = error.message();
                return None;
            }
        };
        let attempt_limit = match policy.attempt_limit.parse::<u8>() {
            Ok(limit)
                if (crate::workflows::definition::MINIMUM_REVIEW_ATTEMPTS
                    ..=crate::workflows::definition::MAXIMUM_REVIEW_ATTEMPTS)
                    .contains(&limit) =>
            {
                limit
            }
            _ => {
                errors.human_attempt_limit =
                    crate::workflows::definition::DefinitionError::AttemptLimit.message();
                return None;
            }
        };
        Some(HumanRevisionPolicy {
            revision_target,
            attempt_limit,
        })
    } else {
        None
    };
    Some(StepAction::HumanGate(HumanGateStep {
        required_output: RequiredOutput {
            key,
            kind: kind.expect("validated gate output"),
        },
        revision,
    }))
}

fn build_command_action(step: &StepDraft, errors: &mut StepErrors) -> Option<StepAction> {
    let Some(command) = SystemCommandId::parse(&step.command) else {
        errors.command = crate::workflows::definition::DefinitionError::Command.message();
        return None;
    };
    let mut outputs = Vec::new();
    for (index, output) in step.outputs.iter().enumerate() {
        let key = match OutputKey::parse(&output.key) {
            Ok(key) => key,
            Err(error) => {
                errors.outputs[index].key = error.message();
                return None;
            }
        };
        let Some(kind) = OutputKind::parse(&output.kind) else {
            errors.outputs[index].kind =
                crate::workflows::definition::DefinitionError::Format.message();
            return None;
        };
        outputs.push(RequiredOutput { key, kind });
    }
    let contract = command.contract();
    let output_kinds: Vec<_> = outputs.iter().map(|output| output.kind).collect();
    if !crate::workflows::commands::kinds_match(&output_kinds, contract.required_outputs) {
        if let Some(index) = (0..outputs.len()).find(|index| {
            contract
                .required_outputs
                .get(*index)
                .is_none_or(|kind| outputs[*index].kind != *kind)
        }) {
            errors.outputs[index].kind =
                crate::workflows::definition::DefinitionError::UnsupportedOutput.message();
        } else {
            errors.command =
                crate::workflows::definition::DefinitionError::UnsupportedOutput.message();
        }
        return None;
    }
    Some(StepAction::SystemCommand(SystemCommandStep {
        environment: parse_step_environment(&step.environment, errors)?,
        command,
        required_outputs: outputs,
    }))
}

fn step_environment_token(environment: StepEnvironment) -> String {
    match environment {
        StepEnvironment::WorkflowDefault => String::new(),
        StepEnvironment::Override { environment_id } => environment_id.as_hex(),
    }
}

fn parse_step_environment(raw: &str, errors: &mut StepErrors) -> Option<StepEnvironment> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Some(StepEnvironment::WorkflowDefault);
    }
    match crate::environments::EnvironmentId::parse(raw) {
        Some(environment_id) => Some(StepEnvironment::Override { environment_id }),
        None => {
            errors.environment =
                crate::workflows::definition::DefinitionError::Environment.message();
            None
        }
    }
}

fn relate_definition_error(
    state: &WorkflowFormState,
    error: crate::workflows::definition::DefinitionError,
    errors: &mut FormErrors,
) {
    use crate::workflows::definition::DefinitionError;
    errors.summary = error.message();
    match error {
        DefinitionError::Name => errors.name = error.message(),

        DefinitionError::DuplicateRole => {
            for role in &mut errors.roles {
                if role.key.is_empty() {
                    role.key = error.message();
                }
            }
        }
        DefinitionError::DuplicateStep => {
            for step in &mut errors.steps {
                if step.key.is_empty() {
                    step.key = error.message();
                }
            }
        }
        DefinitionError::UnknownRole => {
            for (index, step) in state.steps.iter().enumerate() {
                if step.action == "agent" && !state.roles.iter().any(|role| role.key == step.role) {
                    errors.steps[index].role = error.message();
                }
            }
        }

        DefinitionError::PlanDecisionInput => {
            for (index, step) in state.steps.iter().enumerate() {
                for (input_index, input) in step.inputs.iter().enumerate() {
                    if input.kind == ArtefactKind::PlanDecision.as_str() {
                        errors.steps[index].inputs[input_index].kind = error.message();
                    }
                }
            }
        }
        DefinitionError::UnknownStep | DefinitionError::ReviewPolicy => {
            for (index, step) in state.steps.iter().enumerate() {
                if step.review_policy.is_some() {
                    errors.steps[index].revision_target = error.message();
                    errors.steps[index].review_policy = error.message();
                }
                if step.human_revision.is_some() {
                    errors.steps[index].human_revision_target = error.message();
                }
            }
        }
        DefinitionError::UnsupportedOutput => {
            for (index, step) in state.steps.iter().enumerate() {
                if step.action != "system-command" {
                    continue;
                }
                let Some(command) = SystemCommandId::parse(&step.command) else {
                    continue;
                };
                let contract = command.contract();
                let input_kinds: Vec<_> = step
                    .inputs
                    .iter()
                    .filter_map(|input| ArtefactKind::parse(&input.kind))
                    .collect();
                let output_kinds: Vec<_> = step
                    .outputs
                    .iter()
                    .filter_map(|output| OutputKind::parse(&output.kind))
                    .collect();
                if !contract.accepts(&input_kinds, &output_kinds) {
                    mark_command_input_errors(
                        &mut errors.steps[index],
                        step,
                        contract.required_inputs,
                    );
                }
                if !crate::workflows::commands::kinds_match(
                    &output_kinds,
                    contract.required_outputs,
                ) {
                    mark_command_output_errors(
                        &mut errors.steps[index],
                        step,
                        contract.required_outputs,
                    );
                }
            }
        }
        _ => {}
    }
}

fn mark_command_input_errors(errors: &mut StepErrors, step: &StepDraft, required: &[ArtefactKind]) {
    for (index, input) in step.inputs.iter().enumerate() {
        if ArtefactKind::parse(&input.kind).is_some_and(|kind| !required.contains(&kind)) {
            errors.inputs[index].kind =
                crate::workflows::definition::DefinitionError::InputKind.message();
        }
    }
}

fn mark_command_output_errors(errors: &mut StepErrors, step: &StepDraft, required: &[OutputKind]) {
    for (index, output) in step.outputs.iter().enumerate() {
        if OutputKind::parse(&output.kind).is_some_and(|kind| !required.contains(&kind)) {
            errors.outputs[index].kind =
                crate::workflows::definition::DefinitionError::UnsupportedOutput.message();
        }
    }
    if step.outputs.is_empty() && !required.is_empty() {
        errors.command = crate::workflows::definition::DefinitionError::UnsupportedOutput.message();
    }
}

fn next_key(prefix: &str, existing: &[&str]) -> String {
    for ordinal in 1..=existing.len() + 1 {
        let key = format!("{prefix}-{ordinal}");
        if !existing.contains(&key.as_str()) {
            return key;
        }
    }
    format!("{prefix}-x")
}

pub(super) fn can_move_step(steps: &[StepDraft], index: usize, up: bool) -> bool {
    let Ok(target) = move_target(steps.len(), index, up) else {
        return false;
    };
    let mut moved = steps.to_vec();
    moved.swap(index, target);
    review_targets_are_earlier(&moved)
}

pub(super) fn can_remove_step(steps: &[StepDraft], index: usize) -> bool {
    if steps.len() <= 1 || index >= steps.len() {
        return false;
    }
    let mut remaining = steps.to_vec();
    remaining.remove(index);
    review_targets_are_earlier(&remaining)
}

fn move_step(steps: &mut [StepDraft], index: usize, up: bool) -> Result<(), FormError> {
    let target = move_target(steps.len(), index, up)?;
    if !can_move_step(steps, index, up) {
        return Err(FormError::ReviewTarget);
    }
    steps.swap(index, target);
    Ok(())
}

fn review_targets_are_earlier(steps: &[StepDraft]) -> bool {
    steps.iter().enumerate().all(|(index, step)| {
        let review_target_ok = step.review_policy.as_ref().is_none_or(|policy| {
            steps[..index]
                .iter()
                .any(|candidate| candidate.key == policy.revision_target)
        });
        let human_target_ok = step.human_revision.as_ref().is_none_or(|policy| {
            steps[..index]
                .iter()
                .any(|candidate| candidate.key == policy.revision_target)
        });
        review_target_ok && human_target_ok
    }) && phase_connections_are_valid(steps)
}

fn phase_connections_are_valid(steps: &[StepDraft]) -> bool {
    steps.iter().enumerate().all(|(index, step)| {
        step.inputs.iter().all(|input| {
            ArtefactKind::parse(&input.kind).is_some_and(|kind| {
                source_is_valid(&input.source, kind, &steps[..index])
                    || (kind == ArtefactKind::ReviewReport
                        && steps.iter().any(|source| {
                            source.review_policy.as_ref().is_some_and(|policy| {
                                policy.revision_target == step.key
                                    && input.source
                                        == format!(
                                            "step-output:{}:{}",
                                            source.key, policy.report_output
                                        )
                            }) && source_is_valid(&input.source, kind, std::slice::from_ref(source))
                        }))
            })
        })
    })
}

fn move_item<T>(items: &mut [T], index: usize, up: bool) -> Result<(), FormError> {
    let target = move_target(items.len(), index, up)?;
    items.swap(index, target);
    Ok(())
}

fn move_target(len: usize, index: usize, up: bool) -> Result<usize, FormError> {
    if index >= len {
        return Err(FormError::Index);
    }
    if up {
        return index.checked_sub(1).ok_or(FormError::Index);
    }
    let target = index.checked_add(1).ok_or(FormError::Index)?;
    (target < len).then_some(target).ok_or(FormError::Index)
}

pub(super) fn parse_delete(pairs: &[(String, String)]) -> Result<(u64, bool), FormError> {
    let mut revision = None;
    let mut confirm = false;
    let mut seen = Vec::new();
    for (key, value) in pairs {
        if seen.contains(&key) {
            return Err(FormError::DuplicateField);
        }
        seen.push(key);
        match key.as_str() {
            "revision" => revision = Some(parse_revision(value)?),
            "confirm" => confirm = is_checked(value),
            _ => return Err(FormError::UnknownField),
        }
    }
    Ok((revision.ok_or(FormError::Revision)?, confirm))
}

#[cfg(test)]
mod tests;
