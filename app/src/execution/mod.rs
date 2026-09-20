mod approval;
pub(crate) mod authority;
pub(crate) mod command;
mod consent;
mod directory_picker;
mod host;
mod output;
mod overrides;
mod settings;
pub(crate) use overrides::{SettingsOverrides, SettingsOverridesFile};

pub(crate) use approval::{
    ApprovalError, HostApprovalStore, HostCommandDecision, HostCommandRequest, command_token,
};
pub(crate) use authority::ProjectFreeAuthority;
pub(crate) use command::{CommandFailure, CommandResult, CommandStream, CommandTermination};
pub(crate) use consent::{AccessConsentStore, draft_nonce, settings_digest};
pub(crate) use directory_picker::{DirectoryPick, DirectoryPicker};
pub(crate) use host::{
    COMMAND_TIMEOUT, CommandReporter, HostIdentity, command_directory, run_shell_reported,
};
pub(crate) use output::{OUTPUT_PREVIEW_BYTES, OutputKey, OutputScope, OutputStore};
pub(crate) use settings::{
    CanonicalDirectoryIdentity, DirectoryAccess, DirectoryGrant, DirectoryGrantError,
    DirectoryGrantId, ExecutionSettings, ExecutionSettingsFile, HostApprovalPolicy,
    MAXIMUM_DIRECTORY_GRANTS, ToolLocation, valid_alias, validate_directories,
};

pub(crate) const GUEST_WORKSPACE: &str = "/workspace";
