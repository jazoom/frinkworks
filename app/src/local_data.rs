//! Private ownership of the process data root and restart-applied local reset.
//!
//! The owned root is the only deletion target. Reset never takes a path from a
//! query, form, cookie, or route parameter.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::agents::{AgentRecord, AgentStore};
use crate::config::StartupConfig;
use crate::conversations::{ConversationMetadata, ConversationStore};
use crate::storage::{self, PersistError};
use crate::workflows::{ExecutionGuard, WorkflowExecution};

const OWNERSHIP_MARKER_NAME: &str = ".powerplant-data-root";
const OWNERSHIP_CONTENTS: &[u8] = b"powerplant-data-root-v1\n";
const RESET_MARKER_NAME: &str = ".powerplant-reset";
const RESET_CONTENTS: &[u8] = b"powerplant-reset-v1\n";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResetRequest {
    Recorded,
    Pending,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CatalogueResetConflict {
    AgentGrant,
    ConversationGrant,
    PresetGrant,
    WorkflowGrant,
    DefaultDirectory,
}

pub(crate) struct ResetCatalogues<'a> {
    pub(crate) agents: &'a AgentStore,
    pub(crate) conversations: &'a ConversationStore,
    pub(crate) presets: &'a crate::presets::PresetStore,
    pub(crate) workflows: &'a crate::workflows::WorkflowCatalogue,
    pub(crate) preferences: &'a crate::preferences::Preferences,
}

#[derive(Debug)]
pub(crate) enum ResetError {
    WorkflowBusy,
    Catalogue(CatalogueResetConflict),
    Persist(PersistError),
}

impl CatalogueResetConflict {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::AgentGrant => "An agent grant is inside the Power Plant data directory.",
            Self::ConversationGrant => {
                "A conversation grant is inside the Power Plant data directory."
            }
            Self::PresetGrant => "A preset grant is inside the Power Plant data directory.",
            Self::DefaultDirectory => {
                "A directory in future defaults is inside the Power Plant data directory."
            }
            Self::WorkflowGrant => {
                "A workflow directory setting is inside the Power Plant data directory."
            }
        }
    }
}

pub(crate) const HOST_PATH_RESET_PENDING: &str =
    "Power Plant is waiting to reset local data. Stop and restart Power Plant.";

#[derive(Clone)]
pub(crate) struct LocalDataReset {
    root: PathBuf,
    inner: Arc<Mutex<Inner>>,
    mutation: Arc<tokio::sync::Mutex<()>>,
}

struct Inner {
    pending: bool,
    execution: Option<ExecutionGuard>,
}

pub(crate) struct HostPathPermit {
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

#[derive(Clone, Copy)]
enum PrepareError {
    ConfiguredPath,
    UnusableDirectory,
    UnownedRoot,
    Prepare,
    Reset,
}

impl PrepareError {
    fn message(self) -> &'static str {
        match self {
            Self::ConfiguredPath => {
                "POWERPLANT_DATA_DIR must name a directory with a final path component."
            }
            Self::UnusableDirectory => "The Power Plant data directory is not a usable directory.",
            Self::UnownedRoot => "The Power Plant data directory is not a private owned root.",
            Self::Prepare => "Power Plant could not prepare the data directory.",
            Self::Reset => "Power Plant could not reset local data.",
        }
    }
}

pub(crate) fn prepare(
    mut config: StartupConfig,
) -> Result<(StartupConfig, LocalDataReset), String> {
    let root = establish(&config).map_err(|error| error.message().to_owned())?;
    config.data_dir = root.clone();
    Ok((config, LocalDataReset::new(root)))
}

impl LocalDataReset {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            inner: Arc::new(Mutex::new(Inner {
                pending: false,
                execution: None,
            })),
            mutation: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn is_pending(&self) -> bool {
        lock(&self.inner).pending
    }

    async fn lock_host_paths(&self) -> HostPathPermit {
        HostPathPermit {
            _guard: self.mutation.clone().lock_owned().await,
        }
    }

    pub(crate) async fn begin_host_path_mutation(&self) -> Result<HostPathPermit, ()> {
        let permit = self.lock_host_paths().await;
        if self.is_pending() {
            Err(())
        } else {
            Ok(permit)
        }
    }

