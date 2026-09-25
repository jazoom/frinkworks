use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::providers::{
    AuthMethod, ProviderConnection, ProviderKind, SecretString, api_key_is_bounded,
};

#[cfg(test)]
mod tests;

const VAULT_VERSION: u32 = 1;

#[derive(Clone, Default)]
struct VaultState {
    providers: HashMap<ProviderKind, StoredProvider>,
}

#[derive(Clone)]
struct StoredProvider {
    auth: AuthMethod,
    api_key: SecretString,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct VaultFile {
    version: u32,
    providers: Vec<VaultFileProvider>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct VaultFileProvider {
    kind: String,
    auth: String,
    api_key: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VaultError {
    Corrupt,
    Persist,
}

impl VaultError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Corrupt => "The provider vault is unreadable.",
            Self::Persist => "Frinkworks could not store the provider vault. Try again.",
        }
    }
}

impl std::fmt::Display for VaultError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for VaultError {}

pub(crate) struct ProviderVault {
    path: Option<PathBuf>,
    inner: Mutex<VaultState>,
    #[cfg(test)]
    fail_after_next_persist: Mutex<bool>,
    #[cfg(test)]
    fail_next_marker_remove: Mutex<bool>,
}

impl ProviderVault {
    pub(crate) fn open(path: PathBuf) -> Result<Self, VaultError> {
        let state = load(&path)?;
        reconcile(&path, &state)?;
        Ok(Self {
            path: Some(path),
            inner: Mutex::new(state),
            #[cfg(test)]
            fail_after_next_persist: Mutex::new(false),
            #[cfg(test)]
            fail_next_marker_remove: Mutex::new(false),
        })
    }

    pub(crate) fn has_providers(&self) -> bool {
        !self.lock().providers.is_empty()
    }

    pub(crate) fn contains(&self, kind: ProviderKind) -> bool {
        self.lock().providers.contains_key(&kind)
    }

    pub(crate) fn connection_for(
        &self,
        selection: &crate::providers::ModelSelection,
    ) -> Option<ProviderConnection> {
        let state = self.lock();
        let mut connection = connection_from(self.path.as_deref(), &state, selection.provider)?;
        connection.model = selection.model.clone();
        connection.thinking = selection.thinking.clone();
        Some(connection)
    }

    pub(crate) fn providers(&self) -> Vec<(ProviderKind, AuthMethod)> {
        let state = self.lock();
        ProviderKind::ALL
            .into_iter()
            .filter_map(|kind| state.providers.get(&kind).map(|stored| (kind, stored.auth)))
            .collect()
    }

    pub(crate) fn insert_api_key(&self, connection: ProviderConnection) -> Result<(), VaultError> {
        if connection.auth != AuthMethod::ApiKey {
            return Err(VaultError::Persist);
        }
        let mut state = self.lock();
        let plan_path = plan_file_path(self.path.as_deref(), connection.kind);
        let previous_plan = plan_path
            .as_deref()
            .map(read_existing_plan)
            .transpose()?
            .flatten();
        let previous = state.clone();
        state.providers.insert(
            connection.kind,
            StoredProvider {
                auth: AuthMethod::ApiKey,
                api_key: connection.api_key,
            },
        );
        if self.commit(&state).is_err() {
            *state = previous;
            self.commit(&state)?;
            return Err(VaultError::Persist);
        }
        if let Some(plan_path) = plan_path.as_deref()
            && crate::storage::remove_private(plan_path).is_err()
        {
            restore_plan_file(plan_path, previous_plan.as_deref())?;
            *state = previous;
            self.commit(&state)?;
            return Err(VaultError::Persist);
        }
        Ok(())
    }

