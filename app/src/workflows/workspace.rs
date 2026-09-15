use std::path::PathBuf;

use super::id::{AttemptId, RunId};
use crate::storage::{self, PersistError};

pub(crate) struct WorkflowWorkspaces {
    root: PathBuf,
    #[cfg(test)]
    _hold: Option<tempfile::TempDir>,
}

pub(crate) struct AttemptWorkspace {
    pub(crate) root: PathBuf,
    pub(crate) project: PathBuf,
}

#[derive(Debug)]
pub(crate) struct WorkspaceCreateError {
    pub(crate) orphaned: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WorkspaceRecovery {
    pub(crate) run: RunId,
    pub(crate) attempt: AttemptId,
    pub(crate) remains: bool,
}

impl WorkflowWorkspaces {
    pub(crate) fn open(root: PathBuf) -> Result<Self, PersistError> {
        storage::ensure_private_dir(&root)?;
        Ok(Self {
            root,
            #[cfg(test)]
            _hold: None,
        })
    }

    pub(crate) fn create_attempt(
        &self,
        run: RunId,
        attempt: AttemptId,
    ) -> Result<AttemptWorkspace, WorkspaceCreateError> {
        let workspace = self
            .attempt_dir(run, attempt)
            .map_err(|_| WorkspaceCreateError { orphaned: false })?;
        let create = || -> Result<PathBuf, PersistError> {
            storage::ensure_private_dir(&workspace)?;
            let project = storage::confined_child(&workspace, "project")?;
            storage::ensure_private_dir(&project)?;
            Ok(project)
        };
        match create() {
            Ok(project) => Ok(AttemptWorkspace {
                root: workspace,
                project,
            }),
            Err(_) => {
                let orphaned = storage::remove_tree_nofollow(&workspace).is_err();
                Err(WorkspaceCreateError { orphaned })
            }
        }
    }

    pub(crate) fn attempt_dir(
        &self,
        run: RunId,
        attempt: AttemptId,
    ) -> Result<PathBuf, PersistError> {
        let run_dir = storage::confined_child(&self.root, &run.as_hex())?;
        storage::confined_child(&run_dir, &attempt.as_hex())
    }

    pub(crate) fn recover_leftovers(
        &self,
        retain: impl Fn(&RunId, &AttemptId) -> bool,
        guest_present: impl Fn(&RunId, &AttemptId) -> bool,
    ) -> Result<Vec<WorkspaceRecovery>, PersistError> {
        let mut recovered = Vec::new();
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(recovered),
            Err(_) => return Err(PersistError),
        };
        for entry in entries {
            let entry = entry.map_err(|_| PersistError)?;
            let Some(run) = entry.file_name().to_str().and_then(RunId::parse) else {
                continue;
            };
            let run_dir = entry.path();
            let attempts = match std::fs::read_dir(&run_dir) {
                Ok(attempts) => attempts,
                Err(_) => continue,
            };
            let mut remaining = 0usize;
            for attempt_entry in attempts {
                let attempt_entry = attempt_entry.map_err(|_| PersistError)?;
                let Some(attempt) = attempt_entry
                    .file_name()
                    .to_str()
                    .and_then(AttemptId::parse)
                else {
                    remaining += 1;
                    continue;
                };
                if retain(&run, &attempt) {
                    remaining += 1;
                    continue;
                }
                if guest_present(&run, &attempt) {
                    recovered.push(WorkspaceRecovery {
                        run,
                        attempt,
                        remains: true,
                    });
                    remaining += 1;
                    continue;
                }
                let remains = storage::remove_tree_nofollow(&attempt_entry.path()).is_err();
                recovered.push(WorkspaceRecovery {
                    run,
                    attempt,
                    remains,
                });
                if remains {
                    remaining += 1;
                }
            }
            if remaining == 0 {
                let _ = std::fs::remove_dir(&run_dir);
            }
        }
        Ok(recovered)
    }
}

pub(crate) fn reviewed_capture_exclusions(
    root: &std::path::Path,
    data_root: &std::path::Path,
) -> Vec<String> {
    const ENGINE_PATHS: &[&str] = &[
        "workflow-apply-journals",
        "workflow-artefacts",
        "workflow-commit-journals",
        "workflow-evidence",
        "workflow-runs",
        "workflow-workspaces",
    ];
    let mut exclusions = ENGINE_PATHS
        .iter()
        .filter_map(|name| {
            let path = data_root.join(name);
            // A root inside engine storage has no safe capture boundary.
            if root.starts_with(&path) {
                return Some(".".to_owned());
            }
            path.strip_prefix(root)
                .ok()
                .filter(|relative| !relative.as_os_str().is_empty())
                .and_then(|relative| relative.to_str())
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    exclusions.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    exclusions
}

impl AttemptWorkspace {
    pub(crate) fn reviewed_root(&self, alias: &str) -> Result<PathBuf, PersistError> {
        let roots = storage::confined_child(&self.root, "reviewed")?;
        storage::ensure_private_dir(&roots)?;
        let root = storage::confined_child(&roots, alias)?;
        storage::ensure_private_dir(&root)?;
        Ok(root)
    }

    pub(crate) fn destroy(self) -> Result<(), PersistError> {
        storage::remove_tree_nofollow(&self.root)
    }
}

#[cfg(test)]
mod tests;
