use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::agents::{AccessMode, ToolId};
use crate::environments::EnvironmentId;
use crate::execution::ExecutionSettings;
use crate::hex;

use super::commands::CommandSourceEffect;
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
pub(crate) const CANDIDATE_OUTPUT_KEY: &str = "candidate";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommitPolicy {
    NoCommit,
    HumanApproval,
    AutomaticAfterReview,
}

impl CommitPolicy {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "no-commit" => Some(Self::NoCommit),
            "human-approval" => Some(Self::HumanApproval),
            "automatic-after-review" => Some(Self::AutomaticAfterReview),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NoCommit => "no-commit",
            Self::HumanApproval => "human-approval",
            Self::AutomaticAfterReview => "automatic-after-review",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::NoCommit => "No commit",
            Self::HumanApproval => "Human approval before commit",
            Self::AutomaticAfterReview => "Automatic commit after approved review",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkflowDefinition {
    format_version: u32,
    name: String,
    default_environment: EnvironmentId,
    roles: Vec<RoleDefinition>,
    steps: Vec<StepDefinition>,
    commit_policy: CommitPolicy,
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
    pub(crate) candidate_authority: CandidateAuthority,
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
                    || (grant.access != crate::execution::DirectoryAccess::ReadOnly
                        && existing.access != grant.access)
            })
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CandidateAuthority {
    ReadOnly,
    Edit,
}