    pub(crate) fn install_plan(&self, kind: ProviderKind, staged: &Path) -> Result<(), VaultError> {
        if !kind.supports_plan() {
            return Err(VaultError::Persist);
        }
        let final_path = plan_file_path(self.path.as_deref(), kind).ok_or(VaultError::Persist)?;
        let mut state = self.lock();
        let previous = state.clone();
        let previous_plan = stage_existing_plan(&final_path)?;
        if crate::storage::rename_in_dir(staged, &final_path).is_err() {
            return_plan_to_stage_if_moved(&final_path, staged)?;
            restore_plan_backup(&final_path, previous_plan.as_deref())?;
            return Err(VaultError::Persist);
        }
        state.providers.insert(
            kind,
            StoredProvider {
                auth: AuthMethod::Plan,
                api_key: SecretString::new(String::new()),
            },
        );
        if self.commit(&state).is_err() {
            self.rollback_plan_install(
                &mut state,
                previous,
                &final_path,
                staged,
                previous_plan.as_deref(),
            )?;
            return Err(VaultError::Persist);
        }
        if let Some(previous_plan) = previous_plan.as_deref()
            && crate::storage::remove_private(previous_plan).is_err()
        {
            match fs::symlink_metadata(previous_plan) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                _ => {
                    self.rollback_plan_install(
                        &mut state,
                        previous,
                        &final_path,
                        staged,
                        Some(previous_plan),
                    )?;
                    return Err(VaultError::Persist);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn provider_dir(&self) -> Option<PathBuf> {
        Some(self.path.as_ref()?.parent()?.to_path_buf())
    }

    pub(crate) fn forget(&self, kind: ProviderKind) -> Result<(), VaultError> {
        let mut state = self.lock();
        let Some(stored) = state.providers.get(&kind) else {
            return Ok(());
        };
        let previous = state.clone();
        let plan_deletion = if stored.auth == AuthMethod::Plan {
            let final_path =
                plan_file_path(self.path.as_deref(), kind).ok_or(VaultError::Persist)?;
            Some(stage_plan_deletion(final_path)?)
        } else {
            None
        };
        state.providers.remove(&kind);
        if self.commit(&state).is_err() {
            self.rollback_forget(&mut state, previous, plan_deletion.as_ref())?;
            return Err(VaultError::Persist);
        }
        if let Some(deletion) = plan_deletion.as_ref()
            && self.remove_plan_deletion(deletion).is_err()
        {
            self.rollback_forget(&mut state, previous, plan_deletion.as_ref())?;
            return Err(VaultError::Persist);
        }
        Ok(())
    }

    fn rollback_forget(
        &self,
        state: &mut VaultState,
        previous: VaultState,
        plan_deletion: Option<&PlanDeletion>,
    ) -> Result<(), VaultError> {
        *state = previous;
        if let Some(deletion) = plan_deletion {
            restore_plan_deletion(deletion)?;
        }
        self.commit(state)
    }

    fn remove_plan_deletion(&self, deletion: &PlanDeletion) -> Result<(), VaultError> {
        let marker_result =
            crate::storage::remove_private(&deletion.marker_path).map_err(|_| VaultError::Persist);
        #[cfg(test)]
        {
            let mut failure = self
                .fail_next_marker_remove
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if *failure {
                *failure = false;
                marker_result?;
                return Err(VaultError::Persist);
            }
        }
        marker_result?;
        match inspect(&deletion.final_path).map_err(|_| VaultError::Persist)? {
            PathStatus::Absent => Ok(()),
            PathStatus::File => crate::storage::remove_private(&deletion.final_path)
                .map_err(|_| VaultError::Persist),
            PathStatus::Invalid => Err(VaultError::Persist),
        }
    }

    fn rollback_plan_install(
        &self,
        state: &mut VaultState,
        previous: VaultState,
        final_path: &Path,
        staged: &Path,
        previous_plan: Option<&Path>,
    ) -> Result<(), VaultError> {
        *state = previous;
        if crate::storage::rename_in_dir(final_path, staged).is_err() {
            return_plan_to_stage_if_moved(final_path, staged)?;
        }
        restore_plan_backup(final_path, previous_plan)?;
        self.commit(state)
    }

    fn commit(&self, state: &VaultState) -> Result<(), VaultError> {
        let result = persist(self.path.as_deref(), state);
        #[cfg(test)]
        {
            let mut failure = self
                .fail_after_next_persist
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if *failure {
                *failure = false;
                result?;
                return Err(VaultError::Persist);
            }
        }
        result
    }

    fn lock(&self) -> MutexGuard<'_, VaultState> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn connection_from(
    path: Option<&Path>,
    state: &VaultState,
    kind: ProviderKind,
) -> Option<ProviderConnection> {
    state.providers.get(&kind).map(|stored| ProviderConnection {
        kind,
        auth: stored.auth,
        api_key: stored.api_key.clone(),
        model: kind.default_model().to_owned(),
        thinking: None,
        plan_file: (stored.auth == AuthMethod::Plan)
            .then(|| plan_file_path(path, kind))
            .flatten(),
    })
}

fn load(path: &Path) -> Result<VaultState, VaultError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(VaultState::default()),
        Err(_) => return Err(VaultError::Corrupt),
    };
    let file: VaultFile = serde_json::from_slice(&bytes).map_err(|_| VaultError::Corrupt)?;
    if file.version != VAULT_VERSION {
        return Err(VaultError::Corrupt);
    }
    let mut state = VaultState::default();
    for entry in file.providers {
        let Some(kind) = ProviderKind::parse(&entry.kind) else {
            return Err(VaultError::Corrupt);
        };
        if state.providers.contains_key(&kind) {
            return Err(VaultError::Corrupt);
        }
        let Some(auth) = AuthMethod::parse(&entry.auth) else {
            return Err(VaultError::Corrupt);
        };
        let api_key = match auth {
            AuthMethod::ApiKey => {
                if entry.api_key.trim() != entry.api_key || !api_key_is_bounded(&entry.api_key) {
                    return Err(VaultError::Corrupt);
                }
                SecretString::new(entry.api_key)
            }
            AuthMethod::Plan => {
                if !kind.supports_plan() || !entry.api_key.is_empty() {
                    return Err(VaultError::Corrupt);
                }
                SecretString::new(String::new())
            }
        };
        state
            .providers
            .insert(kind, StoredProvider { auth, api_key });
    }
    Ok(state)
}

fn persist(path: Option<&Path>, state: &VaultState) -> Result<(), VaultError> {
    let Some(path) = path else {
        return Ok(());
    };
    if state.providers.is_empty() {
        return crate::storage::remove_private(path).map_err(|_| VaultError::Persist);
    }
    let file = VaultFile {
        version: VAULT_VERSION,
        providers: ProviderKind::ALL
            .into_iter()
            .filter_map(|kind| {
                state.providers.get(&kind).map(|stored| VaultFileProvider {
                    kind: kind.as_str().to_owned(),
                    auth: stored.auth.as_str().to_owned(),
                    api_key: match stored.auth {
                        AuthMethod::ApiKey => stored.api_key.expose().to_owned(),
                        AuthMethod::Plan => String::new(),
                    },
                })
            })
            .collect(),
    };
    let bytes = serde_json::to_vec_pretty(&file).map_err(|_| VaultError::Persist)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| VaultError::Persist)?;
    }
    crate::storage::write_private(path, &bytes).map_err(|_| VaultError::Persist)
}

