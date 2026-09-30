#[cfg(test)]
mod tests;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::execution::{DirectoryAccess, DirectoryGrant, DirectoryGrantId, validate_directories};

use super::{PreferenceError, Preferences};

const MAXIMUM_RECENT_DIRECTORIES: usize = 10;
const MAXIMUM_APPROVED_DIRECTORIES: usize = 1_024;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecentDirectory {
    pub(crate) id: String,
    pub(crate) host_path: PathBuf,
    pub(crate) access: DirectoryAccess,
}

impl RecentDirectory {
    pub(crate) fn grant(&self) -> Option<DirectoryGrant> {
        Some(DirectoryGrant {
            id: DirectoryGrantId::parse(&self.id)?,
            host_path: self.host_path.clone(),
            alias: "recent".to_owned(),
            access: self.access,
        })
    }
}

impl Preferences {
    pub(crate) fn recent_directories(&self) -> Vec<RecentDirectory> {
        self.values().recent_directories
    }

    pub(crate) fn remember_directory(
        &self,
        grant: &DirectoryGrant,
        approved: bool,
    ) -> Result<(), PreferenceError> {
        self.store_directory(grant, approved, true)
    }

    pub(crate) fn update_directory_access(
        &self,
        grant: &DirectoryGrant,
        approved: bool,
    ) -> Result<(), PreferenceError> {
        self.store_directory(grant, approved, false)
    }

    fn store_directory(
        &self,
        grant: &DirectoryGrant,
        approved: bool,
        remember: bool,
    ) -> Result<(), PreferenceError> {
        grant.revalidate().map_err(|_| PreferenceError)?;
        self.update(|values| {
            if let Some(entry) = values
                .recent_directories
                .iter_mut()
                .find(|entry| entry.host_path == grant.host_path)
            {
                entry.access = grant.access;
            } else if remember {
                values.recent_directories.insert(
                    0,
                    RecentDirectory {
                        id: grant.id.as_hex(),
                        host_path: grant.host_path.clone(),
                        access: grant.access,
                    },
                );
                values
                    .recent_directories
                    .truncate(MAXIMUM_RECENT_DIRECTORIES);
            }
            if approved && !values.approved_directories.contains(&grant.host_path) {
                values.approved_directories.push(grant.host_path.clone());
            }
        })
    }

    pub(crate) fn forget_directory(&self, id: &str) -> Result<(), PreferenceError> {
        self.update(|values| values.recent_directories.retain(|entry| entry.id != id))
    }

    // Saved consent covers exactly this canonical path, never its parent or a redirected link.
    pub(crate) fn directory_approved(&self, grant: &DirectoryGrant) -> bool {
        self.values()
            .approved_directories
            .contains(&grant.host_path)
            && grant.revalidate().is_ok()
    }
}

pub(super) fn valid_history(recent: &[RecentDirectory], approved: &[PathBuf]) -> bool {
    recent.len() <= MAXIMUM_RECENT_DIRECTORIES
        && approved.len() <= MAXIMUM_APPROVED_DIRECTORIES
        && recent.iter().enumerate().all(|(index, entry)| {
            entry
                .grant()
                .is_some_and(|grant| validate_directories(&[grant]).is_ok())
                && !recent[..index].iter().any(|previous| {
                    previous.id == entry.id || previous.host_path == entry.host_path
                })
        })
        && approved.iter().enumerate().all(|(index, path)| {
            let entry = RecentDirectory {
                id: "00000000000000000000000000000000".to_owned(),
                host_path: path.clone(),
                access: DirectoryAccess::Read,
            };
            entry
                .grant()
                .is_some_and(|grant| validate_directories(&[grant]).is_ok())
                && !approved[..index].contains(path)
        })
}