impl CandidateAuthority {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "read-only" => Some(Self::ReadOnly),
            "edit-candidate" => Some(Self::Edit),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::Edit => "edit-candidate",
        }
    }

    pub(crate) fn access(self) -> AccessMode {
        match self {
            Self::ReadOnly => AccessMode::ReadOnly,
            Self::Edit => AccessMode::ReadWrite,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "Read-only",
            Self::Edit => "Can edit candidate",
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
    RunInitialCandidate,
    RunCurrentCandidate,
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
    CandidateRevision,
    ReviewReport,
    TestReport,
    HumanDecision,
    PlanDecision,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ArtefactKind {
    Plan,
    CandidateRevision,
    ReviewReport,
    TestReport,
    HumanDecision,
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
    ForwardInput,
    SelfInput,
    UnknownOutput,
    InputKind,
    AssistantInput,
    CandidateInput,
    CandidateOutput,
    AssuranceInput,
    PlanDecisionInput,
    SecondaryWrite,
    UnusedRole,
    UnknownRole,
    UnknownStep,
    Command,
    Tools,
    Authority,
    WriteStrategy,
    Alias,
    DuplicateAlias,
    HumanGate,
    ReviewPolicy,
    AttemptLimit,
    CommitPolicy,

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
            Self::WriteStrategy => {
                "A root cannot bypass candidate approval through Direct write or unrestricted host execution. Read-only review phases cannot request direct writes."
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
            Self::ForwardInput => "An input cannot name a later step.",
            Self::SelfInput => "An input cannot name its own step.",
            Self::UnknownOutput => "An input names an unknown earlier output.",
            Self::InputKind => "That input kind does not match the named output.",
            Self::AssistantInput => "Assistant replies cannot be artefact inputs.",
            Self::CandidateInput => {
                "Each sandbox-backed step needs exactly one latest candidate input."
            }
            Self::CandidateOutput => {
                "Candidate access does not match the candidate revision outputs."
            }
            Self::AssuranceInput => {
                "A step that uses an assurance artefact also needs a candidate input."
            }

            Self::PlanDecisionInput => "A plan decision must accompany the exact accepted plan.",
            Self::SecondaryWrite => "Secondary directory grants must stay read-only.",
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
                "A human gate needs one candidate input and one human decision output."
            }
            Self::ReviewPolicy => "Configure a valid review policy.",
            Self::AttemptLimit => "Set the review attempt limit from one through eight.",
            Self::CommitPolicy => "Choose a commit policy that matches this workflow.",

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
    #[serde(default)]
    commit_policy: Option<String>,
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
        #[serde(rename = "candidate-authority")]
        candidate_authority: String,
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
    RunInitialCandidate,
    RunCurrentCandidate,
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
        assemble(name, default_environment, roles, steps, None)
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn default_environment(&self) -> EnvironmentId {
        self.default_environment
    }

    pub(crate) fn commit_policy(&self) -> CommitPolicy {
        self.commit_policy
    }

    pub(crate) fn with_commit_policy(&self, policy: CommitPolicy) -> Result<Self, DefinitionError> {
        let mut steps = self.steps.clone();
        if policy == CommitPolicy::AutomaticAfterReview {
            let removed_gates: Vec<_> = steps
                .iter()
                .filter_map(|step| match &step.action {
                    StepAction::HumanGate(action) if !action.is_plan_checkpoint() => {
                        Some((step.key.clone(), action.required_output.key.clone()))
                    }
                    _ => None,
                })
                .collect();
            if removed_gates.is_empty() {
                return assemble(
                    self.name.clone(),
                    self.default_environment,
                    self.roles.clone(),
                    steps,
                    Some(policy),
                );
            }
            steps.retain(|step| {
                !matches!(&step.action, StepAction::HumanGate(action) if !action.is_plan_checkpoint())
            });
            for step in &mut steps {
                step.inputs.retain(|input| {
                    !removed_gates.iter().any(|(gate, output)| {
                        matches!(
                            &input.source,
                            ArtefactSource::StepOutput { step, output: input_output }
                                if step == gate && input_output == output
                        )
                    })
                });
            }
        }
        assemble(
            self.name.clone(),
            self.default_environment,
            self.roles.clone(),
            steps,
            Some(policy),
        )
    }

    pub(crate) fn commit_policy_choices(&self) -> Vec<CommitPolicy> {
        [
            CommitPolicy::NoCommit,
            CommitPolicy::HumanApproval,
            CommitPolicy::AutomaticAfterReview,
        ]
        .into_iter()
        .filter(|policy| self.with_commit_policy(*policy).is_ok())
        .collect()
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
        let combined = ExecutionSettings::combined(phases.iter().map(|(_, settings)| settings));
        if !phases.is_empty() && combined.is_none() {
            return Err(DefinitionError::WriteStrategy);
        }
        // An override cannot move an approved candidate's destination outside its review boundary.
        let requires_candidate_approval = self.steps.iter().any(|step| {
            step.writes_primary_source()
                || matches!(&step.action, StepAction::HumanGate(gate) if !gate.is_plan_checkpoint())
        });
        let requested_roots: Vec<_> = self
            .steps
            .iter()
            .filter_map(|step| {
                if let StepAction::Agent(action) = &step.action {
                    Some(action.settings.resolve(defaults).directories)
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        for (key, effective) in phases {
            if !matches!(
                self.step(key),
                Some(StepDefinition {
                    action: StepAction::Agent(_),
                    ..
                })
            ) {
                return Err(DefinitionError::Authority);
            }
            if effective.location == crate::execution::ToolLocation::Host
                && (requires_candidate_approval
                    || self.steps.iter().any(|step| {
                        step.writes_primary_source()
                            || matches!(
                                &step.action,
                                StepAction::SystemCommand(action)
                                    if matches!(
                                        action.command,
                                        SystemCommandId::CommitCandidate
                                            | SystemCommandId::ApplyChanges
                                    )
                            )
                    }))
            {
                return Err(DefinitionError::WriteStrategy);
            }
            if effective.location == crate::execution::ToolLocation::Host
                && effective.directories.iter().any(|grant| {
                    grant.access == crate::execution::DirectoryAccess::ReviewBeforeApply
                })
            {
                return Err(DefinitionError::WriteStrategy);
            }
            if effective.directories.iter().any(|grant| {
                requires_candidate_approval
                    && grant.access == crate::execution::DirectoryAccess::DirectWrite
                    && defaults
                        .directories
                        .iter()
                        .chain(&requested_roots)
                        .any(|root| {
                            (root.identity == grant.identity
                                || root.host_path.starts_with(&grant.host_path)
                                || grant.host_path.starts_with(&root.host_path))
                                && root.access
                                    == crate::execution::DirectoryAccess::ReviewBeforeApply
                        })
            }) {
                return Err(DefinitionError::WriteStrategy);
            }
        }
        for step in &mut steps {
            let resolved = match &step.action {
                StepAction::Agent(_) => phases
                    .iter()
                    .find(|(key, _)| key == &step.key)
                    .map(|(_, settings)| settings),
                StepAction::SystemCommand(_) | StepAction::HumanGate(_) => None,
            };
            let effective = resolved.unwrap_or(combined.as_ref().unwrap_or(defaults));
            let reviewed = effective
                .directories
                .iter()
                .any(|grant| grant.access == crate::execution::DirectoryAccess::ReviewBeforeApply);
            if step.writes_primary_source() && !reviewed {
                return Err(DefinitionError::Authority);
            }
            if effective.directories.is_empty()
                && step.inputs.iter().any(|input| {
                    matches!(
                        input.source,
                        ArtefactSource::RunInitialCandidate | ArtefactSource::RunCurrentCandidate
                    )
                })
            {
                return Err(DefinitionError::Authority);
            }
            if let StepAction::Agent(action) = &mut step.action {
                if action.candidate_authority == CandidateAuthority::ReadOnly
                    && action
                        .required_outputs
                        .iter()
                        .any(|output| output.kind == OutputKind::ReviewReport)
                    && effective
                        .directories
                        .iter()
                        .any(|grant| grant.access == crate::execution::DirectoryAccess::DirectWrite)
                {
                    return Err(DefinitionError::WriteStrategy);
                }
                if reviewed
                    && !step
                        .inputs
                        .iter()
                        .any(|input| input.kind == ArtefactKind::CandidateRevision)
                {
                    let mut input = initial_candidate_input();
                    input.source = ArtefactSource::RunCurrentCandidate;
                    step.inputs.push(input);
                }
                action.authority = AgentAuthority::new(
                    effective.tools.clone(),
                    effective
                        .directories
                        .iter()
                        .map(|grant| GuestDirectoryAccess {
                            alias: grant.alias.clone(),
                            access: AccessMode::ReadOnly,
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
            commit_policy: Some(self.commit_policy.as_str().to_owned()),

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

    pub(crate) fn writes_primary_source(&self) -> bool {
        matches!(
            &self.action,
            StepAction::Agent(AgentStep {
                candidate_authority: CandidateAuthority::Edit,
                ..
            })
        )
    }

    pub(crate) fn command_source_effect(&self) -> Option<CommandSourceEffect> {
        match &self.action {
            StepAction::SystemCommand(action) => Some(action.command.contract().source_effect),
            StepAction::Agent(_) | StepAction::HumanGate(_) => None,
        }
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
                        ArtefactSource::RunInitialCandidate => InputSourceFile::RunInitialCandidate,
                        ArtefactSource::RunCurrentCandidate => InputSourceFile::RunCurrentCandidate,
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
                candidate_authority,
                authority,
                required_outputs,
                settings,
            } => Ok(Self::Agent(AgentStep {
                role: RoleKey::parse(&role)?,
                environment: StepEnvironment::from_file(environment)?,
                candidate_authority: CandidateAuthority::parse(&candidate_authority)
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
                candidate_authority: step.candidate_authority.as_str().to_owned(),
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
            if directory.access.is_writable() {
                return Err(DefinitionError::SecondaryWrite);
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
            "candidate-revision" => Some(Self::CandidateRevision),
            "review-report" => Some(Self::ReviewReport),
            "test-report" => Some(Self::TestReport),
            "human-decision" => Some(Self::HumanDecision),
            "plan-decision" => Some(Self::PlanDecision),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::AssistantReply => "assistant-reply",
            Self::Plan => "plan",
            Self::CandidateRevision => "candidate-revision",
            Self::ReviewReport => "review-report",
            Self::TestReport => "test-report",
            Self::HumanDecision => "human-decision",
            Self::PlanDecision => "plan-decision",
        }
    }

    pub(crate) fn as_artefact_kind(self) -> Option<ArtefactKind> {
        match self {
            Self::AssistantReply => None,
            Self::Plan => Some(ArtefactKind::Plan),
            Self::CandidateRevision => Some(ArtefactKind::CandidateRevision),
            Self::ReviewReport => Some(ArtefactKind::ReviewReport),
            Self::TestReport => Some(ArtefactKind::TestReport),
            Self::HumanDecision => Some(ArtefactKind::HumanDecision),
            Self::PlanDecision => Some(ArtefactKind::PlanDecision),
        }
    }
}

impl ArtefactKind {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "plan" => Some(Self::Plan),
            "candidate-revision" => Some(Self::CandidateRevision),
            "review-report" => Some(Self::ReviewReport),
            "test-report" => Some(Self::TestReport),
            "human-decision" => Some(Self::HumanDecision),
            "plan-decision" => Some(Self::PlanDecision),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::CandidateRevision => "candidate-revision",
            Self::ReviewReport => "review-report",
            Self::TestReport => "test-report",
            Self::HumanDecision => "human-decision",
            Self::PlanDecision => "plan-decision",
        }
    }

    pub(crate) fn is_assurance(self) -> bool {
        matches!(self, Self::ReviewReport | Self::TestReport)
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
    requested_policy: Option<CommitPolicy>,
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
    let mut roots: Vec<&crate::execution::DirectoryGrant> = Vec::new();
    for step in &steps {
        if let StepAction::Agent(action) = &step.action
            && let ModelStepSettings::Override(settings) = &action.settings
            && let Some(directories) = &settings.directories
        {
            for grant in directories {
                if roots.iter().any(|root| {
                    root.identity == grant.identity
                        && root.access != grant.access
                        && root.access != crate::execution::DirectoryAccess::ReadOnly
                        && grant.access != crate::execution::DirectoryAccess::ReadOnly
                }) {
                    return Err(DefinitionError::WriteStrategy);
                }
                roots.push(grant);
            }
        }
    }
    reject_duplicate_roles(&roles)?;
    reject_duplicate_steps(&steps)?;
    reject_step_outputs(&steps)?;
    reject_step_inputs(&steps)?;
    reject_unsupported_outputs(&steps)?;
    reject_secondary_writes(&steps)?;
    reject_role_use(&roles, &steps)?;
    reject_review_policies(&steps)?;
    reject_human_revisions(&steps)?;
    reject_handoff(&steps)?;
    reject_plan_decision_inputs(&steps)?;
    let commit_policy = requested_policy.unwrap_or_else(|| derive_commit_policy(&steps));
    reject_commit_policy(&steps, commit_policy)?;

    Ok(WorkflowDefinition {
        format_version: DEFINITION_FORMAT_VERSION,
        name,
        default_environment,
        roles,
        steps,
        commit_policy,
    })
}

fn from_current_file(file: DefinitionFile) -> Result<WorkflowDefinition, DefinitionError> {
    let requested_policy = match file.commit_policy.as_deref() {
        None => None,
        Some(value) => Some(CommitPolicy::parse(value).ok_or(DefinitionError::Format)?),
    };

    let (name, default_environment, roles, steps) = parse_file_parts(file)?;
    assemble(name, default_environment, roles, steps, requested_policy)
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

fn produces_candidate_revision(step: &StepDefinition) -> bool {
    step.writes_primary_source()
        || matches!(
            step.command_source_effect(),
            Some(CommandSourceEffect::Apply | CommandSourceEffect::Commit)
        )
}

fn has_secondary_write(step: &StepDefinition) -> bool {
    let StepAction::Agent(action) = &step.action else {
        return false;
    };
    action
        .authority
        .directories
        .iter()
        .any(|directory| directory.access.is_writable())
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
            InputSourceFile::RunInitialCandidate => ArtefactSource::RunInitialCandidate,
            InputSourceFile::RunCurrentCandidate => ArtefactSource::RunCurrentCandidate,
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
                let candidate_outputs = action
                    .required_outputs
                    .iter()
                    .filter(|output| output.kind == OutputKind::CandidateRevision)
                    .count();
                let review_outputs = action
                    .required_outputs
                    .iter()
                    .filter(|output| output.kind == OutputKind::ReviewReport)
                    .count();
                if action.required_outputs.iter().any(|output| {
                    matches!(
                        output.kind,
                        OutputKind::HumanDecision | OutputKind::PlanDecision
                    )
                }) {
                    return Err(DefinitionError::UnsupportedOutput);
                }
                match action.candidate_authority {
                    CandidateAuthority::ReadOnly if candidate_outputs != 0 => {
                        return Err(DefinitionError::CandidateOutput);
                    }
                    CandidateAuthority::Edit if candidate_outputs != 1 => {
                        return Err(DefinitionError::CandidateOutput);
                    }
                    _ => {}
                }
                if candidate_outputs == 1 && review_outputs > 1 {
                    return Err(DefinitionError::UnsupportedOutput);
                }
            }
            StepAction::SystemCommand(action) => {
                let contract = action.command.contract();
                let input_kinds: Vec<_> = step.inputs.iter().map(|input| input.kind).collect();
                let output_kinds: Vec<_> = action
                    .required_outputs
                    .iter()
                    .map(|output| output.kind)
                    .collect();
                if !contract.accepts(&input_kinds, &output_kinds) {
                    return Err(DefinitionError::UnsupportedOutput);
                }
            }
            StepAction::HumanGate(action) => {
                let valid_inputs = if action.is_plan_checkpoint() {
                    action.required_output.kind == OutputKind::PlanDecision
                        && step
                            .inputs
                            .iter()
                            .filter(|input| input.kind == ArtefactKind::Plan)
                            .count()
                            == 1
                        && !step
                            .inputs
                            .iter()
                            .any(|input| input.kind == ArtefactKind::CandidateRevision)
                } else {
                    action.required_output.kind == OutputKind::HumanDecision
                        && step
                            .inputs
                            .iter()
                            .filter(|input| input.kind == ArtefactKind::CandidateRevision)
                            .count()
                            == 1
                };
                if !valid_inputs
                    || step.inputs.iter().any(|input| {
                        !matches!(
                            input.kind,
                            ArtefactKind::CandidateRevision
                                | ArtefactKind::Plan
                                | ArtefactKind::ReviewReport
                                | ArtefactKind::TestReport
                                | ArtefactKind::HumanDecision
                                | ArtefactKind::PlanDecision
                        )
                    })
                {
                    return Err(DefinitionError::HumanGate);
                }
            }
        }
    }
    Ok(())
}

fn reject_secondary_writes(steps: &[StepDefinition]) -> Result<(), DefinitionError> {
    for step in steps {
        if has_secondary_write(step) {
            return Err(DefinitionError::SecondaryWrite);
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
    let mut produced: Vec<(StepKey, OutputKey, ArtefactKind)> = Vec::new();
    let mut latest_candidate: Option<(StepKey, OutputKey)> = None;
    for (index, step) in steps.iter().enumerate() {
        let candidate_inputs: Vec<_> = step
            .inputs
            .iter()
            .filter(|input| input.kind == ArtefactKind::CandidateRevision)
            .collect();
        let assurance = step.inputs.iter().any(|input| input.kind.is_assurance())
            || step.required_outputs().iter().any(|output| {
                matches!(
                    output.kind,
                    OutputKind::ReviewReport | OutputKind::TestReport
                )
            });
        let plan_checkpoint = matches!(
            &step.action,
            StepAction::HumanGate(action) if action.is_plan_checkpoint()
        );
        if plan_checkpoint {
            if !candidate_inputs.is_empty()
                || step
                    .inputs
                    .iter()
                    .filter(|input| input.kind == ArtefactKind::Plan)
                    .count()
                    != 1
            {
                return Err(DefinitionError::CandidateInput);
            }
        } else if step.is_sandbox_backed()
            || assurance
            || matches!(step.action, StepAction::HumanGate(_))
        {
            let source_free_agent = matches!(
                &step.action,
                StepAction::Agent(AgentStep {
                    candidate_authority: CandidateAuthority::ReadOnly,
                    ..
                })
            ) && candidate_inputs.is_empty();
            if candidate_inputs.len() != 1 && !source_free_agent {
                return Err(DefinitionError::CandidateInput);
            }
            let Some(candidate) = candidate_inputs.first() else {
                continue;
            };
            match &candidate.source {
                ArtefactSource::RunInitialCandidate => {
                    if latest_candidate.is_some() {
                        return Err(DefinitionError::CandidateInput);
                    }
                }
                ArtefactSource::RunCurrentCandidate => {}
                ArtefactSource::RunCurrentPlan => {}

                ArtefactSource::StepOutput {
                    step: source_step,
                    output,
                } => {
                    let Some((latest_step, latest_output)) = &latest_candidate else {
                        return Err(DefinitionError::CandidateInput);
                    };
                    if source_step != latest_step || output != latest_output {
                        return Err(DefinitionError::CandidateInput);
                    }
                }
            }
        } else if !candidate_inputs.is_empty() {
            return Err(DefinitionError::CandidateInput);
        }
        for input in &step.inputs {
            match &input.source {
                ArtefactSource::RunInitialCandidate | ArtefactSource::RunCurrentCandidate => {
                    if input.kind != ArtefactKind::CandidateRevision {
                        return Err(DefinitionError::InputKind);
                    }
                }
                ArtefactSource::RunCurrentPlan => {
                    if input.kind != ArtefactKind::Plan
                        || (!produced
                            .iter()
                            .any(|(_, _, kind)| *kind == ArtefactKind::Plan))
                    {
                        return Err(DefinitionError::InputKind);
                    }
                }

                ArtefactSource::StepOutput {
                    step: source_step,
                    output,
                } => {
                    if source_step == &step.key {
                        return Err(DefinitionError::SelfInput);
                    }
                    if let Some(produced_kind) =
                        produced.iter().find_map(|(item_step, item_output, kind)| {
                            (*item_step == *source_step && *item_output == *output).then_some(*kind)
                        })
                    {
                        if produced_kind != input.kind {
                            return Err(DefinitionError::InputKind);
                        }
                    } else {
                        let Some(source_index) =
                            steps.iter().position(|item| item.key == *source_step)
                        else {
                            return Err(DefinitionError::UnknownOutput);
                        };
                        let source = &steps[source_index];
                        if source_index >= index {
                            return Err(DefinitionError::ForwardInput);
                        }
                        if source.required_outputs().iter().any(|item| {
                            item.key == *output && item.kind == OutputKind::AssistantReply
                        }) {
                            return Err(DefinitionError::AssistantInput);
                        }
                        return Err(DefinitionError::UnknownOutput);
                    }
                }
            }
        }
        if assurance && candidate_inputs.is_empty() {
            return Err(DefinitionError::AssuranceInput);
        }
        let mut candidate_outputs = 0usize;
        for output in step.required_outputs() {
            if output.kind == OutputKind::CandidateRevision {
                candidate_outputs += 1;
            }
            if let Some(kind) = output.kind.as_artefact_kind() {
                produced.push((step.key.clone(), output.key.clone(), kind));
                if kind == ArtefactKind::CandidateRevision {
                    latest_candidate = Some((step.key.clone(), output.key.clone()));
                }
            }
        }
        if produces_candidate_revision(step) {
            if candidate_outputs != 1 {
                return Err(DefinitionError::CandidateOutput);
            }
        } else if candidate_outputs != 0 {
            return Err(DefinitionError::CandidateOutput);
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

fn derive_commit_policy(steps: &[StepDefinition]) -> CommitPolicy {
    let has_commit = steps.iter().any(|step| {
        matches!(
            &step.action,
            StepAction::SystemCommand(action)
                if action.command == SystemCommandId::CommitCandidate
        )
    });
    if !has_commit {
        return CommitPolicy::NoCommit;
    }
    if steps.iter().any(|step| {
        matches!(&step.action, StepAction::HumanGate(action) if !action.is_plan_checkpoint())
    }) {
        CommitPolicy::HumanApproval
    } else {
        CommitPolicy::AutomaticAfterReview
    }
}

fn reject_commit_policy(
    steps: &[StepDefinition],
    policy: CommitPolicy,
) -> Result<(), DefinitionError> {
    let commits: Vec<_> = steps
        .iter()
        .filter(|step| {
            matches!(
                &step.action,
                StepAction::SystemCommand(action)
                    if action.command == SystemCommandId::CommitCandidate
            )
        })
        .collect();
    if policy == CommitPolicy::NoCommit {
        return if commits.is_empty() {
            Ok(())
        } else {
            Err(DefinitionError::CommitPolicy)
        };
    }
    if commits.len() != 1 {
        return Err(DefinitionError::CommitPolicy);
    }
    let commit = commits[0];
    let decision = commit
        .inputs
        .iter()
        .any(|input| input.kind == ArtefactKind::HumanDecision);
    let reviews = commit
        .inputs
        .iter()
        .any(|input| input.kind == ArtefactKind::ReviewReport);
    match policy {
        CommitPolicy::NoCommit => Err(DefinitionError::CommitPolicy),
        CommitPolicy::HumanApproval if !decision => Err(DefinitionError::CommitPolicy),
        CommitPolicy::AutomaticAfterReview if decision || !reviews => {
            Err(DefinitionError::CommitPolicy)
        }
        CommitPolicy::HumanApproval | CommitPolicy::AutomaticAfterReview => Ok(()),
    }
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
        let interval = &steps[target_index..=gate_index];
        if !matches!(
            steps[target_index].action,
            StepAction::Agent(AgentStep {
                candidate_authority: CandidateAuthority::Edit,
                ..
            })
        ) || steps[target_index]
            .inputs
            .iter()
            .find(|input| input.kind == ArtefactKind::CandidateRevision)
            .is_none_or(|input| input.source != ArtefactSource::RunCurrentCandidate)
            || !interval.iter().any(StepDefinition::writes_primary_source)
            || interval.iter().any(|item| {
                matches!(
                    &item.action,
                    StepAction::SystemCommand(action)
                        if action.command == SystemCommandId::CommitCandidate
                )
            })
        {
            return Err(DefinitionError::ReviewPolicy);
        }
        for interval_step in &steps[target_index..] {
            if (interval_step.is_sandbox_backed()
                || matches!(interval_step.action, StepAction::HumanGate(_)))
                && interval_step
                    .inputs
                    .iter()
                    .find(|input| input.kind == ArtefactKind::CandidateRevision)
                    .is_none_or(|input| input.source != ArtefactSource::RunCurrentCandidate)
            {
                return Err(DefinitionError::CandidateInput);
            }
        }
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
    for (commit_index, commit) in steps.iter().enumerate().filter(|(_, step)| {
        matches!(
            &step.action,
            StepAction::SystemCommand(action) if action.command == SystemCommandId::CommitCandidate
        )
    }) {
        for review_step in steps[..commit_index]
            .iter()
            .filter(|step| step.review.is_some())
        {
            let Some(policy) = &review_step.review else {
                continue;
            };
            if !commit.inputs.iter().any(|input| {
                input.kind == ArtefactKind::ReviewReport
                    && matches!(
                        &input.source,
                        ArtefactSource::StepOutput { step, output }
                            if step == &review_step.key && output == &policy.report_output
                    )
            }) {
                return Err(DefinitionError::AssuranceInput);
            }
        }
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
        let target_is_plan = action.is_plan_checkpoint();
        let target_is_valid =
            if target_is_plan {
                matches!(
                    steps[target_index].action,
                    StepAction::Agent(AgentStep {
                        candidate_authority: CandidateAuthority::ReadOnly,
                        ..
                    })
                ) && steps[target_index]
                    .required_outputs()
                    .iter()
                    .any(|output| output.kind == OutputKind::Plan)
                    && steps[target_index].inputs.iter().any(|input| {
                        input.kind == ArtefactKind::Plan
                            && input.source == ArtefactSource::RunCurrentPlan
                    })
            } else {
                !steps[target_index].required_outputs().iter().any(|output| {
                    output.kind.as_artefact_kind() == Some(ArtefactKind::ReviewReport)
                }) && matches!(
                    steps[target_index].action,
                    StepAction::Agent(AgentStep {
                        candidate_authority: CandidateAuthority::Edit,
                        ..
                    })
                ) && steps[target_index]
                    .inputs
                    .iter()
                    .find(|input| input.kind == ArtefactKind::CandidateRevision)
                    .is_none_or(|input| input.source == ArtefactSource::RunCurrentCandidate)
            };
        if target_index >= gate_index
            || !target_is_valid
            || steps[target_index..=gate_index].iter().any(|item| {
                matches!(
                    &item.action,
                    StepAction::SystemCommand(action)
                        if action.command == SystemCommandId::CommitCandidate
                )
            })
        {
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

pub(crate) fn initial_candidate_input() -> RequiredInput {
    RequiredInput {
        key: InputKey::parse("candidate").expect("candidate input"),
        kind: ArtefactKind::CandidateRevision,
        source: ArtefactSource::RunInitialCandidate,
    }
}

pub(crate) fn candidate_revision_output() -> RequiredOutput {
    RequiredOutput {
        key: OutputKey::parse(CANDIDATE_OUTPUT_KEY).expect("candidate output"),
        kind: OutputKind::CandidateRevision,
    }
}

#[cfg(test)]
pub(in crate::workflows) mod tests;
