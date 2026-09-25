use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::agents::{AccessMode, ToolId};
use crate::environments::EnvironmentId;
use crate::execution::ExecutionSettings;
use crate::hex;

use super::id::WorkflowId;

pub(crate) use super::commands::SystemCommandId;

pub(crate) const DEFINITION_FORMAT_VERSION: u32 = 1;
pub(crate) const MINIMUM_REVIEW_ATTEMPTS: u8 = 1;
pub(crate) const MAXIMUM_REVIEW_ATTEMPTS: u8 = 8;
pub(crate) const MAXIMUM_RUN_ATTEMPTS: usize = 128;
pub(crate) const MAXIMUM_NAME_BYTES: usize = 80;
pub(crate) const MAXIMUM_EXPERTISE_BYTES: usize = 4_096;
pub(crate) const MAXIMUM_PROMPT_DEFAULTS_BYTES: usize = 32_768;
pub(crate) const MAXIMUM_KEY_BYTES: usize = 32;
pub(crate) const MAXIMUM_ROLES: usize = 16;
pub(crate) const MAXIMUM_STEPS: usize = 32;
pub(crate) const MAXIMUM_OUTPUTS: usize = 8;
pub(crate) const MAXIMUM_INPUTS: usize = 8;
pub(crate) const MAXIMUM_DIRECTORIES: usize = 8;
pub(crate) const ASSISTANT_REPLY: &str = "assistant-reply";
pub(crate) const PRIMARY_SOURCE_ALIAS: &str = "project";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkflowDefinition {
    format_version: u32,
    name: String,
    default_environment: EnvironmentId,
    roles: Vec<RoleDefinition>,
    steps: Vec<StepDefinition>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RoleDefinition {
    pub(crate) key: RoleKey,
    pub(crate) name: String,
    pub(crate) expertise: String,
    pub(crate) prompt_defaults: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StepDefinition {
    pub(crate) key: StepKey,
    pub(crate) name: String,
    pub(crate) inputs: Vec<RequiredInput>,
    pub(crate) action: StepAction,
    pub(crate) review: Option<ReviewPolicy>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StepAction {
    Agent(AgentStep),
    SystemCommand(SystemCommandStep),
    HumanGate(HumanGateStep),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HumanGateStep {
    pub(crate) required_output: RequiredOutput,
    pub(crate) revision: Option<HumanRevisionPolicy>,
}

impl HumanGateStep {
    pub(crate) fn is_plan_checkpoint(&self) -> bool {
        self.required_output.kind == OutputKind::PlanDecision
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HumanRevisionPolicy {
    pub(crate) revision_target: StepKey,
    pub(crate) attempt_limit: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AgentStep {
    pub(crate) role: RoleKey,
    pub(crate) environment: StepEnvironment,
    pub(crate) directory_access: StepAccess,
    pub(crate) authority: AgentAuthority,
    pub(crate) required_outputs: Vec<RequiredOutput>,
    pub(crate) settings: ModelStepSettings,
}

impl AgentStep {
    pub(crate) fn host_tools(&self) -> bool {
        matches!(
            &self.settings,
            ModelStepSettings::Override(settings)
                if settings.location == Some(crate::execution::ToolLocation::Host)
        )
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum ModelStepSettings {
    #[default]
    SameAsRunDefaults,
    Override(Box<crate::execution::SettingsOverrides>),
}

impl ModelStepSettings {
    pub(crate) fn resolve(&self, defaults: &ExecutionSettings) -> ExecutionSettings {
        match self {
            Self::SameAsRunDefaults => defaults.clone(),
            Self::Override(settings) => settings.resolve(defaults),
        }
    }

    pub(crate) fn is_same_as_defaults(&self) -> bool {
        matches!(self, Self::SameAsRunDefaults)
    }
}

pub(crate) fn additional_access(
    defaults: &ExecutionSettings,
    resolved: &ExecutionSettings,
) -> bool {
    resolved
        .tools
        .iter()
        .any(|tool| !defaults.tools.contains(tool))
        || resolved.network != defaults.network
        || resolved.location != defaults.location
        || resolved.host_approval != defaults.host_approval
        || resolved.directories.iter().any(|grant| {
            defaults.directories.iter().all(|existing| {
                existing.identity != grant.identity
                    || (grant.access != crate::execution::DirectoryAccess::Read
                        && existing.access != grant.access)
            })
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StepAccess {
    Read,
    Write,
}

impl StepAccess {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }

    pub(crate) fn access(self) -> AccessMode {
        match self {
            Self::Read => AccessMode::ReadOnly,
            Self::Write => AccessMode::ReadWrite,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Read => "Read",
            Self::Write => "Write",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SystemCommandStep {
    pub(crate) command: SystemCommandId,
    pub(crate) environment: StepEnvironment,
    pub(crate) required_outputs: Vec<RequiredOutput>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StepEnvironment {
    WorkflowDefault,
    Override { environment_id: EnvironmentId },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AgentAuthority {
    pub(crate) tools: Vec<ToolId>,
    pub(crate) directories: Vec<GuestDirectoryAccess>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GuestDirectoryAccess {
    pub(crate) alias: String,
    pub(crate) access: AccessMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RequiredInput {
    pub(crate) key: InputKey,
    pub(crate) kind: ArtefactKind,
    pub(crate) source: ArtefactSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ArtefactSource {
    RunCurrentPlan,
    StepOutput { step: StepKey, output: OutputKey },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RequiredOutput {
    pub(crate) key: OutputKey,
    pub(crate) kind: OutputKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OutputKind {
    AssistantReply,
    Plan,
    ReviewReport,
    TestReport,
    PlanDecision,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ArtefactKind {
    Plan,
    ReviewReport,
    TestReport,
    PlanDecision,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewPolicy {
    pub(crate) report_output: OutputKey,
    pub(crate) revision_target: StepKey,
    pub(crate) attempt_limit: u8,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct RoleKey(String);

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct StepKey(String);

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct OutputKey(String);

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct InputKey(String);

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct DefinitionVersion([u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PinnedWorkflowDefinition {
    pub(crate) workflow_id: Option<WorkflowId>,
    pub(crate) version: DefinitionVersion,
    pub(crate) definition: WorkflowDefinition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DefinitionError {
    Format,
    Environment,
    Name,
    Expertise,
    PromptDefaults,
    Key,
    RoleCount,
    StepCount,
    OutputCount,
    DirectoryCount,
    DuplicateRole,
    DuplicateStep,
    DuplicateOutput,
    DuplicateInput,
    InputCount,
    UnsupportedOutput,
    UnknownOutput,
    InputKind,
    PlanDecisionInput,
    UnusedRole,
    UnknownRole,
    UnknownStep,
    Command,
    Tools,
    Authority,
    Alias,
    DuplicateAlias,
    HumanGate,
    ReviewPolicy,
    AttemptLimit,

    RunBound,
}

impl DefinitionError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Format => "That workflow definition uses an unsupported format.",
            Self::Environment => "Enter a valid environment identifier.",
            Self::Authority => {
                "The workflow needs tools or directory access outside these settings."
            }
            Self::Name => "Enter a name of at most 80 bytes.",
            Self::Expertise => "Those expertise notes are too long.",
            Self::PromptDefaults => "Those prompt defaults are too long.",
            Self::Key => "Enter a key that uses letters, numbers, hyphen or underscore.",
            Self::RoleCount => "Add at most 16 roles.",
            Self::StepCount => "Add between one and 32 steps.",
            Self::OutputCount => "Add at most eight required outputs for each step.",
            Self::DirectoryCount => "Add at most eight secondary directory grants.",
            Self::DuplicateRole => "Role keys must be unique.",
            Self::DuplicateStep => "Step keys must be unique.",
            Self::DuplicateOutput => "Output keys must be unique in a step.",
            Self::DuplicateInput => "Input keys must be unique in a step.",
            Self::InputCount => "Add at most eight inputs for each step.",
            Self::UnsupportedOutput => "That action cannot produce the required output.",
            Self::UnknownOutput => "An input names an unknown earlier output.",
            Self::InputKind => "That input kind does not match the named output.",

            Self::PlanDecisionInput => "A plan decision must accompany the exact accepted plan.",
            Self::UnusedRole => "Every role must be used by an agent step.",
            Self::UnknownRole => "An agent step names an unknown role.",
            Self::UnknownStep => "A review policy names an unknown step.",
            Self::Command => "Choose a registered system command.",
            Self::Tools => "Choose tools from the built-in set.",
            Self::Alias => {
                "Enter a directory alias that uses letters, numbers, hyphen or underscore."
            }
            Self::DuplicateAlias => "Directory aliases must be unique.",
            Self::HumanGate => {
                "A plan checkpoint needs one plan input and one plan decision output."
            }
            Self::ReviewPolicy => "Configure a valid review policy.",
            Self::AttemptLimit => "Set the review attempt limit from one through eight.",

            Self::RunBound => "This workflow can create too many attempts or artefacts.",
        }
    }
}

impl std::fmt::Display for DefinitionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for DefinitionError {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct DefinitionFile {
    format_version: u32,
    name: String,
    default_environment: String,
    roles: Vec<RoleFile>,
    steps: Vec<StepFile>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct RoleFile {
    key: String,
    name: String,
    expertise: String,
    prompt_defaults: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct StepFile {
    key: String,
    name: String,
    inputs: Vec<InputFile>,
    action: ActionFile,
    #[serde(deserialize_with = "crate::storage::required_option")]
    review: Option<ReviewPolicyFile>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum ActionFile {
    Agent {
        role: String,
        environment: StepEnvironmentFile,
        #[serde(rename = "directory-access")]
        directory_access: String,
        authority: AuthorityFile,
        #[serde(rename = "required-outputs")]
        required_outputs: Vec<OutputFile>,
        #[serde(default)]
        settings: ModelStepSettingsFile,
    },
    SystemCommand {
        command: String,
        environment: StepEnvironmentFile,
        #[serde(rename = "required-outputs")]
        required_outputs: Vec<OutputFile>,
    },
    HumanGate {
        #[serde(rename = "required-output")]
        required_output: OutputFile,
        #[serde(default, rename = "revision-target")]
        revision_target: Option<String>,
        #[serde(default, rename = "attempt-limit")]
        attempt_limit: Option<u8>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "source", rename_all = "kebab-case")]
enum StepEnvironmentFile {
    WorkflowDefault,
    Override { environment_id: String },
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(tag = "source", rename_all = "kebab-case")]
enum ModelStepSettingsFile {
    #[default]
    SameAsRunDefaults,
    Override {
        settings: Box<crate::execution::SettingsOverridesFile>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct AuthorityFile {
    tools: Vec<String>,
    directories: Vec<DirectoryFile>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct DirectoryFile {
    alias: String,
    access: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct OutputFile {
    key: String,
    kind: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct InputFile {
    key: String,
    kind: String,
    source: InputSourceFile,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "source", rename_all = "kebab-case")]
enum InputSourceFile {
    RunCurrentPlan,
    StepOutput { step: String, output: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ReviewPolicyFile {
    report_output: String,
    revision_target: String,
    attempt_limit: u8,
}

impl WorkflowDefinition {
    pub(crate) fn from_file(file: DefinitionFile) -> Result<Self, DefinitionError> {
        if file.format_version != DEFINITION_FORMAT_VERSION {
            return Err(DefinitionError::Format);
        }
        from_current_file(file)
    }

    pub(crate) fn from_parts(
        name: String,
        default_environment: EnvironmentId,
        roles: Vec<RoleDefinition>,
        steps: Vec<StepDefinition>,
    ) -> Result<Self, DefinitionError> {
        assemble(name, default_environment, roles, steps)
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn default_environment(&self) -> EnvironmentId {
        self.default_environment
    }

    pub(crate) fn with_conversation_settings(
        &self,
        settings: &ExecutionSettings,
    ) -> Result<Self, DefinitionError> {
        let phases: Vec<_> = self
            .steps
            .iter()
            .filter_map(|step| match &step.action {
                StepAction::Agent(action) => {
                    Some((step.key.clone(), action.settings.resolve(settings)))
                }
                StepAction::SystemCommand(_) | StepAction::HumanGate(_) => None,
            })
            .collect();
        self.with_phase_settings(settings, &phases)
    }

    pub(crate) fn with_phase_settings(
        &self,
        defaults: &ExecutionSettings,
        phases: &[(StepKey, ExecutionSettings)],
    ) -> Result<Self, DefinitionError> {
        let mut steps = self.steps.clone();
        for (key, settings) in phases {
            if !matches!(
                self.step(key).map(|step| &step.action),
                Some(StepAction::Agent(_))
            ) || !settings.host_access_allowed()
            {
                return Err(DefinitionError::Authority);
            }
        }
        for step in &mut steps {
            let StepAction::Agent(action) = &mut step.action else {
                continue;
            };
            let effective = phases
                .iter()
                .find(|(key, _)| key == &step.key)
                .map(|(_, settings)| settings)
                .unwrap_or(defaults);
            if !effective.host_access_allowed()
                || (effective.location == crate::execution::ToolLocation::Host
                    && action.directory_access == StepAccess::Read)
            {
                return Err(DefinitionError::Authority);
            }
            action.authority = AgentAuthority::new(
                effective.tools.clone(),
                effective
                    .directories
                    .iter()
                    .map(|grant| GuestDirectoryAccess {
                        alias: grant.alias.clone(),
                        access: if grant.access == crate::execution::DirectoryAccess::Write
                            && action.directory_access == StepAccess::Write
                        {
                            AccessMode::ReadWrite
                        } else {
                            AccessMode::ReadOnly
                        },
                    })
                    .collect(),
            )?;
            action.settings = ModelStepSettings::Override(Box::new(
                crate::execution::SettingsOverrides::all(effective.clone()),
            ));
            action.environment = if effective.environment == defaults.environment {
                StepEnvironment::WorkflowDefault
            } else {
                StepEnvironment::Override {
                    environment_id: effective.environment,
                }
            };
        }
        Self::from_parts(
            self.name.clone(),
            defaults.environment,
            self.roles.clone(),
            steps,
        )
    }

    pub(crate) fn directory_grants(
        &self,
    ) -> impl Iterator<Item = &crate::execution::DirectoryGrant> {
        self.steps
            .iter()
            .filter_map(|step| match &step.action {
                StepAction::Agent(action) => match &action.settings {
                    ModelStepSettings::Override(settings) => settings.directories.as_deref(),
                    ModelStepSettings::SameAsRunDefaults => None,
                },
                StepAction::SystemCommand(_) | StepAction::HumanGate(_) => None,
            })
            .flatten()
    }

    pub(crate) fn referenced_environments(&self) -> Vec<EnvironmentId> {
        let mut ids = vec![self.default_environment];
        for step in &self.steps {
            if let StepAction::Agent(action) = &step.action
                && let ModelStepSettings::Override(settings) = &action.settings
                && let Some(environment) = settings.environment
                && !ids.contains(&environment)
            {
                ids.push(environment);
            }
            if let Some(StepEnvironment::Override { environment_id }) = step.environment()
                && !ids.contains(&environment_id)
            {
                ids.push(environment_id);
            }
        }
        ids
    }

    pub(crate) fn effective_environment(&self, step: &StepDefinition) -> EnvironmentId {
        if let StepAction::Agent(action) = &step.action
            && let ModelStepSettings::Override(settings) = &action.settings
            && let Some(environment) = settings.environment
        {
            return environment;
        }
        match step.environment().expect("sandbox-backed step") {
            StepEnvironment::WorkflowDefault => self.default_environment,
            StepEnvironment::Override { environment_id } => environment_id,
        }
    }

    pub(crate) fn roles(&self) -> &[RoleDefinition] {
        &self.roles
    }

    pub(crate) fn first_step(&self) -> &StepKey {
        &self.steps[0].key
    }

    pub(crate) fn steps(&self) -> &[StepDefinition] {
        &self.steps
    }

    pub(crate) fn step(&self, key: &StepKey) -> Option<&StepDefinition> {
        self.steps.iter().find(|step| step.key == *key)
    }

    pub(crate) fn step_position(&self, key: &StepKey) -> Option<usize> {
        self.steps.iter().position(|step| step.key == *key)
    }

    pub(crate) fn next_step(&self, key: &StepKey) -> Option<&StepKey> {
        let index = self.step_position(key)?;
        self.steps.get(index + 1).map(|step| &step.key)
    }

    pub(crate) fn review_phase(&self, key: &StepKey) -> Option<u32> {
        let mut phase = 0u32;
        for step in &self.steps {
            if step.review.is_some() {
                phase += 1;
                if &step.key == key {
                    return Some(phase);
                }
            }
        }
        None
    }

    pub(crate) fn attempt_bound(&self) -> usize {
        self.steps.len()
            + self
                .steps
                .iter()
                .enumerate()
                .filter_map(|(end, step)| {
                    let policy = step.review.as_ref()?;
                    let start = self.step_position(&policy.revision_target)?;
                    Some((end - start + 1) * usize::from(policy.attempt_limit - 1))
                })
                .chain(self.steps.iter().enumerate().filter_map(|(end, step)| {
                    let StepAction::HumanGate(action) = &step.action else {
                        return None;
                    };
                    let policy = action.revision.as_ref()?;
                    let start = self.step_position(&policy.revision_target)?;
                    Some((end - start + 1) * usize::from(policy.attempt_limit - 1))
                }))
                .sum::<usize>()
    }

    pub(crate) fn role(&self, key: &RoleKey) -> Option<&RoleDefinition> {
        self.roles.iter().find(|role| role.key == *key)
    }

    pub(crate) fn version(&self) -> DefinitionVersion {
        DefinitionVersion::of(&self.to_file())
    }

    pub(crate) fn to_file(&self) -> DefinitionFile {
        DefinitionFile {
            format_version: self.format_version,
            name: self.name.clone(),
            default_environment: self.default_environment.as_hex(),

            roles: self
                .roles
                .iter()
                .map(|role| RoleFile {
                    key: role.key.as_str().to_owned(),
                    name: role.name.clone(),
                    expertise: role.expertise.clone(),
                    prompt_defaults: role.prompt_defaults.clone(),
                })
                .collect(),
            steps: self.steps.iter().map(StepDefinition::to_file).collect(),
        }
    }
}

impl RoleDefinition {
    pub(crate) fn new(
        key: RoleKey,
        name: String,
        expertise: String,
        prompt_defaults: String,
    ) -> Result<Self, DefinitionError> {
        Ok(Self {
            key,
            name: normalise_name(&name)?,
            expertise: normalise_text(
                &expertise,
                MAXIMUM_EXPERTISE_BYTES,
                DefinitionError::Expertise,
            )?,
            prompt_defaults: normalise_text(
                &prompt_defaults,
                MAXIMUM_PROMPT_DEFAULTS_BYTES,
                DefinitionError::PromptDefaults,
            )?,
        })
    }

    fn from_file(file: RoleFile) -> Result<Self, DefinitionError> {
        Self::new(
            RoleKey::parse(&file.key)?,
            file.name,
            file.expertise,
            file.prompt_defaults,
        )
    }
}

impl StepDefinition {
    fn from_file(file: StepFile) -> Result<Self, DefinitionError> {
        Ok(Self {
            key: StepKey::parse(&file.key)?,
            name: normalise_name(&file.name)?,
            inputs: parse_inputs(file.inputs)?,
            action: StepAction::from_file(file.action)?,
            review: file.review.map(ReviewPolicy::from_file).transpose()?,
        })
    }

    pub(crate) fn environment(&self) -> Option<StepEnvironment> {
        match &self.action {
            StepAction::Agent(action) => Some(action.environment),
            StepAction::SystemCommand(action) => Some(action.environment),
            StepAction::HumanGate(_) => None,
        }
    }

    pub(crate) fn is_sandbox_backed(&self) -> bool {
        match &self.action {
            StepAction::Agent(action) => !action.host_tools(),
            StepAction::SystemCommand(_) => true,
            StepAction::HumanGate(_) => false,
        }
    }

    pub(crate) fn required_outputs(&self) -> &[RequiredOutput] {
        match &self.action {
            StepAction::Agent(action) => &action.required_outputs,
            StepAction::SystemCommand(action) => &action.required_outputs,
            StepAction::HumanGate(action) => std::slice::from_ref(&action.required_output),
        }
    }

    pub(crate) fn is_review_feedback(&self, input: &RequiredInput, steps: &[Self]) -> bool {
        let ArtefactSource::StepOutput { step, output } = &input.source else {
            return false;
        };
        input.kind == ArtefactKind::ReviewReport
            && steps.iter().any(|source| {
                source.key == *step
                    && source.review.as_ref().is_some_and(|policy| {
                        policy.revision_target == self.key && policy.report_output == *output
                    })
                    && source.required_outputs().iter().any(|declared| {
                        declared.key == *output && declared.kind == OutputKind::ReviewReport
                    })
            })
    }

    pub(crate) fn accepts_missing_initial_plan(&self, input: &RequiredInput) -> bool {
        input.kind == ArtefactKind::Plan
            && input.source == ArtefactSource::RunCurrentPlan
            && matches!(&self.action, StepAction::Agent(action) if action.directory_access == StepAccess::Read)
            && self
                .required_outputs()
                .iter()
                .any(|output| output.kind == OutputKind::Plan)
    }

    pub(crate) fn writes_primary_source(&self) -> bool {
        matches!(
            &self.action,
            StepAction::Agent(AgentStep {
                directory_access: StepAccess::Write,
                ..
            })
        )
    }

    fn to_file(&self) -> StepFile {
        StepFile {
            key: self.key.as_str().to_owned(),
            name: self.name.clone(),
            inputs: self
                .inputs
                .iter()
                .map(|input| InputFile {
                    key: input.key.as_str().to_owned(),
                    kind: input.kind.as_str().to_owned(),
                    source: match &input.source {
                        ArtefactSource::RunCurrentPlan => InputSourceFile::RunCurrentPlan,

                        ArtefactSource::StepOutput { step, output } => {
                            InputSourceFile::StepOutput {
                                step: step.as_str().to_owned(),
                                output: output.as_str().to_owned(),
                            }
                        }
                    },
                })
                .collect(),
            action: self.action.to_file(),
            review: self.review.as_ref().map(ReviewPolicy::to_file),
        }
    }
}

impl StepAction {
    fn from_file(file: ActionFile) -> Result<Self, DefinitionError> {
        match file {
            ActionFile::Agent {
                role,
                environment,
                directory_access,
                authority,
                required_outputs,
                settings,
            } => Ok(Self::Agent(AgentStep {
                role: RoleKey::parse(&role)?,
                environment: StepEnvironment::from_file(environment)?,
                directory_access: StepAccess::parse(&directory_access)
                    .ok_or(DefinitionError::Format)?,
                authority: AgentAuthority::from_file(authority)?,
                required_outputs: parse_outputs(required_outputs)?,
                settings: ModelStepSettings::from_file(settings)?,
            })),
            ActionFile::SystemCommand {
                command,
                environment,
                required_outputs,
            } => Ok(Self::SystemCommand(SystemCommandStep {
                command: SystemCommandId::parse(&command).ok_or(DefinitionError::Command)?,
                environment: StepEnvironment::from_file(environment)?,
                required_outputs: parse_outputs(required_outputs)?,
            })),
            ActionFile::HumanGate {
                required_output,
                revision_target,
                attempt_limit,
            } => {
                let mut outputs = parse_outputs(vec![required_output])?;
                let revision = match (revision_target, attempt_limit) {
                    (None, None) => None,
                    (Some(target), Some(limit)) => Some(HumanRevisionPolicy {
                        revision_target: StepKey::parse(&target)?,
                        attempt_limit: limit,
                    }),
                    _ => return Err(DefinitionError::Format),
                };
                Ok(Self::HumanGate(HumanGateStep {
                    required_output: outputs.remove(0),
                    revision,
                }))
            }
        }
    }

    fn to_file(&self) -> ActionFile {
        match self {
            Self::Agent(step) => ActionFile::Agent {
                role: step.role.as_str().to_owned(),
                environment: step.environment.to_file(),
                directory_access: step.directory_access.as_str().to_owned(),
                authority: step.authority.to_file(),
                required_outputs: outputs_to_file(&step.required_outputs),
                settings: step.settings.to_file(),
            },
            Self::SystemCommand(step) => ActionFile::SystemCommand {
                command: step.command.as_str().to_owned(),
                environment: step.environment.to_file(),
                required_outputs: outputs_to_file(&step.required_outputs),
            },
            Self::HumanGate(step) => ActionFile::HumanGate {
                required_output: outputs_to_file(std::slice::from_ref(&step.required_output))
                    .remove(0),
                revision_target: step
                    .revision
                    .as_ref()
                    .map(|revision| revision.revision_target.as_str().to_owned()),
                attempt_limit: step
                    .revision
                    .as_ref()
                    .map(|revision| revision.attempt_limit),
            },
        }
    }

    pub(crate) fn kind_label(&self) -> &'static str {
        match self {
            Self::Agent(_) => "Agent",
            Self::SystemCommand(_) => "System command",
            Self::HumanGate(_) => "Human gate",
        }
    }
}

impl AgentAuthority {
    pub(crate) fn new(
        tools: Vec<ToolId>,
        directories: Vec<GuestDirectoryAccess>,
    ) -> Result<Self, DefinitionError> {
        if tools.len() > ToolId::ALL.len() {
            return Err(DefinitionError::Tools);
        }
        let mut unique_tools = Vec::new();
        for tool in tools {
            if unique_tools.contains(&tool) {
                return Err(DefinitionError::Tools);
            }
            unique_tools.push(tool);
        }
        let tools = ToolId::ALL
            .into_iter()
            .filter(|tool| unique_tools.contains(tool))
            .collect();
        if directories.len() > MAXIMUM_DIRECTORIES {
            return Err(DefinitionError::DirectoryCount);
        }
        let mut unique_dirs = Vec::new();
        for directory in directories {
            let alias = normalise_alias(&directory.alias)?;
            if alias == PRIMARY_SOURCE_ALIAS {
                return Err(DefinitionError::Alias);
            }
            if unique_dirs
                .iter()
                .any(|seen: &GuestDirectoryAccess| seen.alias == alias)
            {
                return Err(DefinitionError::DuplicateAlias);
            }
            unique_dirs.push(GuestDirectoryAccess {
                alias,
                access: directory.access,
            });
        }
        Ok(Self {
            tools,
            directories: unique_dirs,
        })
    }

    pub(crate) fn allowed_by<'a>(
        &self,
        tools: &[ToolId],
        directories: impl IntoIterator<Item = (&'a str, AccessMode)>,
    ) -> bool {
        let grants: Vec<(&str, AccessMode)> = directories.into_iter().collect();
        self.tools.iter().all(|tool| tools.contains(tool))
            && self.directories.iter().all(|directory| {
                grants.iter().any(|(alias, access)| {
                    *alias == directory.alias
                        && (!directory.access.is_writable() || access.is_writable())
                })
            })
    }

    fn from_file(file: AuthorityFile) -> Result<Self, DefinitionError> {
        let mut tools = Vec::new();
        for name in file.tools {
            let tool = ToolId::parse(&name).ok_or(DefinitionError::Tools)?;
            tools.push(tool);
        }
        let directories = file
            .directories
            .into_iter()
            .map(|directory| {
                let access = AccessMode::parse(&directory.access).ok_or(DefinitionError::Format)?;
                Ok(GuestDirectoryAccess {
                    alias: directory.alias,
                    access,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(tools, directories)
    }

    fn to_file(&self) -> AuthorityFile {
        AuthorityFile {
            tools: self
                .tools
                .iter()
                .map(|tool| tool.as_str().to_owned())
                .collect(),
            directories: self
                .directories
                .iter()
                .map(|directory| DirectoryFile {
                    alias: directory.alias.clone(),
                    access: directory.access.as_str().to_owned(),
                })
                .collect(),
        }
    }
}

impl ModelStepSettings {
    fn from_file(file: ModelStepSettingsFile) -> Result<Self, DefinitionError> {
        match file {
            ModelStepSettingsFile::SameAsRunDefaults => Ok(Self::SameAsRunDefaults),
            ModelStepSettingsFile::Override { settings } => {
                let settings = crate::execution::SettingsOverrides::from_file(*settings)
                    .ok_or(DefinitionError::Format)?;
                Ok(Self::Override(Box::new(settings)))
            }
        }
    }

    fn to_file(&self) -> ModelStepSettingsFile {
        match self {
            Self::SameAsRunDefaults => ModelStepSettingsFile::SameAsRunDefaults,
            Self::Override(settings) => ModelStepSettingsFile::Override {
                settings: Box::new(settings.to_file()),
            },
        }
    }
}

impl StepEnvironment {
    fn from_file(file: StepEnvironmentFile) -> Result<Self, DefinitionError> {
        match file {
            StepEnvironmentFile::WorkflowDefault => Ok(Self::WorkflowDefault),
            StepEnvironmentFile::Override { environment_id } => {
                let environment_id =
                    EnvironmentId::parse(&environment_id).ok_or(DefinitionError::Environment)?;
                Ok(Self::Override { environment_id })
            }
        }
    }

    fn to_file(self) -> StepEnvironmentFile {
        match self {
            Self::WorkflowDefault => StepEnvironmentFile::WorkflowDefault,
            Self::Override { environment_id } => StepEnvironmentFile::Override {
                environment_id: environment_id.as_hex(),
            },
        }
    }

    fn normalised(self, default: EnvironmentId) -> Self {
        match self {
            Self::Override { environment_id } if environment_id == default => Self::WorkflowDefault,
            other => other,
        }
    }
}

impl ReviewPolicy {
    fn from_file(file: ReviewPolicyFile) -> Result<Self, DefinitionError> {
        Ok(Self {
            report_output: OutputKey::parse(&file.report_output)?,
            revision_target: StepKey::parse(&file.revision_target)?,
            attempt_limit: file.attempt_limit,
        })
    }

    fn to_file(&self) -> ReviewPolicyFile {
        ReviewPolicyFile {
            report_output: self.report_output.as_str().to_owned(),
            revision_target: self.revision_target.as_str().to_owned(),
            attempt_limit: self.attempt_limit,
        }
    }
}

impl OutputKind {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "assistant-reply" => Some(Self::AssistantReply),
            "plan" => Some(Self::Plan),
            "review-report" => Some(Self::ReviewReport),
            "test-report" => Some(Self::TestReport),
            "plan-decision" => Some(Self::PlanDecision),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::AssistantReply => "assistant-reply",
            Self::Plan => "plan",
            Self::ReviewReport => "review-report",
            Self::TestReport => "test-report",
            Self::PlanDecision => "plan-decision",
        }
    }

    pub(crate) fn as_artefact_kind(self) -> Option<ArtefactKind> {
        match self {
            Self::AssistantReply => None,
            Self::Plan => Some(ArtefactKind::Plan),
            Self::ReviewReport => Some(ArtefactKind::ReviewReport),
            Self::TestReport => Some(ArtefactKind::TestReport),
            Self::PlanDecision => Some(ArtefactKind::PlanDecision),
        }
    }
}

impl ArtefactKind {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "plan" => Some(Self::Plan),
            "review-report" => Some(Self::ReviewReport),
            "test-report" => Some(Self::TestReport),
            "plan-decision" => Some(Self::PlanDecision),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::ReviewReport => "review-report",
            Self::TestReport => "test-report",
            Self::PlanDecision => "plan-decision",
        }
    }
}

impl RoleKey {
    pub(crate) fn parse(value: &str) -> Result<Self, DefinitionError> {
        Ok(Self(parse_key(value)?))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl StepKey {
    pub(crate) fn parse(value: &str) -> Result<Self, DefinitionError> {
        Ok(Self(parse_key(value)?))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl OutputKey {
    pub(crate) fn parse(value: &str) -> Result<Self, DefinitionError> {
        Ok(Self(parse_key(value)?))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl InputKey {
    pub(crate) fn parse(value: &str) -> Result<Self, DefinitionError> {
        Ok(Self(parse_key(value)?))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl DefinitionVersion {
    fn of(file: &DefinitionFile) -> Self {
        let bytes = serde_json::to_vec(file).expect("canonical definition json");
        Self(Sha256::digest(bytes).into())
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        hex::decode(value).map(Self)
    }

    pub(crate) fn as_hex(&self) -> String {
        hex::encode(&self.0)
    }
}

impl std::fmt::Debug for DefinitionVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("DefinitionVersion(")?;
        formatter.write_str(&self.as_hex())?;
        formatter.write_str(")")
    }
}

impl PinnedWorkflowDefinition {
    pub(crate) fn pin(workflow_id: Option<WorkflowId>, definition: WorkflowDefinition) -> Self {
        let version = definition.version();
        Self {
            workflow_id,
            version,
            definition,
        }
    }
}

fn assemble(
    name: String,
    default_environment: EnvironmentId,
    roles: Vec<RoleDefinition>,
    mut steps: Vec<StepDefinition>,
) -> Result<WorkflowDefinition, DefinitionError> {
    let name = normalise_name(&name)?;
    if roles.len() > MAXIMUM_ROLES {
        return Err(DefinitionError::RoleCount);
    }
    if steps.is_empty() || steps.len() > MAXIMUM_STEPS {
        return Err(DefinitionError::StepCount);
    }
    for step in &mut steps {
        match &mut step.action {
            StepAction::Agent(action) => {
                action.environment = action.environment.normalised(default_environment);
            }
            StepAction::SystemCommand(action) => {
                action.environment = action.environment.normalised(default_environment);
            }
            StepAction::HumanGate(_) => {}
        }
    }
    reject_duplicate_roles(&roles)?;
    reject_duplicate_steps(&steps)?;
    reject_step_outputs(&steps)?;
    reject_step_inputs(&steps)?;
    reject_unsupported_outputs(&steps)?;
    reject_role_use(&roles, &steps)?;
    reject_review_policies(&steps)?;
    reject_human_revisions(&steps)?;
    reject_handoff(&steps)?;
    reject_plan_decision_inputs(&steps)?;

    Ok(WorkflowDefinition {
        format_version: DEFINITION_FORMAT_VERSION,
        name,
        default_environment,
        roles,
        steps,
    })
}

fn from_current_file(file: DefinitionFile) -> Result<WorkflowDefinition, DefinitionError> {
    let (name, default_environment, roles, steps) = parse_file_parts(file)?;
    assemble(name, default_environment, roles, steps)
}

type FileParts = (
    String,
    EnvironmentId,
    Vec<RoleDefinition>,
    Vec<StepDefinition>,
);

fn parse_file_parts(file: DefinitionFile) -> Result<FileParts, DefinitionError> {
    let name = normalise_name(&file.name)?;
    if file.roles.len() > MAXIMUM_ROLES {
        return Err(DefinitionError::RoleCount);
    }
    if file.steps.is_empty() || file.steps.len() > MAXIMUM_STEPS {
        return Err(DefinitionError::StepCount);
    }
    let default_environment =
        EnvironmentId::parse(&file.default_environment).ok_or(DefinitionError::Environment)?;
    let roles = file
        .roles
        .into_iter()
        .map(RoleDefinition::from_file)
        .collect::<Result<Vec<_>, _>>()?;
    let steps = file
        .steps
        .into_iter()
        .map(StepDefinition::from_file)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((name, default_environment, roles, steps))
}

fn parse_inputs(files: Vec<InputFile>) -> Result<Vec<RequiredInput>, DefinitionError> {
    if files.len() > MAXIMUM_INPUTS {
        return Err(DefinitionError::InputCount);
    }
    let mut inputs = Vec::with_capacity(files.len());
    for file in files {
        let key = InputKey::parse(&file.key)?;
        if inputs.iter().any(|item: &RequiredInput| item.key == key) {
            return Err(DefinitionError::DuplicateInput);
        }
        let kind = ArtefactKind::parse(&file.kind).ok_or(DefinitionError::Format)?;
        let source = match file.source {
            InputSourceFile::RunCurrentPlan => ArtefactSource::RunCurrentPlan,

            InputSourceFile::StepOutput { step, output } => ArtefactSource::StepOutput {
                step: StepKey::parse(&step)?,
                output: OutputKey::parse(&output)?,
            },
        };
        inputs.push(RequiredInput { key, kind, source });
    }
    Ok(inputs)
}

fn parse_outputs(files: Vec<OutputFile>) -> Result<Vec<RequiredOutput>, DefinitionError> {
    if files.len() > MAXIMUM_OUTPUTS {
        return Err(DefinitionError::OutputCount);
    }
    let mut outputs = Vec::with_capacity(files.len());
    for file in files {
        let key = OutputKey::parse(&file.key)?;
        if outputs.iter().any(|item: &RequiredOutput| item.key == key) {
            return Err(DefinitionError::DuplicateOutput);
        }
        let kind = OutputKind::parse(&file.kind).ok_or(DefinitionError::Format)?;
        outputs.push(RequiredOutput { key, kind });
    }
    Ok(outputs)
}

fn outputs_to_file(outputs: &[RequiredOutput]) -> Vec<OutputFile> {
    outputs
        .iter()
        .map(|output| OutputFile {
            key: output.key.as_str().to_owned(),
            kind: output.kind.as_str().to_owned(),
        })
        .collect()
}

fn reject_duplicate_roles(roles: &[RoleDefinition]) -> Result<(), DefinitionError> {
    for (index, role) in roles.iter().enumerate() {
        if roles
            .iter()
            .skip(index + 1)
            .any(|other| other.key == role.key)
        {
            return Err(DefinitionError::DuplicateRole);
        }
    }
    Ok(())
}

fn reject_duplicate_steps(steps: &[StepDefinition]) -> Result<(), DefinitionError> {
    for (index, step) in steps.iter().enumerate() {
        if steps
            .iter()
            .skip(index + 1)
            .any(|other| other.key == step.key)
        {
            return Err(DefinitionError::DuplicateStep);
        }
    }
    Ok(())
}

fn reject_unsupported_outputs(steps: &[StepDefinition]) -> Result<(), DefinitionError> {
    for step in steps {
        match &step.action {
            StepAction::Agent(action) => {
                if action
                    .required_outputs
                    .iter()
                    .any(|output| output.kind == OutputKind::PlanDecision)
                {
                    return Err(DefinitionError::UnsupportedOutput);
                }
            }
            StepAction::SystemCommand(action) => {
                if action.command != SystemCommandId::RepositoryStatus
                    || !action.required_outputs.is_empty()
                {
                    return Err(DefinitionError::UnsupportedOutput);
                }
            }
            StepAction::HumanGate(action) => {
                if !action.is_plan_checkpoint()
                    || step
                        .inputs
                        .iter()
                        .filter(|input| input.kind == ArtefactKind::Plan)
                        .count()
                        != 1
                {
                    return Err(DefinitionError::HumanGate);
                }
            }
        }
    }
    Ok(())
}

fn reject_step_inputs(steps: &[StepDefinition]) -> Result<(), DefinitionError> {
    for step in steps {
        if step.inputs.len() > MAXIMUM_INPUTS {
            return Err(DefinitionError::InputCount);
        }
        for (index, input) in step.inputs.iter().enumerate() {
            if step
                .inputs
                .iter()
                .skip(index + 1)
                .any(|other| other.key == input.key)
            {
                return Err(DefinitionError::DuplicateInput);
            }
        }
    }
    Ok(())
}

fn reject_handoff(steps: &[StepDefinition]) -> Result<(), DefinitionError> {
    let mut produced = Vec::new();
    for step in steps {
        for input in &step.inputs {
            if step.is_review_feedback(input, steps) {
                continue;
            }
            match &input.source {
                ArtefactSource::RunCurrentPlan if input.kind == ArtefactKind::Plan => {}
                ArtefactSource::StepOutput { step, output }
                    if produced.iter().any(|(key, name, kind)| {
                        key == step && name == output && *kind == input.kind
                    }) => {}
                _ => return Err(DefinitionError::UnknownOutput),
            }
        }
        for output in step.required_outputs() {
            if let Some(kind) = output.kind.as_artefact_kind() {
                produced.push((step.key.clone(), output.key.clone(), kind));
            }
        }
    }
    Ok(())
}

fn reject_plan_decision_inputs(steps: &[StepDefinition]) -> Result<(), DefinitionError> {
    for step in steps {
        let decisions = step
            .inputs
            .iter()
            .filter(|input| input.kind == ArtefactKind::PlanDecision)
            .count();
        if decisions > 0
            && (decisions != 1
                || step
                    .inputs
                    .iter()
                    .filter(|input| input.kind == ArtefactKind::Plan)
                    .count()
                    != 1)
        {
            return Err(DefinitionError::PlanDecisionInput);
        }
    }
    Ok(())
}

fn reject_role_use(
    roles: &[RoleDefinition],
    steps: &[StepDefinition],
) -> Result<(), DefinitionError> {
    let mut used = Vec::new();
    for step in steps {
        if let StepAction::Agent(action) = &step.action {
            if !roles.iter().any(|role| role.key == action.role) {
                return Err(DefinitionError::UnknownRole);
            }
            if !used.contains(&action.role) {
                used.push(action.role.clone());
            }
        }
    }
    if used.len() != roles.len() {
        return Err(DefinitionError::UnusedRole);
    }
    Ok(())
}

fn reject_step_outputs(steps: &[StepDefinition]) -> Result<(), DefinitionError> {
    for step in steps {
        let outputs = match &step.action {
            StepAction::Agent(action) => &action.required_outputs,
            StepAction::SystemCommand(action) => &action.required_outputs,
            StepAction::HumanGate(action) => std::slice::from_ref(&action.required_output),
        };
        if outputs.len() > MAXIMUM_OUTPUTS {
            return Err(DefinitionError::OutputCount);
        }
        for (index, output) in outputs.iter().enumerate() {
            if outputs
                .iter()
                .skip(index + 1)
                .any(|other| other.key == output.key)
            {
                return Err(DefinitionError::DuplicateOutput);
            }
        }
    }
    Ok(())
}

fn reject_review_policies(steps: &[StepDefinition]) -> Result<(), DefinitionError> {
    let mut attempt_bound = steps.len();
    let mut artefact_bound = 1usize;
    for (gate_index, step) in steps.iter().enumerate() {
        artefact_bound = artefact_bound
            .checked_add(
                step.required_outputs()
                    .iter()
                    .filter(|output| output.kind.as_artefact_kind().is_some())
                    .count(),
            )
            .ok_or(DefinitionError::RunBound)?;
        let Some(policy) = &step.review else {
            continue;
        };
        if !(MINIMUM_REVIEW_ATTEMPTS..=MAXIMUM_REVIEW_ATTEMPTS).contains(&policy.attempt_limit) {
            return Err(DefinitionError::AttemptLimit);
        }
        let Some(target_index) = steps
            .iter()
            .position(|item| item.key == policy.revision_target)
        else {
            return Err(DefinitionError::UnknownStep);
        };
        if target_index >= gate_index {
            return Err(DefinitionError::ReviewPolicy);
        }
        if !matches!(step.action, StepAction::Agent(_))
            || !step.required_outputs().iter().any(|output| {
                output.key == policy.report_output && output.kind == OutputKind::ReviewReport
            })
        {
            return Err(DefinitionError::ReviewPolicy);
        }
        if !matches!(&steps[target_index].action, StepAction::Agent(_)) {
            return Err(DefinitionError::ReviewPolicy);
        }
        let interval = &steps[target_index..=gate_index];
        let repeats = usize::from(policy.attempt_limit - 1);
        attempt_bound = attempt_bound
            .checked_add(
                interval
                    .len()
                    .checked_mul(repeats)
                    .ok_or(DefinitionError::RunBound)?,
            )
            .ok_or(DefinitionError::RunBound)?;
        let interval_outputs: usize = interval
            .iter()
            .map(|item| {
                item.required_outputs()
                    .iter()
                    .filter(|output| output.kind.as_artefact_kind().is_some())
                    .count()
            })
            .sum();
        artefact_bound = artefact_bound
            .checked_add(
                interval_outputs
                    .checked_mul(repeats)
                    .ok_or(DefinitionError::RunBound)?,
            )
            .ok_or(DefinitionError::RunBound)?;
    }
    if attempt_bound > MAXIMUM_RUN_ATTEMPTS
        || artefact_bound > crate::workflows::artefacts::MAXIMUM_ARTEFACTS
    {
        return Err(DefinitionError::RunBound);
    }
    Ok(())
}

fn reject_human_revisions(steps: &[StepDefinition]) -> Result<(), DefinitionError> {
    let mut attempt_bound = steps.len();
    let mut artefact_bound = 1usize
        .checked_add(
            steps
                .iter()
                .map(|step| {
                    step.required_outputs()
                        .iter()
                        .filter(|output| output.kind.as_artefact_kind().is_some())
                        .count()
                })
                .sum::<usize>(),
        )
        .ok_or(DefinitionError::RunBound)?;
    for (gate_index, step) in steps.iter().enumerate() {
        let StepAction::HumanGate(action) = &step.action else {
            continue;
        };
        let Some(policy) = &action.revision else {
            continue;
        };
        if !(MINIMUM_REVIEW_ATTEMPTS..=MAXIMUM_REVIEW_ATTEMPTS).contains(&policy.attempt_limit) {
            return Err(DefinitionError::AttemptLimit);
        }
        let Some(target_index) = steps
            .iter()
            .position(|item| item.key == policy.revision_target)
        else {
            return Err(DefinitionError::UnknownStep);
        };
        let target_is_valid = matches!(
            steps[target_index].action,
            StepAction::Agent(AgentStep {
                directory_access: StepAccess::Read,
                ..
            })
        ) && steps[target_index]
            .required_outputs()
            .iter()
            .any(|output| output.kind == OutputKind::Plan)
            && steps[target_index].inputs.iter().any(|input| {
                input.kind == ArtefactKind::Plan && input.source == ArtefactSource::RunCurrentPlan
            });
        if target_index >= gate_index || !target_is_valid {
            return Err(DefinitionError::ReviewPolicy);
        }
        let interval = &steps[target_index..=gate_index];
        let repeats = usize::from(policy.attempt_limit - 1);
        attempt_bound = attempt_bound
            .checked_add(
                interval
                    .len()
                    .checked_mul(repeats)
                    .ok_or(DefinitionError::RunBound)?,
            )
            .ok_or(DefinitionError::RunBound)?;
        let interval_outputs = interval
            .iter()
            .map(|item| {
                item.required_outputs()
                    .iter()
                    .filter(|output| output.kind.as_artefact_kind().is_some())
                    .count()
            })
            .sum::<usize>();
        artefact_bound = artefact_bound
            .checked_add(
                interval_outputs
                    .checked_mul(repeats)
                    .ok_or(DefinitionError::RunBound)?,
            )
            .ok_or(DefinitionError::RunBound)?;
    }
    for (gate_index, step) in steps.iter().enumerate() {
        let Some(policy) = step.review.as_ref() else {
            continue;
        };
        let Some(target_index) = steps
            .iter()
            .position(|item| item.key == policy.revision_target)
        else {
            return Err(DefinitionError::UnknownStep);
        };
        let interval = &steps[target_index..=gate_index];
        let repeats = usize::from(policy.attempt_limit - 1);
        attempt_bound = attempt_bound
            .checked_add(
                interval
                    .len()
                    .checked_mul(repeats)
                    .ok_or(DefinitionError::RunBound)?,
            )
            .ok_or(DefinitionError::RunBound)?;
        let interval_outputs = interval
            .iter()
            .map(|item| {
                item.required_outputs()
                    .iter()
                    .filter(|output| output.kind.as_artefact_kind().is_some())
                    .count()
            })
            .sum::<usize>();
        artefact_bound = artefact_bound
            .checked_add(
                interval_outputs
                    .checked_mul(repeats)
                    .ok_or(DefinitionError::RunBound)?,
            )
            .ok_or(DefinitionError::RunBound)?;
    }
    if attempt_bound > MAXIMUM_RUN_ATTEMPTS
        || artefact_bound > crate::workflows::artefacts::MAXIMUM_ARTEFACTS
    {
        return Err(DefinitionError::RunBound);
    }
    Ok(())
}

fn parse_key(raw: &str) -> Result<String, DefinitionError> {
    let key = raw.trim();
    if key.is_empty() || key.len() > MAXIMUM_KEY_BYTES {
        return Err(DefinitionError::Key);
    }
    let mut characters = key.chars();
    let Some(first) = characters.next() else {
        return Err(DefinitionError::Key);
    };
    if !first.is_ascii_alphabetic() {
        return Err(DefinitionError::Key);
    }
    if !characters
        .all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_')
    {
        return Err(DefinitionError::Key);
    }
    Ok(key.to_owned())
}

fn normalise_name(raw: &str) -> Result<String, DefinitionError> {
    let name = raw.trim();
    if name.is_empty() || name.len() > MAXIMUM_NAME_BYTES || name.chars().any(char::is_control) {
        return Err(DefinitionError::Name);
    }
    Ok(name.to_owned())
}

fn normalise_text(
    raw: &str,
    maximum: usize,
    error: DefinitionError,
) -> Result<String, DefinitionError> {
    if raw.len() > maximum
        || raw
            .chars()
            .any(|character| character.is_control() && character != '\n' && character != '\t')
    {
        return Err(error);
    }
    Ok(raw.to_owned())
}

fn normalise_alias(raw: &str) -> Result<String, DefinitionError> {
    let alias = parse_key(raw).map_err(|_| DefinitionError::Alias)?;
    Ok(alias)
}

#[cfg(test)]
pub(in crate::workflows) mod tests;
