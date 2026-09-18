use crate::agents::{
    AccessMode, AgentRecord, EffectiveAuthority, NetworkAccess, ToolId, guest_path_for,
};
use crate::workflows::commands::{CommandSourceEffect, SystemCommandId};
#[cfg(test)]
use crate::workflows::definition::PRIMARY_SOURCE_ALIAS;
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
    AttemptWorkspace,
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
    pub(crate) fn derive(
        step: &StepDefinition,
        agent: &AgentRecord,
        primary_alias: &str,
    ) -> Result<Self, CapabilityError> {
        let policy = crate::agents::DirectoryPolicy::from_record_with_primary(agent, primary_alias);
        derive_with_ceiling(step, agent.revision, &agent.tools, &agent.network, &policy)
    }

    pub(crate) fn derive_project_free(
        step: &StepDefinition,
        authority: &crate::execution::ProjectFreeAuthority,
    ) -> Result<Self, CapabilityError> {
        if let StepAction::SystemCommand(action) = &step.action {
            return match action.command {
                SystemCommandId::ApplyChanges if !authority.reviewed_aliases.is_empty() => Ok(
                    apply_capabilities(authority.revision, &authority.reviewed_aliases),
                ),
                SystemCommandId::CommitCandidate | SystemCommandId::RepositoryStatus => {
                    let primary = authority.policy.primary_alias();
                    if primary.is_empty()
                        || (action.command == SystemCommandId::CommitCandidate
                            && !authority
                                .reviewed_aliases
                                .iter()
                                .any(|alias| alias == primary))
                    {
                        return Err(CapabilityError::Authority);
                    }
                    Ok(commit_or_read_only(
                        action.command,
                        authority.revision,
                        primary,
                    ))
                }
                _ => Err(CapabilityError::Authority),
            };
        }
        let StepAction::Agent(action) = &step.action else {
            return Err(CapabilityError::Authority);
        };
        let reviewed = &authority.reviewed_aliases;
        let writes =
            action.candidate_authority == crate::workflows::definition::CandidateAuthority::Edit;
        if (writes && reviewed.is_empty())
            || (!writes
                && action.required_outputs.iter().any(|output| {
                    output.kind == crate::workflows::definition::OutputKind::ReviewReport
                })
                && authority
                    .policy
                    .grants()
                    .iter()
                    .any(|grant| grant.access.is_writable()))
            || !action
                .authority
                .tools
                .iter()
                .all(|tool| authority.tools.contains(tool))
            || action.authority.directories.len() != authority.policy.grants().len()
            || action.authority.directories.iter().any(|directory| {
                directory.access != AccessMode::ReadOnly
                    || !authority
                        .policy
                        .grants()
                        .iter()
                        .any(|grant| grant.alias == directory.alias)
            })
        {
            return Err(CapabilityError::Authority);
        }
        let directories = if authority.policy.grants().is_empty() {
            vec![CapabilityDirectory {
                alias: "workspace".to_owned(),
                guest_path: crate::execution::GUEST_WORKSPACE.to_owned(),
                access: AccessMode::ReadWrite,
                role: DirectoryRole::PrimarySource,
            }]
        } else {
            authority
                .policy
                .grants()
                .iter()
                .map(|grant| {
                    let reviewed_root = reviewed.contains(&grant.alias);
                    let primary = grant.alias == authority.policy.primary_alias();
                    CapabilityDirectory {
                        alias: grant.alias.clone(),
                        guest_path: grant.guest_path.clone(),
                        access: if (reviewed_root && writes) || grant.access.is_writable() {
                            AccessMode::ReadWrite
                        } else {
                            AccessMode::ReadOnly
                        },
                        role: if primary {
                            DirectoryRole::PrimarySource
                        } else {
                            DirectoryRole::SecondaryContext
                        },
                    }
                })
                .collect()
        };
        Ok(Self {
            schema: CAPABILITY_SCHEMA,
            agent_revision: authority.revision,
            tools: action.authority.tools.clone(),
            directories,
            source_location: if step.inputs.iter().any(|input| {
                input.kind == crate::workflows::definition::ArtefactKind::CandidateRevision
            }) {
                PrimarySourceLocation::AttemptWorkspace
            } else {
                PrimarySourceLocation::PrivateWorkspace
            },
            git_admin: AccessMode::ReadOnly,
            network: NetworkCapability::from_agent(&authority.network),
        })
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
            Some(AccessMode::ReadWrite) => "read-write",
            _ => "read-only",
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

impl PrimarySourceLocation {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::AttemptWorkspace => "attempt-workspace",
            Self::PrivateWorkspace => "private-workspace",
            Self::UserProject => "user-project",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "attempt-workspace" => Some(Self::AttemptWorkspace),
            "private-workspace" => Some(Self::PrivateWorkspace),
            "user-project" => Some(Self::UserProject),
            _ => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::AttemptWorkspace => "Attempt workspace",
            Self::PrivateWorkspace => "Private workspace",
            Self::UserProject => "User project",
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
            Self::None | Self::Public => &[],
        }
    }

    pub(crate) fn parse(value: &str, domains: Vec<String>) -> Option<Self> {
        match value {
            "none" if domains.is_empty() => Some(Self::None),
            "restricted" if !domains.is_empty() => {
                let access = NetworkAccess::parse_form(value, &domains.join("\n")).ok()?;
                let NetworkAccess::Restricted(domains) = access else {
                    return None;
                };
                Some(Self::Restricted(domains))
            }
            "public" if domains.is_empty() => Some(Self::Public),
            _ => None,
        }
    }
}

