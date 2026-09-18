use std::path::PathBuf;

use super::record::{
    AccessMode, AgentError, AgentRecord, GUEST_PROJECT, NetworkAccess, canonical_directory,
    guest_path_for,
};
use super::tool_id::ToolId;
use crate::execution::GUEST_WORKSPACE;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PolicyGrant {
    pub(crate) alias: String,
    pub(crate) guest_path: String,
    pub(crate) host_path: PathBuf,
    pub(crate) access: AccessMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DirectoryPolicy {
    grants: Vec<PolicyGrant>,
    primary_alias: String,
    private_workspace: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EffectiveAuthority {
    pub(crate) revision: u32,
    pub(crate) grant_alias: String,
    pub(crate) grant_access: AccessMode,
    pub(crate) tools: Vec<ToolId>,
    pub(crate) network: NetworkAccess,
    pub(crate) policy: DirectoryPolicy,
}

impl EffectiveAuthority {
    pub(crate) fn directories(&self) -> impl Iterator<Item = (&str, AccessMode)> {
        self.policy
            .grants()
            .iter()
            .map(|grant| (grant.alias.as_str(), grant.access))
    }
}

impl DirectoryPolicy {
    pub(crate) fn is_private_workspace(&self) -> bool {
        self.private_workspace
    }

    // The selected project grant is /project even when another grant is the saved primary.
    pub(crate) fn from_record_with_primary(record: &AgentRecord, primary_alias: &str) -> Self {
        let grants = record
            .directories
            .iter()
            .map(|grant| PolicyGrant {
                alias: grant.alias.clone(),
                guest_path: guest_path_for(&grant.alias, primary_alias),
                host_path: grant.host_path.clone(),
                access: grant.access,
            })
            .collect();
        Self {
            grants,
            primary_alias: primary_alias.to_owned(),
            private_workspace: false,
        }
    }

    pub(crate) fn from_grants(grants: Vec<PolicyGrant>, primary_alias: String) -> Self {
        Self {
            grants,
            primary_alias,
            private_workspace: false,
        }
    }

    pub(crate) fn from_grants_with_workspace(
        grants: Vec<PolicyGrant>,
        primary_alias: String,
    ) -> Self {
        Self {
            grants,
            primary_alias,
            private_workspace: true,
        }
    }

    pub(crate) fn grants(&self) -> &[PolicyGrant] {
        &self.grants
    }

    pub(crate) fn primary_alias(&self) -> &str {
        &self.primary_alias
    }

    pub(crate) fn primary_guest(&self) -> &str {
        self.grants
            .iter()
            .find(|grant| grant.alias == self.primary_alias)
            .map(|grant| grant.guest_path.as_str())
            .unwrap_or(if self.grants.is_empty() {
                GUEST_WORKSPACE
            } else {
                GUEST_PROJECT
            })
    }

    pub(crate) fn primary_access(&self) -> AccessMode {
        self.grants
            .iter()
            .find(|grant| grant.alias == self.primary_alias)
            .map(|grant| grant.access)
            .unwrap_or(if self.grants.is_empty() {
                AccessMode::ReadWrite
            } else {
                AccessMode::ReadOnly
            })
    }

    pub(crate) fn resolve(&self, raw: &str) -> Result<(String, AccessMode), &'static str> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Ok((self.primary_guest().to_owned(), self.primary_access()));
        }
        if raw.chars().any(char::is_control) {
            return Err("That path is not valid.");
        }
        let joined = if raw.starts_with('/') {
            raw.to_owned()
        } else {
            format!("{}/{raw}", self.primary_guest())
        };
        let normalised = normalise_absolute(&joined)?;
        if self.private_workspace
            && (normalised == GUEST_WORKSPACE
                || normalised.starts_with(&format!("{GUEST_WORKSPACE}/")))
        {
            return Ok((normalised, AccessMode::ReadWrite));
        }
        self.grant_for(&normalised)
            .map(|grant| (normalised, grant.access))
            .ok_or(if self.grants.is_empty() {
                "Stay inside the private workspace."
            } else {
                "Stay inside a granted directory."
            })
    }

    pub(crate) fn guest_roots(&self) -> Vec<String> {
        let mut roots = self
            .private_workspace
            .then(|| GUEST_WORKSPACE.to_owned())
            .into_iter()
            .collect::<Vec<_>>();
        roots.extend(self.grants.iter().map(|grant| grant.guest_path.clone()));
        roots
    }

    pub(crate) fn writable_roots(&self) -> Vec<String> {
        let mut roots = self
            .private_workspace
            .then(|| GUEST_WORKSPACE.to_owned())
            .into_iter()
            .collect::<Vec<_>>();
        roots.extend(
            self.grants
                .iter()
                .filter(|grant| grant.access.is_writable())
                .map(|grant| grant.guest_path.clone()),
        );
        roots
    }

    pub(crate) fn confirm_hosts(&self) -> Result<(), AgentError> {
        for grant in &self.grants {
            let resolved = canonical_directory(&grant.host_path)?;
            if resolved != grant.host_path {
                return Err(AgentError::Path);
            }
        }
        Ok(())
    }

    fn grant_for(&self, path: &str) -> Option<&PolicyGrant> {
        self.grants.iter().find(|grant| {
            path == grant.guest_path || path.starts_with(&format!("{}/", grant.guest_path))
        })
    }
}

fn normalise_absolute(path: &str) -> Result<String, &'static str> {
    let mut parts = Vec::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            if parts.is_empty() {
                return Err("Stay inside a granted directory.");
            }
            parts.pop();
            continue;
        }
        parts.push(part);
    }
    Ok(format!("/{}", parts.join("/")))
}

#[cfg(test)]
mod tests;