fn read_existing_plan(final_path: &Path) -> Result<Option<Vec<u8>>, VaultError> {
    match fs::symlink_metadata(final_path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(VaultError::Persist),
        Ok(_) => crate::storage::read_private(final_path)
            .map(Some)
            .map_err(|_| VaultError::Persist),
    }
}

fn stage_existing_plan(final_path: &Path) -> Result<Option<PathBuf>, VaultError> {
    let Some(bytes) = read_existing_plan(final_path)? else {
        return Ok(None);
    };
    let dir = final_path.parent().ok_or(VaultError::Persist)?;
    crate::storage::create_unique_private(dir, &bytes)
        .map(Some)
        .map_err(|_| VaultError::Persist)
}

fn return_plan_to_stage_if_moved(final_path: &Path, staged: &Path) -> Result<(), VaultError> {
    match fs::symlink_metadata(staged) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match fs::symlink_metadata(final_path) {
                Ok(_) => crate::storage::rename_in_dir(final_path, staged)
                    .map_err(|_| VaultError::Persist),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(_) => Err(VaultError::Persist),
            }
        }
        Err(_) => Err(VaultError::Persist),
    }
}

fn restore_plan_backup(final_path: &Path, backup: Option<&Path>) -> Result<(), VaultError> {
    if let Some(backup) = backup {
        crate::storage::rename_in_dir(backup, final_path).map_err(|_| VaultError::Persist)?;
    }
    Ok(())
}