    pub(crate) async fn request_reset(
        &self,
        workflow_execution: &Arc<WorkflowExecution>,
        catalogues: ResetCatalogues<'_>,
    ) -> Result<ResetRequest, ResetError> {
        if self.is_pending() {
            return Ok(ResetRequest::Pending);
        }
        let execution = match workflow_execution.acquire_exclusive() {
            Ok(execution) => execution,
            Err(()) if self.is_pending() => return Ok(ResetRequest::Pending),
            Err(()) => return Err(ResetError::WorkflowBusy),
        };
        let _permit = self.lock_host_paths().await;
        if self.is_pending() {
            return Ok(ResetRequest::Pending);
        }
        if let Some(conflict) = self.catalogue_conflict(
            &catalogues.agents.list(),
            &catalogues
                .conversations
                .try_metadata()
                .map_err(|_| ResetError::Persist(PersistError))?,
            &catalogues.presets.list(),
            &catalogues.workflows.list(),
        ) {
            return Err(ResetError::Catalogue(conflict));
        }
        if catalogues
            .preferences
            .conversation_defaults()
            .is_some_and(|settings| {
                settings
                    .directories
                    .iter()
                    .any(|grant| path_under_root(&self.root, &grant.host_path))
            })
        {
            return Err(ResetError::Catalogue(
                CatalogueResetConflict::DefaultDirectory,
            ));
        }
        let mut inner = lock(&self.inner);
        let result = write_reset_marker(&self.root, &mut inner).map_err(ResetError::Persist)?;
        inner.execution = Some(execution);
        Ok(result)
    }

    fn catalogue_conflict(
        &self,
        agents: &[AgentRecord],
        conversations: &[ConversationMetadata],
        presets: &[crate::presets::PresetRecord],
        workflows: &[crate::workflows::WorkflowRecord],
    ) -> Option<CatalogueResetConflict> {
        if agents.iter().any(|agent| {
            agent
                .directories
                .iter()
                .any(|grant| path_under_root(&self.root, &grant.host_path))
        }) {
            return Some(CatalogueResetConflict::AgentGrant);
        }
        if conversations.iter().any(|conversation| {
            conversation.model.as_ref().is_some_and(|model| {
                model
                    .settings
                    .directories
                    .iter()
                    .any(|grant| path_under_root(&self.root, &grant.host_path))
            })
        }) {
            return Some(CatalogueResetConflict::ConversationGrant);
        }
        if presets.iter().any(|preset| {
            preset
                .settings
                .directories
                .iter()
                .any(|grant| path_under_root(&self.root, &grant.host_path))
        }) {
            return Some(CatalogueResetConflict::PresetGrant);
        }
        if workflows.iter().any(|workflow| {
            workflow
                .definition
                .directory_grants()
                .any(|grant| path_under_root(&self.root, &grant.host_path))
        }) {
            return Some(CatalogueResetConflict::WorkflowGrant);
        }
        None
    }

    #[cfg(test)]
    fn record_reset_for_test(&self) -> Result<ResetRequest, PersistError> {
        let mut inner = lock(&self.inner);
        write_reset_marker(&self.root, &mut inner)
    }
}

fn write_reset_marker(root: &Path, inner: &mut Inner) -> Result<ResetRequest, PersistError> {
    if inner.pending {
        return Ok(ResetRequest::Pending);
    }
    let owned = marker_is_exact(root, OWNERSHIP_MARKER_NAME, OWNERSHIP_CONTENTS)
        .map_err(|_| PersistError)?;
    if !owned {
        return Err(PersistError);
    }
    if marker_is_exact(root, RESET_MARKER_NAME, RESET_CONTENTS).map_err(|_| PersistError)? {
        inner.pending = true;
        return Ok(ResetRequest::Pending);
    }
    let path = storage::confined_child(root, RESET_MARKER_NAME)?;
    storage::write_private(&path, RESET_CONTENTS)?;
    inner.pending = true;
    Ok(ResetRequest::Recorded)
}

fn path_under_root(root: &Path, path: &Path) -> bool {
    path.starts_with(root) || root.starts_with(path)
}

