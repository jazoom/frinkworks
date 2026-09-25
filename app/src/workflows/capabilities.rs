use crate::agents::{AccessMode, DirectoryPolicy, EffectiveAuthority, NetworkAccess, ToolId};
use crate::workflows::definition::{StepAction, StepDefinition};

pub(crate) const CAPABILITY_SCHEMA: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AttemptCapabilities {
    pub(crate) schema: u32,
    pub(crate) agent_revision: u32,
    pub(crate) tools: Vec<ToolId>,
    pub(crate) directories: Vec<CapabilityDirectory>,
    pub(crate) source_location: PrimarySourceLocation,
    pub(crate) git_admin: AccessMode,
    pub(crate) network: NetworkCapability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PrimarySourceLocation {
    PrivateWorkspace,
    UserProject,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CapabilityDirectory {
    pub(crate) alias: String,
    pub(crate) guest_path: String,
    pub(crate) access: AccessMode,
    pub(crate) role: DirectoryRole,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DirectoryRole {
    PrimarySource,
    SecondaryContext,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum NetworkCapability {
    None,
    Restricted(Vec<String>),
    Public,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CapabilityError {
    Authority,
}

impl AttemptCapabilities {
    pub(crate) fn derive_project_free(
        step: &StepDefinition,
        authority: &crate::execution::ProjectFreeAuthority,
    ) -> Result<Self, CapabilityError> {
        derive_with_ceiling(
            step,
            authority.revision,
            &authority.tools,
            &authority.network,
            &authority.policy,
        )
    }

    pub(crate) fn derive_for_authority(
        step: &StepDefinition,
        authority: &EffectiveAuthority,
    ) -> Result<Self, CapabilityError> {
        derive_with_ceiling(
            step,
            authority.revision,
            &authority.tools,
            &authority.network,
            &authority.policy,
        )
    }

    pub(crate) fn sandbox_network(&self) -> NetworkAccess {
        match &self.network {
            NetworkCapability::None => NetworkAccess::None,
            NetworkCapability::Restricted(domains) => NetworkAccess::Restricted(domains.clone()),
            NetworkCapability::Public => NetworkAccess::Public,
        }
    }

    pub(crate) fn primary(&self) -> Option<&CapabilityDirectory> {
        self.directories
            .iter()
            .find(|directory| directory.role == DirectoryRole::PrimarySource)
    }

    pub(crate) fn tools_label(&self) -> String {
        if self.tools.is_empty() {
            return "none".to_owned();
        }
        self.tools
            .iter()
            .map(|tool| tool.label())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(crate) fn primary_access_label(&self) -> &'static str {
        match self.primary().map(|directory| directory.access) {
            Some(AccessMode::ReadWrite) => "Write",
            _ => "Read",
        }
    }

    pub(crate) fn network_label(&self) -> String {
        match &self.network {
            NetworkCapability::None => "None".to_owned(),
            NetworkCapability::Restricted(domains) => {
                format!("Restricted ({} domains)", domains.len())
            }
            NetworkCapability::Public => "Public internet".to_owned(),
        }
    }
}

fn derive_with_ceiling(
    step: &StepDefinition,
    revision: u32,
    tools: &[ToolId],
    network: &NetworkAccess,
    policy: &DirectoryPolicy,
) -> Result<AttemptCapabilities, CapabilityError> {
    let (tools, requested, maximum, network) = match &step.action {
        StepAction::Agent(action) => {
            if !action.authority.allowed_by(
                tools,
                policy
                    .grants()
                    .iter()
                    .map(|grant| (grant.alias.as_str(), grant.access)),
            ) {
                return Err(CapabilityError::Authority);
            }
            (
                action.authority.tools.clone(),
                action.authority.directories.as_slice(),
                action.directory_access.access(),
                NetworkCapability::from_agent(network),
            )
        }
        StepAction::SystemCommand(_) => (
            Vec::new(),
            &[][..],
            AccessMode::ReadOnly,
            NetworkCapability::None,
        ),
        StepAction::HumanGate(_) => return Err(CapabilityError::Authority),
    };
    let directories: Vec<_> = policy
        .grants()
        .iter()
        .map(|grant| {
            let requested = requested
                .iter()
                .find(|directory| directory.alias == grant.alias)
                .map(|directory| directory.access)
                .unwrap_or(maximum);
            let access =
                if maximum.is_writable() && requested.is_writable() && grant.access.is_writable() {
                    AccessMode::ReadWrite
                } else {
                    AccessMode::ReadOnly
                };
            CapabilityDirectory {
                alias: grant.alias.clone(),
                guest_path: grant.guest_path.clone(),
                access,
                role: if grant.alias == policy.primary_alias() {
                    DirectoryRole::PrimarySource
                } else {
                    DirectoryRole::SecondaryContext
                },
            }
        })
        .collect();
    let source_location = if directories.is_empty() {
        PrimarySourceLocation::PrivateWorkspace
    } else {
        PrimarySourceLocation::UserProject
    };
    let git_admin = directories
        .iter()
        .find(|directory| directory.role == DirectoryRole::PrimarySource)
        .map(|directory| directory.access)
        .unwrap_or(AccessMode::ReadOnly);
    Ok(AttemptCapabilities {
        schema: CAPABILITY_SCHEMA,
        agent_revision: revision,
        tools,
        directories,
        source_location,
        git_admin,
        network,
    })
}

impl PrimarySourceLocation {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::PrivateWorkspace => "private-workspace",
            Self::UserProject => "user-project",
        }
    }
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "private-workspace" => Some(Self::PrivateWorkspace),
            "user-project" => Some(Self::UserProject),
            _ => None,
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::PrivateWorkspace => "Private workspace",
            Self::UserProject => "Live directories",
        }
    }
}

impl DirectoryRole {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::PrimarySource => "primary-source",
            Self::SecondaryContext => "secondary-context",
        }
    }
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "primary-source" => Some(Self::PrimarySource),
            "secondary-context" => Some(Self::SecondaryContext),
            _ => None,
        }
    }
}

impl NetworkCapability {
    pub(crate) fn from_agent(access: &NetworkAccess) -> Self {
        match access {
            NetworkAccess::None => Self::None,
            NetworkAccess::Restricted(domains) => Self::Restricted(domains.clone()),
            NetworkAccess::Public => Self::Public,
        }
    }
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Restricted(_) => "restricted",
            Self::Public => "public",
        }
    }
    pub(crate) fn domains(&self) -> &[String] {
        match self {
            Self::Restricted(domains) => domains,
            _ => &[],
        }
    }
    pub(crate) fn parse(value: &str, domains: Vec<String>) -> Option<Self> {
        match value {
            "none" if domains.is_empty() => Some(Self::None),
            "restricted" if !domains.is_empty() => {
                let NetworkAccess::Restricted(domains) =
                    NetworkAccess::parse_form(value, &domains.join("\n")).ok()?
                else {
                    return None;
                };
                Some(Self::Restricted(domains))
            }
            "public" if domains.is_empty() => Some(Self::Public),
            _ => None,
        }
    }
}

impl std::fmt::Display for CapabilityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}
impl CapabilityError {
    pub(crate) fn message(self) -> &'static str {
        "The pinned step authority exceeds the current directory permissions."
    }
}

#[cfg(test)]
pub(in crate::workflows) mod tests;