fn restore_plan_file(final_path: &Path, bytes: Option<&[u8]>) -> Result<(), VaultError> {
    if let Some(bytes) = bytes {
        crate::storage::write_private(final_path, bytes).map_err(|_| VaultError::Persist)?;
    }
    Ok(())
}

fn plan_file_path(vault_path: Option<&Path>, kind: ProviderKind) -> Option<PathBuf> {
    let name = kind.plan_file_name()?;
    Some(vault_path?.parent()?.join(name))
}

fn deletion_marker_path(final_path: &Path) -> PathBuf {
    let mut name = final_path.file_name().unwrap_or_default().to_os_string();
    name.push(".deleting");
    final_path.with_file_name(name)
}

struct PlanDeletion {
    final_path: PathBuf,
    marker_path: PathBuf,
    bytes: Vec<u8>,
}

#[derive(Clone, Copy)]
enum PathStatus {
    Absent,
    File,
    Invalid,
}

fn inspect(path: &Path) -> io::Result<PathStatus> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() => Ok(PathStatus::File),
        Ok(_) => Ok(PathStatus::Invalid),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(PathStatus::Absent),
        Err(error) => Err(error),
    }
}

fn reconcile(vault_path: &Path, state: &VaultState) -> Result<(), VaultError> {
    let dir = vault_path.parent().ok_or(VaultError::Corrupt)?;
    let mut restores = Vec::new();
    let mut restricts = Vec::new();
    let mut removals = Vec::new();
    for kind in ProviderKind::ALL {
        let Some(name) = kind.plan_file_name() else {
            continue;
        };
        let final_path = dir.join(name);
        let marker_path = deletion_marker_path(&final_path);
        let named = state
            .providers
            .get(&kind)
            .is_some_and(|stored| stored.auth == AuthMethod::Plan);
        let final_status = inspect(&final_path).map_err(|_| VaultError::Corrupt)?;
        let marker_status = inspect(&marker_path).map_err(|_| VaultError::Corrupt)?;
        match (named, final_status, marker_status) {
            (true, PathStatus::File, PathStatus::Absent) => restricts.push(final_path),
            (true, PathStatus::Absent, PathStatus::File) => {
                restores.push((marker_path, final_path.clone()));
                restricts.push(final_path);
            }
            (false, PathStatus::File, PathStatus::Absent) => removals.push(final_path),
            (false, PathStatus::Absent, PathStatus::File) => removals.push(marker_path),
            (false, PathStatus::Absent, PathStatus::Absent) => {}
            (true, PathStatus::Absent, PathStatus::Absent)
            | (true, PathStatus::File, PathStatus::File)
            | (true, PathStatus::Invalid, _)
            | (true, _, PathStatus::Invalid)
            | (false, PathStatus::File, PathStatus::File)
            | (false, PathStatus::Invalid, _)
            | (false, _, PathStatus::Invalid) => return Err(VaultError::Corrupt),
        }
    }
    removals.extend(abandoned_staged_files(dir)?);
    for (from, to) in restores {
        crate::storage::rename_in_dir(&from, &to).map_err(|_| VaultError::Persist)?;
    }
    for path in restricts {
        crate::storage::restrict_private_file(&path).map_err(|_| VaultError::Persist)?;
    }
    for path in removals {
        crate::storage::remove_private(&path).map_err(|_| VaultError::Persist)?;
    }
    Ok(())
}

