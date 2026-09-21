pub(crate) mod agent;
mod approval;
pub(crate) mod authority;
pub(crate) mod budget;
pub(crate) mod command;
mod consent;
pub(crate) mod context;
pub(crate) mod conversation;
mod directory_picker;
mod host;
mod output;
mod overrides;
pub(crate) mod resources;
mod settings;
pub(crate) use overrides::{SettingsOverrides, SettingsOverridesFile};

pub(crate) use agent::{AgentOutcome, AgentRunSpec, StreamRedactor, bound_reply, run_agent_action};
pub(crate) use approval::{
    ApprovalError, HostApprovalStore, HostCommandDecision, HostCommandRequest, command_token,
};
pub(crate) use authority::ProjectFreeAuthority;
pub(crate) use budget::{Budget, BudgetPolicy, BudgetReason, BudgetSnapshot};
pub(crate) use command::{CommandFailure, CommandResult, CommandStream, CommandTermination};
pub(crate) use consent::{AccessConsentStore, draft_nonce, settings_digest};
pub(crate) use context::{ContextError, ContextEstimate, ContextRequest};
pub(crate) use conversation::{ConversationRuntime, OrdinaryKind, ordinary_kind};
pub(crate) use directory_picker::{DirectoryPick, DirectoryPicker};
pub(crate) use host::{
    COMMAND_TIMEOUT, CommandReporter, HostIdentity, command_directory, run_shell_reported,
};
pub(crate) use output::{OUTPUT_PREVIEW_BYTES, OutputKey, OutputScope, OutputStore};
pub(crate) use resources::{
    InstructionSource, ResourceKind, ResourceSource, SkillAdvertisement, discover_skills,
};
pub(crate) use settings::{
    CanonicalDirectoryIdentity, DirectoryAccess, DirectoryGrant, DirectoryGrantError,
    DirectoryGrantId, ExecutionSettings, ExecutionSettingsFile, HostApprovalPolicy,
    MAXIMUM_DIRECTORY_GRANTS, ToolLocation, valid_alias, validate_directories,
};

pub(crate) const GUEST_WORKSPACE: &str = "/workspace";
