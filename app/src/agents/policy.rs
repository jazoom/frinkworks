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

/// A read-only skill root outside the directory grants. Power Plant owns the
/// global skills directory and mounts it into each sandbox. The root is not
/// part of the directory authority, so policy equality ignores it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SkillRoot {
    pub(crate) scope: String,
    pub(crate) guest_path: String,
    pub(crate) host_path: PathBuf,
}

#[derive(Clone, Debug, Eq)]
pub(crate) struct DirectoryPolicy {
    grants: Vec<PolicyGrant>,
    primary_alias: String,
    private_workspace: bool,
    skill_root: Option<SkillRoot>,
    host_directory: Option<String>,
}

impl PartialEq for DirectoryPolicy {
    fn eq(&self, other: &Self) -> bool {
        self.grants == other.grants
            && self.primary_alias == other.primary_alias
            && self.private_workspace == other.private_workspace
            && self.host_directory == other.host_directory
    }
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
            skill_root: None,
            host_directory: None,
        }
    }

    pub(crate) fn from_grants(grants: Vec<PolicyGrant>, primary_alias: String) -> Self {
        Self {
            grants,
            primary_alias,
            private_workspace: false,
            skill_root: None,
            host_directory: None,
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
            skill_root: None,
            host_directory: None,
        }
    }

    pub(crate) fn with_skill_root(mut self, skill_root: Option<SkillRoot>) -> Self {
        self.skill_root = skill_root;
        self
    }

    // Host consent authorises process-user access. Saved sandbox grants do not confine host tools.
    pub(crate) fn on_host(mut self, directory: &std::path::Path) -> Self {
        self.private_workspace = false;
        self.host_directory = Some(directory.to_string_lossy().into_owned());
        for grant in &mut self.grants {
            grant.guest_path = grant.host_path.to_string_lossy().into_owned();
            grant.access = AccessMode::ReadWrite;
        }
        if let Some(root) = &mut self.skill_root {
            root.guest_path = root.host_path.to_string_lossy().into_owned();
        }
        self
    }

    pub(crate) fn host_directory(&self) -> Option<&str> {
        self.host_directory.as_deref()
    }

    pub(crate) fn skill_root(&self) -> Option<&SkillRoot> {
        self.skill_root.as_ref()
    }

    pub(crate) fn grants(&self) -> &[PolicyGrant] {
        &self.grants
    }

    pub(crate) fn primary_alias(&self) -> &str {
        &self.primary_alias
    }

    pub(crate) fn primary_guest(&self) -> &str {
        if let Some(directory) = &self.host_directory {
            return directory;
        }
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
        if self.host_directory.is_some() {
            return AccessMode::ReadWrite;
        }
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
        if let Some(root) = &self.skill_root
            && (normalised == root.guest_path
                || normalised.starts_with(&format!("{}/", root.guest_path)))
        {
            return Ok((normalised, AccessMode::ReadOnly));
        }
        if self.host_directory.is_some() {
            return Ok((normalised, AccessMode::ReadWrite));
        }
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
        if self.host_directory.is_some() {
            return vec!["/".to_owned()];
        }
        let mut roots = self
            .private_workspace
            .then(|| GUEST_WORKSPACE.to_owned())
            .into_iter()
            .collect::<Vec<_>>();
        roots.extend(self.grants.iter().map(|grant| grant.guest_path.clone()));
        if let Some(root) = &self.skill_root {
            roots.push(root.guest_path.clone());
        }
        roots
    }

    /// The alias that owns a guest path. Global skills use their own scope.
    pub(crate) fn resource_scope(&self, path: &str) -> Option<String> {
        if let Some(root) = &self.skill_root
            && (path == root.guest_path || path.starts_with(&format!("{}/", root.guest_path)))
        {
            return Some(root.scope.clone());
        }
        self.grant_for(path)
            .map(|grant| grant.alias.clone())
            .or_else(|| self.host_directory.as_ref().map(|_| "host".to_owned()))
    }

    pub(crate) fn writable_roots(&self) -> Vec<String> {
        if self.host_directory.is_some() {
            return vec!["/".to_owned()];
        }
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
