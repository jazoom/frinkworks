use std::path::Path;

use crate::agents::{AccessMode, DirectoryPolicy, NetworkAccess, PolicyGrant, ToolId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectFreeAuthority {
    pub(crate) revision: u32,
    pub(crate) tools: Vec<ToolId>,
    pub(crate) network: NetworkAccess,
    pub(crate) policy: DirectoryPolicy,
    pub(crate) reviewed_aliases: Vec<String>,
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

    // Historical evidence uses saved identities. Dispatch must use from_settings to inspect the host.
    pub(crate) fn from_snapshot(
        revision: u32,
        settings: &super::ExecutionSettings,
    ) -> Result<Self, super::DirectoryGrantError> {
        super::settings::validate_directories(&settings.directories)?;
        let reviewed_aliases = settings
            .directories
            .iter()
            .filter(|grant| grant.access == super::DirectoryAccess::ReviewBeforeApply)
            .map(|grant| grant.alias.clone())
            .collect::<Vec<_>>();
        let grants = settings
            .directories
            .iter()
            .map(|grant| PolicyGrant {
                alias: grant.alias.clone(),
                guest_path: grant.guest_path(),
                host_path: grant.host_path.clone(),
                access: if grant.access == super::DirectoryAccess::DirectWrite {
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
            reviewed_aliases,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SensitiveDirectory {
    PowerPlantData,
    Home,
}

pub(crate) fn classify_sensitive_directory(
    path: &Path,
    data_root: &Path,
) -> Option<SensitiveDirectory> {
    if paths_overlap(path, data_root) {
        return Some(SensitiveDirectory::PowerPlantData);
    }
    std::env::var_os("HOME")
        .and_then(|home| std::fs::canonicalize(home).ok())
        .filter(|home| path == home || home.starts_with(path))
        .map(|_| SensitiveDirectory::Home)
}

pub(crate) fn sensitive_directory(path: &Path, data_root: &Path) -> bool {
    classify_sensitive_directory(path, data_root).is_some()
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

#[cfg(test)]
mod tests;