fn derive_with_ceiling(
    step: &StepDefinition,
    revision: u32,
    tools: &[ToolId],
    network: &NetworkAccess,
    policy: &crate::agents::DirectoryPolicy,
) -> Result<AttemptCapabilities, CapabilityError> {
    let primary = policy
        .grants()
        .iter()
        .find(|grant| grant.alias == policy.primary_alias())
        .ok_or(CapabilityError::Authority)?;
    match &step.action {
        StepAction::Agent(action) => {
            let primary_access = action.candidate_authority.access();
            if (primary_access.is_writable() && !primary.access.is_writable())
                || !action.authority.allowed_by(
                    tools,
                    policy
                        .grants()
                        .iter()
                        .map(|grant| (grant.alias.as_str(), grant.access)),
                )
            {
                return Err(CapabilityError::Authority);
            }
            let mut directories = vec![CapabilityDirectory {
                alias: policy.primary_alias().to_owned(),
                guest_path: guest_path_for(policy.primary_alias(), policy.primary_alias()),
                access: primary_access,
                role: DirectoryRole::PrimarySource,
            }];
            for directory in &action.authority.directories {
                if directory.alias == policy.primary_alias() || directory.access.is_writable() {
                    return Err(CapabilityError::Authority);
                }
                let Some(grant) = policy
                    .grants()
                    .iter()
                    .find(|grant| grant.alias == directory.alias)
                else {
                    return Err(CapabilityError::Authority);
                };
                directories.push(CapabilityDirectory {
                    alias: directory.alias.clone(),
                    guest_path: guest_path_for(&directory.alias, policy.primary_alias()),
                    access: min_access(AccessMode::ReadOnly, grant.access),
                    role: DirectoryRole::SecondaryContext,
                });
            }
            Ok(AttemptCapabilities {
                schema: CAPABILITY_SCHEMA,
                agent_revision: revision,
                tools: action.authority.tools.clone(),
                directories,
                source_location: PrimarySourceLocation::AttemptWorkspace,
                git_admin: AccessMode::ReadOnly,
                network: NetworkCapability::from_agent(network),
            })
        }
        StepAction::SystemCommand(action) => {
            if matches!(
                action.command.contract().source_effect,
                CommandSourceEffect::Apply | CommandSourceEffect::Commit
            ) && !primary.access.is_writable()
            {
                return Err(CapabilityError::Authority);
            }
            Ok(commit_or_read_only(
                action.command,
                revision,
                policy.primary_alias(),
            ))
        }
        StepAction::HumanGate(_) => Err(CapabilityError::Authority),
    }
}

fn commit_or_read_only(
    command: SystemCommandId,
    revision: u32,
    primary_alias: &str,
) -> AttemptCapabilities {
    if matches!(
        command.contract().source_effect,
        CommandSourceEffect::Apply | CommandSourceEffect::Commit
    ) {
        AttemptCapabilities {
            schema: CAPABILITY_SCHEMA,
            agent_revision: revision,
            tools: Vec::new(),
            directories: vec![CapabilityDirectory {
                alias: primary_alias.to_owned(),
                guest_path: guest_path_for(primary_alias, primary_alias),
                access: AccessMode::ReadWrite,
                role: DirectoryRole::PrimarySource,
            }],
            source_location: PrimarySourceLocation::UserProject,
            git_admin: if command.contract().source_effect == CommandSourceEffect::Commit {
                AccessMode::ReadWrite
            } else {
                AccessMode::ReadOnly
            },
            network: NetworkCapability::None,
        }
    } else {
        AttemptCapabilities {
            schema: CAPABILITY_SCHEMA,
            agent_revision: revision,
            tools: Vec::new(),
            directories: vec![primary_read_only(primary_alias)],
            source_location: PrimarySourceLocation::AttemptWorkspace,
            git_admin: AccessMode::ReadOnly,
            network: NetworkCapability::None,
        }
    }
}

fn apply_capabilities(revision: u32, aliases: &[String]) -> AttemptCapabilities {
    AttemptCapabilities {
        schema: CAPABILITY_SCHEMA,
        agent_revision: revision,
        tools: Vec::new(),
        directories: aliases
            .iter()
            .enumerate()
            .map(|(index, alias)| CapabilityDirectory {
                alias: alias.clone(),
                guest_path: format!("/access/{alias}"),
                access: AccessMode::ReadWrite,
                role: if index == 0 {
                    DirectoryRole::PrimarySource
                } else {
                    DirectoryRole::SecondaryContext
                },
            })
            .collect(),
        source_location: PrimarySourceLocation::UserProject,
        git_admin: AccessMode::ReadOnly,
        network: NetworkCapability::None,
    }
}

fn primary_read_only(primary_alias: &str) -> CapabilityDirectory {
    CapabilityDirectory {
        alias: primary_alias.to_owned(),
        guest_path: guest_path_for(primary_alias, primary_alias),
        access: AccessMode::ReadOnly,
        role: DirectoryRole::PrimarySource,
    }
}

fn min_access(left: AccessMode, right: AccessMode) -> AccessMode {
    if left.is_writable() && right.is_writable() {
        AccessMode::ReadWrite
    } else {
        AccessMode::ReadOnly
    }
}

impl std::fmt::Display for CapabilityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl CapabilityError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Authority => "The pinned step authority exceeds the current agent ceiling.",
        }
    }
}

#[cfg(test)]
pub(in crate::workflows) mod tests;