fn establish(config: &StartupConfig) -> Result<PathBuf, PrepareError> {
    let configured = &config.data_dir;
    if !usable_configured_path(configured) {
        return Err(PrepareError::ConfiguredPath);
    }
    let protected = expand_protected(&config.protected_user_roots);
    reject_protected(configured, &protected)?;
    ensure_directory(configured)?;
    let root = canonicalize_directory(configured)?;
    if !usable_configured_path(&root) {
        return Err(PrepareError::ConfiguredPath);
    }
    reject_protected(&root, &protected)?;
    let owned = marker_is_exact(&root, OWNERSHIP_MARKER_NAME, OWNERSHIP_CONTENTS)?;
    if !owned {
        ensure_claimable(&root)?;
    }
    storage::ensure_private_dir(&root).map_err(|_| PrepareError::Prepare)?;
    if !owned {
        write_ownership_marker(&root).map_err(|_| PrepareError::Prepare)?;
    }
    if marker_is_exact(&root, RESET_MARKER_NAME, RESET_CONTENTS)? {
        return apply_reset(&root);
    }
    Ok(root)
}

fn usable_configured_path(path: &Path) -> bool {
    path.is_absolute() && path.file_name().is_some()
}

fn expand_protected(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut expanded = Vec::with_capacity(roots.len().saturating_mul(2));
    for root in roots {
        if !root.is_absolute() {
            continue;
        }
        if let Ok(canonical) = fs::canonicalize(root)
            && !expanded.contains(&canonical)
        {
            expanded.push(canonical);
        }
        if !expanded.contains(root) {
            expanded.push(root.clone());
        }
    }
    expanded
}

// Deleting this root would remove a user home or profile directory.
fn reject_protected(data_root: &Path, protected: &[PathBuf]) -> Result<(), PrepareError> {
    if protected
        .iter()
        .any(|root| root.is_absolute() && root.starts_with(data_root))
    {
        Err(PrepareError::UnownedRoot)
    } else {
        Ok(())
    }
}

fn ensure_directory(path: &Path) -> Result<(), PrepareError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(PrepareError::UnusableDirectory)
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|_| PrepareError::Prepare)?;
            match fs::symlink_metadata(path) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                    Err(PrepareError::UnusableDirectory)
                }
                Ok(_) => Ok(()),
                Err(_) => Err(PrepareError::Prepare),
            }
        }
        Err(_) => Err(PrepareError::Prepare),
    }
}

fn canonicalize_directory(path: &Path) -> Result<PathBuf, PrepareError> {
    let canonical = fs::canonicalize(path).map_err(|_| PrepareError::Prepare)?;
    match fs::symlink_metadata(&canonical) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(PrepareError::UnusableDirectory)
        }
        Ok(_) => Ok(canonical),
        Err(_) => Err(PrepareError::Prepare),
    }
}

fn marker_is_exact(root: &Path, name: &str, expected: &[u8]) -> Result<bool, PrepareError> {
    let path = storage::confined_child(root, name).map_err(|_| PrepareError::UnownedRoot)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(PrepareError::UnownedRoot),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(PrepareError::UnownedRoot)
        }
        Ok(_) => {
            let bytes = storage::read_private_bounded(&path, expected.len())
                .map_err(|_| PrepareError::UnownedRoot)?;
            if bytes == expected {
                Ok(true)
            } else {
                Err(PrepareError::UnownedRoot)
            }
        }
    }
}

fn ensure_claimable(root: &Path) -> Result<(), PrepareError> {
    let mut entries = fs::read_dir(root).map_err(|_| PrepareError::Prepare)?;
    match entries.next() {
        None => Ok(()),
        Some(Ok(_)) => Err(PrepareError::UnownedRoot),
        Some(Err(_)) => Err(PrepareError::Prepare),
    }
}

fn write_ownership_marker(root: &Path) -> Result<(), PersistError> {
    let path = storage::confined_child(root, OWNERSHIP_MARKER_NAME)?;
    storage::write_private(&path, OWNERSHIP_CONTENTS)
}

fn apply_reset(root: &Path) -> Result<PathBuf, PrepareError> {
    storage::remove_tree_nofollow(root).map_err(|_| PrepareError::Reset)?;
    storage::ensure_private_dir(root).map_err(|_| PrepareError::Reset)?;
    let root = canonicalize_directory(root).map_err(|_| PrepareError::Reset)?;
    if !directory_is_empty(&root)? {
        return Err(PrepareError::Reset);
    }
    write_ownership_marker(&root).map_err(|_| PrepareError::Reset)?;
    Ok(root)
}

fn directory_is_empty(root: &Path) -> Result<bool, PrepareError> {
    let mut entries = fs::read_dir(root).map_err(|_| PrepareError::Reset)?;
    match entries.next() {
        None => Ok(true),
        Some(Ok(_)) => Ok(false),
        Some(Err(_)) => Err(PrepareError::Reset),
    }
}

fn lock(mutex: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests;
