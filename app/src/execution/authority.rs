use std::path::Path;

use crate::agents::{AccessMode, DirectoryPolicy, NetworkAccess, PolicyGrant, ToolId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectFreeAuthority {
    pub(crate) revision: u32,
    pub(crate) tools: Vec<ToolId>,
    pub(crate) network: NetworkAccess,
    pub(crate) policy: DirectoryPolicy,
}

impl ProjectFreeAuthority {
    pub(crate) fn from_settings(
        revision: u32,
        settings: &super::ExecutionSettings,
    ) -> Result<Self, super::DirectoryGrantError> {
        let authority = Self::from_snapshot(revision, settings)?;
        for grant in &settings.directories {
            grant.revalidate()?;
        }
        Ok(authority)
    }

    // Historical evidence uses saved paths. Dispatch must use from_settings to inspect the host.
    pub(crate) fn from_snapshot(
        revision: u32,
        settings: &super::ExecutionSettings,
    ) -> Result<Self, super::DirectoryGrantError> {
        super::settings::validate_directories(&settings.directories)?;
        if !settings.host_access_allowed() {
            return Err(super::DirectoryGrantError::Invalid);
        }
        let grants = settings
            .directories
            .iter()
            .map(|grant| PolicyGrant {
                alias: grant.alias.clone(),
                guest_path: grant.guest_path(),
                host_path: grant.host_path.clone(),
                access: if grant.access == super::DirectoryAccess::Write {
                    AccessMode::ReadWrite
                } else {
                    AccessMode::ReadOnly
                },
            })
            .collect();
        Ok(Self {
            revision,
            tools: settings.tools.clone(),
            network: settings.network.clone(),
            policy: DirectoryPolicy::from_grants_with_workspace(
                grants,
                settings
                    .directories
                    .first()
                    .map(|grant| grant.alias.clone())
                    .unwrap_or_default(),
            ),
        })
    }

    /// Build preview authority from saved grants without a model selection.
    /// This revalidates every root path and never starts a sandbox. A host
    /// preview keeps host paths; a sandbox preview keeps grant aliases.
    pub(crate) fn from_preview_grants(
        revision: u32,
        grants: &[super::DirectoryGrant],
        location: super::ToolLocation,
    ) -> Result<Self, super::DirectoryGrantError> {
        super::validate_directories(grants)?;
        for grant in grants {
            grant.revalidate()?;
        }
        let policy_grants = grants
            .iter()
            .map(|grant| PolicyGrant {
                alias: grant.alias.clone(),
                guest_path: grant.guest_path(),
                host_path: grant.host_path.clone(),
                access: AccessMode::ReadOnly,
            })
            .collect();
        let primary_alias = grants
            .first()
            .map(|grant| grant.alias.clone())
            .unwrap_or_default();
        let mut policy = DirectoryPolicy::from_grants_with_workspace(policy_grants, primary_alias);
        if location == super::ToolLocation::Host {
            policy = policy.on_host(&super::command_directory(grants));
        }
        Ok(Self {
            revision,
            tools: Vec::new(),
            network: NetworkAccess::None,
            policy,
        })
    }
}

const SENSITIVE_HOME_DIRECTORIES: [&str; 7] = [
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".kube",
    ".config",
    ".password-store",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SensitiveDirectory {
    FrinkworksData,
    Home,
    Credentials,
}

pub(crate) fn classify_sensitive_directory(
    path: &Path,
    data_root: &Path,
) -> Option<SensitiveDirectory> {
    if paths_overlap(path, data_root) {
        return Some(SensitiveDirectory::FrinkworksData);
    }
    let home = std::env::var_os("HOME").and_then(|home| std::fs::canonicalize(home).ok())?;
    classify_sensitive_home_directory(path, &home)
}

fn classify_sensitive_home_directory(path: &Path, home: &Path) -> Option<SensitiveDirectory> {
    if path == home || home.starts_with(path) {
        return Some(SensitiveDirectory::Home);
    }
    SENSITIVE_HOME_DIRECTORIES
        .iter()
        .any(|relative| {
            let root = home.join(relative);
            // An ancestor grant also exposes credentials, including relocated symbolic-link targets.
            paths_overlap(path, &root)
                || std::fs::canonicalize(&root).is_ok_and(|target| paths_overlap(path, &target))
        })
        .then_some(SensitiveDirectory::Credentials)
}

pub(crate) fn sensitive_directory(path: &Path, data_root: &Path) -> bool {
    classify_sensitive_directory(path, data_root).is_some()
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

#[cfg(test)]
mod tests;