fn abandoned_staged_files(dir: &Path) -> Result<Vec<PathBuf>, VaultError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(VaultError::Corrupt),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| VaultError::Corrupt)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with('.') && name.ends_with(".staging") {
            let path = entry.path();
            if !matches!(inspect(&path), Ok(PathStatus::File)) {
                return Err(VaultError::Corrupt);
            }
            paths.push(path);
        }
    }
    Ok(paths)
}

fn stage_plan_deletion(final_path: PathBuf) -> Result<PlanDeletion, VaultError> {
    let marker_path = deletion_marker_path(&final_path);
    match (
        inspect(&final_path).map_err(|_| VaultError::Persist)?,
        inspect(&marker_path).map_err(|_| VaultError::Persist)?,
    ) {
        (PathStatus::File, PathStatus::Absent) => {
            if crate::storage::rename_in_dir(&final_path, &marker_path).is_err() {
                restore_plan_from_marker(&final_path, &marker_path)?;
                return Err(VaultError::Persist);
            }
        }
        (PathStatus::Absent, PathStatus::File) => {}
        (PathStatus::Absent, PathStatus::Absent)
        | (PathStatus::File, PathStatus::File)
        | (PathStatus::Invalid, _)
        | (_, PathStatus::Invalid) => return Err(VaultError::Persist),
    }
    let bytes = match crate::storage::read_private(&marker_path) {
        Ok(bytes) => bytes,
        Err(_) => {
            restore_plan_from_marker(&final_path, &marker_path)?;
            return Err(VaultError::Persist);
        }
    };
    Ok(PlanDeletion {
        final_path,
        marker_path,
        bytes,
    })
}

fn restore_plan_from_marker(final_path: &Path, marker_path: &Path) -> Result<(), VaultError> {
    match (
        inspect(final_path).map_err(|_| VaultError::Persist)?,
        inspect(marker_path).map_err(|_| VaultError::Persist)?,
    ) {
        (PathStatus::Absent, PathStatus::File) => {
            crate::storage::rename_in_dir(marker_path, final_path).map_err(|_| VaultError::Persist)
        }
        (PathStatus::File, PathStatus::Absent) => Ok(()),
        (PathStatus::Absent, PathStatus::Absent)
        | (PathStatus::File, PathStatus::File)
        | (PathStatus::Invalid, _)
        | (_, PathStatus::Invalid) => Err(VaultError::Persist),
    }
}

fn restore_plan_deletion(deletion: &PlanDeletion) -> Result<(), VaultError> {
    match (
        inspect(&deletion.final_path).map_err(|_| VaultError::Persist)?,
        inspect(&deletion.marker_path).map_err(|_| VaultError::Persist)?,
    ) {
        (PathStatus::Absent, PathStatus::File) => {
            crate::storage::rename_in_dir(&deletion.marker_path, &deletion.final_path)
                .map_err(|_| VaultError::Persist)
        }
        (PathStatus::Absent, PathStatus::Absent) => {
            crate::storage::write_private(&deletion.final_path, &deletion.bytes)
                .map_err(|_| VaultError::Persist)
        }
        (PathStatus::File, PathStatus::Absent) => {
            crate::storage::restrict_private_file(&deletion.final_path)
                .map_err(|_| VaultError::Persist)
        }
        (PathStatus::File, PathStatus::File) => Err(VaultError::Persist),
        (PathStatus::Invalid, _) | (_, PathStatus::Invalid) => Err(VaultError::Persist),
    }
}
