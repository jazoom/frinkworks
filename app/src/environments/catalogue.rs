use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use super::id::{EnvironmentId, IdError, PreparationId};
use super::preparation::{
    FailureCategory, PreparationFailure, PreparationLogRecord, PreparationPhase, PreparationRecord,
    PreparationState,
};
use super::recipe::{
    EnvironmentDraft, EnvironmentRecipe, EnvironmentRecipeVersion, OciImageReference, RecipeError,
};
use super::seeds::{EnvironmentSeed, SeedKey};
use super::snapshot::{
    OciManifestDigest, PreparedSnapshot, RecordedIntegrity, SnapshotArtifactKey, SnapshotDigest,
};
use crate::storage::{self, BoundedLogger};

pub(crate) const CATALOGUE_FILE_VERSION: u32 = 1;
pub(crate) const MAXIMUM_ENVIRONMENTS: usize = 64;
pub(crate) const MAXIMUM_PREPARATIONS: usize = 4_096;
pub(crate) const MAXIMUM_RETIRED_IDS: usize = 4_096;
pub(crate) const MAXIMUM_APPLIED_SEEDS: usize = 64;
pub(crate) const MAXIMUM_CATALOGUE_BYTES: usize = 24 * 1024 * 1024;
pub(crate) const BROWSER_LOG_TAIL_BYTES: usize = 16 * 1024;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EnvironmentRecord {
    pub(crate) id: EnvironmentId,
    pub(crate) revision: u64,
    pub(crate) name: String,
    pub(crate) recipe: EnvironmentRecipe,
    pub(crate) recipe_version: EnvironmentRecipeVersion,
    pub(crate) ready_preparation: Option<PreparationId>,
    pub(crate) latest_preparation: PreparationId,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EnvironmentUpdate {
    pub(crate) environment: EnvironmentRecord,
    pub(crate) preparation: Option<PreparationRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeletedEnvironment {
    pub(crate) id: EnvironmentId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReadyPointer {
    pub(crate) environment_id: EnvironmentId,
    pub(crate) name: String,
    pub(crate) preparation_id: PreparationId,
    pub(crate) recipe_version: EnvironmentRecipeVersion,
    pub(crate) snapshot: PreparedSnapshot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReadyPointerError {
    Missing,
    NotReady,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RefreshCursor {
    pub(crate) generation: u64,
    pub(crate) sequence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EnvironmentError {
    Corrupt,
    Persist,
    Random,
    Full,
    DuplicateName,
    Missing,
    Conflict,
    Revision,
    Name,
    Image,
    Script,
    LocalPath,
    DiskImage,
    Archive,
    Busy,
}

impl EnvironmentError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Corrupt => "The environment catalogue is unreadable.",
            Self::Persist => "Frinkworks could not store that environment. Try again.",
            Self::Random => "Frinkworks could not create an environment identifier. Try again.",
            Self::Full => "The environment catalogue is full.",
            Self::DuplicateName => "An environment with that name already exists.",
            Self::Missing => "That environment is no longer in the catalogue.",
            Self::Conflict => "That environment changed in another tab. Reload it.",
            Self::Revision => "Frinkworks could not store another edit of that environment.",
            Self::Name => RecipeError::Name.message(),
            Self::Image => RecipeError::Image.message(),
            Self::Script => RecipeError::Script.message(),
            Self::LocalPath => RecipeError::LocalPath.message(),
            Self::DiskImage => RecipeError::DiskImage.message(),
            Self::Archive => RecipeError::Archive.message(),
            Self::Busy => "Wait until the current preparation finishes.",
        }
    }
}

impl From<RecipeError> for EnvironmentError {
    fn from(error: RecipeError) -> Self {
        match error {
            RecipeError::Name => Self::Name,
            RecipeError::Image => Self::Image,
            RecipeError::Script => Self::Script,
            RecipeError::LocalPath => Self::LocalPath,
            RecipeError::DiskImage => Self::DiskImage,
            RecipeError::Archive => Self::Archive,
        }
    }
}

impl std::fmt::Display for EnvironmentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for EnvironmentError {}

struct AppliedEnvironmentSeed {
    key: SeedKey,
    environment_id: EnvironmentId,
}

struct CatalogueState {
    applied_seeds: Vec<AppliedEnvironmentSeed>,
    retired_environment_ids: Vec<EnvironmentId>,
    environments: Vec<EnvironmentRecord>,
    preparations: Vec<PreparationRecord>,
}

struct RefreshState {
    generation: u64,
    sequence: Mutex<u64>,
    notify: Notify,
}

pub(crate) struct EnvironmentCatalogue {
    path: Option<PathBuf>,
    log_dir: Option<PathBuf>,
    inner: Mutex<CatalogueState>,
    refresh: RefreshState,
    #[cfg(test)]
    _scratch: Option<tempfile::TempDir>,
}

impl EnvironmentCatalogue {
    pub(crate) fn open(path: PathBuf, log_dir: PathBuf) -> Result<Self, EnvironmentError> {
        Self::open_with_seeds(path, log_dir, &super::seeds::production_seeds())
    }

    pub(crate) fn open_with_seeds(
        path: PathBuf,
        log_dir: PathBuf,
        seeds: &[EnvironmentSeed],
    ) -> Result<Self, EnvironmentError> {
        storage::ensure_private_dir(&log_dir).map_err(|_| EnvironmentError::Persist)?;
        let mut state = load_path(&path, &log_dir)?;
        let seeded = apply_absent_seeds(&mut state, seeds)?;
        let interrupted = interrupt_preparing(&mut state);
        if seeded || interrupted {
            persist(Some(&path), &state)?;
        }
        Ok(Self {
            path: Some(path),
            log_dir: Some(log_dir),
            inner: Mutex::new(state),
            refresh: RefreshState::new(),
            #[cfg(test)]
            _scratch: None,
        })
    }

    pub(crate) fn list(&self) -> Vec<EnvironmentRecord> {
        let mut records = self.lock().environments.clone();
        records.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then(left.id.cmp(&right.id))
        });
        records
    }

    pub(crate) fn seed_id(&self, key: &str) -> Option<EnvironmentId> {
        let key = SeedKey::parse(key)?;
        self.lock()
            .applied_seeds
            .iter()
            .find(|seed| seed.key == key)
            .map(|seed| seed.environment_id)
    }

    pub(crate) fn copy_ready_pointer(
        &self,
        id: &EnvironmentId,
    ) -> Result<ReadyPointer, ReadyPointerError> {
        let state = self.lock();
        let environment = state
            .environments
            .iter()
            .find(|record| record.id == *id)
            .ok_or(ReadyPointerError::Missing)?;
        let preparation_id = environment
            .ready_preparation
            .ok_or(ReadyPointerError::NotReady)?;
        let preparation = state
            .preparations
            .iter()
            .find(|record| record.id == preparation_id)
            .ok_or(ReadyPointerError::NotReady)?;
        if preparation.environment_id != *id || preparation.state != PreparationState::Ready {
            return Err(ReadyPointerError::NotReady);
        }
        let snapshot = preparation
            .snapshot
            .clone()
            .ok_or(ReadyPointerError::NotReady)?;
        Ok(ReadyPointer {
            environment_id: *id,
            name: environment.name.clone(),
            preparation_id,
            recipe_version: preparation.recipe_version,
            snapshot,
        })
    }

    pub(crate) fn ready_pointer_matches(&self, copied: &ReadyPointer) -> bool {
        match self.copy_ready_pointer(&copied.environment_id) {
            Ok(current) => {
                current.preparation_id == copied.preparation_id
                    && current.snapshot.snapshot_digest == copied.snapshot.snapshot_digest
                    && current.snapshot.artifact_key == copied.snapshot.artifact_key
            }
            Err(_) => false,
        }
    }

    pub(crate) fn get(&self, id: &EnvironmentId) -> Option<EnvironmentRecord> {
        self.lock()
            .environments
            .iter()
            .find(|record| record.id == *id)
            .cloned()
    }

    pub(crate) fn preparation(&self, id: &PreparationId) -> Option<PreparationRecord> {
        self.lock()
            .preparations
            .iter()
            .find(|record| record.id == *id)
            .cloned()
    }

    pub(crate) fn preparations_for(&self, id: &EnvironmentId) -> Vec<PreparationRecord> {
        let mut records: Vec<_> = self
            .lock()
            .preparations
            .iter()
            .filter(|record| record.environment_id == *id)
            .cloned()
            .collect();
        records.sort_by(|left, right| {
            right
                .ordinal
                .cmp(&left.ordinal)
                .then(left.id.cmp(&right.id))
        });
        records
    }

    pub(crate) fn create(
        &self,
        draft: EnvironmentDraft,
    ) -> Result<(EnvironmentRecord, PreparationRecord), EnvironmentError> {
        let (name, recipe) = EnvironmentRecipe::from_draft(&draft)?;
        let mut state = self.lock();
        if state.environments.len() >= MAXIMUM_ENVIRONMENTS
            || state.preparations.len() >= MAXIMUM_PREPARATIONS
        {
            return Err(EnvironmentError::Full);
        }
        reject_duplicate_name(&state.environments, None, &name)?;
        let environment_id = unused_environment_id(&state)?;
        let preparation_id = unused_preparation_id(&state)?;
        let now = now_ms();
        let recipe_version = recipe.version();
        let preparation =
            PreparationRecord::queued(preparation_id, environment_id, 1, 1, recipe_version, now);
        let environment = EnvironmentRecord {
            id: environment_id,
            revision: 1,
            name,
            recipe,
            recipe_version,
            ready_preparation: None,
            latest_preparation: preparation_id,
            created_at_ms: now,
            updated_at_ms: now,
        };
        let mut next = state.clone_state();
        next.environments.push(environment.clone());
        next.preparations.push(preparation.clone());
        persist(self.path.as_deref(), &next)?;
        *state = next;
        drop(state);
        self.bump_refresh();
        Ok((environment, preparation))
    }

    pub(crate) fn update(
        &self,
        id: &EnvironmentId,
        expected_revision: u64,
        draft: EnvironmentDraft,
    ) -> Result<EnvironmentUpdate, EnvironmentError> {
        let (name, recipe) = EnvironmentRecipe::from_draft(&draft)?;
        let mut state = self.lock();
        let index = state
            .environments
            .iter()
            .position(|record| record.id == *id)
            .ok_or(EnvironmentError::Missing)?;
        let current = state.environments[index].clone();
        if current.revision != expected_revision {
            return Err(EnvironmentError::Conflict);
        }
        if current.name == name && current.recipe == recipe {
            return Ok(EnvironmentUpdate {
                environment: current,
                preparation: None,
            });
        }
        reject_duplicate_name(&state.environments, Some(*id), &name)?;
        let revision = current
            .revision
            .checked_add(1)
            .ok_or(EnvironmentError::Revision)?;
        let recipe_version = recipe.version();
        let recipe_changed = current.recipe_version != recipe_version;
        let now = now_ms();
        let mut next = state.clone_state();
        let mut queued = None;
        let mut latest = current.latest_preparation;
        if recipe_changed {
            if next.preparations.len() >= MAXIMUM_PREPARATIONS {
                return Err(EnvironmentError::Full);
            }
            supersede_queued(&mut next, *id, now);
            let ordinal = next_ordinal(&next, *id)?;
            let preparation_id = unused_preparation_id(&next)?;
            let preparation = PreparationRecord::queued(
                preparation_id,
                *id,
                ordinal,
                revision,
                recipe_version,
                now,
            );
            latest = preparation_id;
            next.preparations.push(preparation.clone());
            queued = Some(preparation);
        }
        let environment = EnvironmentRecord {
            id: current.id,
            revision,
            name,
            recipe,
            recipe_version,
            ready_preparation: current.ready_preparation,
            latest_preparation: latest,
            created_at_ms: current.created_at_ms,
            updated_at_ms: now,
        };
        next.environments[index] = environment.clone();
        persist(self.path.as_deref(), &next)?;
        *state = next;
        drop(state);
        self.bump_refresh();
        Ok(EnvironmentUpdate {
            environment,
            preparation: queued,
        })
    }

    pub(crate) fn retry_preparation(
        &self,
        id: &EnvironmentId,
        expected_revision: u64,
        expected_recipe: &EnvironmentRecipeVersion,
    ) -> Result<PreparationRecord, EnvironmentError> {
        let mut state = self.lock();
        let index = state
            .environments
            .iter()
            .position(|record| record.id == *id)
            .ok_or(EnvironmentError::Missing)?;
        let current = state.environments[index].clone();
        if current.revision != expected_revision {
            return Err(EnvironmentError::Conflict);
        }
        if current.recipe_version != *expected_recipe {
            return Err(EnvironmentError::Conflict);
        }
        if let Some(latest) = state
            .preparations
            .iter()
            .find(|record| record.id == current.latest_preparation)
            && latest.state.is_active()
            && latest.recipe_version == current.recipe_version
        {
            return Err(EnvironmentError::Busy);
        }
        if state.preparations.len() >= MAXIMUM_PREPARATIONS {
            return Err(EnvironmentError::Full);
        }
        let revision = current
            .revision
            .checked_add(1)
            .ok_or(EnvironmentError::Revision)?;
        let now = now_ms();
        let mut next = state.clone_state();
        supersede_queued(&mut next, *id, now);
        let ordinal = next_ordinal(&next, *id)?;
        let preparation_id = unused_preparation_id(&next)?;
        let preparation = PreparationRecord::queued(
            preparation_id,
            *id,
            ordinal,
            revision,
            current.recipe_version,
            now,
        );
        next.environments[index] = EnvironmentRecord {
            revision,
            latest_preparation: preparation_id,
            updated_at_ms: now,
            ..current
        };
        next.preparations.push(preparation.clone());
        persist(self.path.as_deref(), &next)?;
        *state = next;
        drop(state);
        self.bump_refresh();
        Ok(preparation)
    }

    pub(crate) fn delete(
        &self,
        id: &EnvironmentId,
        expected_revision: u64,
    ) -> Result<DeletedEnvironment, EnvironmentError> {
        let mut state = self.lock();
        let index = state
            .environments
            .iter()
            .position(|record| record.id == *id)
            .ok_or(EnvironmentError::Missing)?;
        if state.environments[index].revision != expected_revision {
            return Err(EnvironmentError::Conflict);
        }
        if state.retired_environment_ids.len() >= MAXIMUM_RETIRED_IDS {
            return Err(EnvironmentError::Full);
        }
        let now = now_ms();
        let mut next = state.clone_state();
        next.environments.remove(index);
        next.retired_environment_ids.push(*id);
        for preparation in &mut next.preparations {
            if preparation.environment_id != *id || !preparation.state.is_active() {
                continue;
            }
            preparation.state = PreparationState::Cancelled;
            preparation.phase = PreparationPhase::Finished;
            preparation.finished_at_ms = Some(
                now.max(
                    preparation
                        .started_at_ms
                        .unwrap_or(preparation.requested_at_ms),
                ),
            );
            preparation.failure = None;
            preparation.snapshot = None;
        }
        persist(self.path.as_deref(), &next)?;
        *state = next;
        drop(state);
        self.bump_refresh();
        Ok(DeletedEnvironment { id: *id })
    }

    pub(crate) fn claim_oldest_queued(
        &self,
    ) -> Result<Option<PreparationRecord>, EnvironmentError> {
        let mut state = self.lock();
        let mut candidates: Vec<usize> = state
            .preparations
            .iter()
            .enumerate()
            .filter(|(_, record)| record.state == PreparationState::Queued)
            .map(|(index, _)| index)
            .collect();
        candidates.sort_by(|&left, &right| {
            let left_record = &state.preparations[left];
            let right_record = &state.preparations[right];
            left_record
                .requested_at_ms
                .cmp(&right_record.requested_at_ms)
                .then(left_record.id.cmp(&right_record.id))
        });
        let Some(index) = candidates.first().copied() else {
            return Ok(None);
        };
        let now = now_ms();
        let mut next = state.clone_state();
        next.preparations[index].state = PreparationState::Preparing;
        next.preparations[index].phase = PreparationPhase::CreatingGuest;
        next.preparations[index].started_at_ms =
            Some(now.max(next.preparations[index].requested_at_ms));
        persist(self.path.as_deref(), &next)?;
        let claimed = next.preparations[index].clone();
        *state = next;
        drop(state);
        self.bump_refresh();
        Ok(Some(claimed))
    }

    pub(crate) fn set_phase(
        &self,
        id: &PreparationId,
        phase: PreparationPhase,
        log: PreparationLogRecord,
    ) -> Result<PreparationRecord, EnvironmentError> {
        self.mutate_preparation(id, |record| {
            if record.state != PreparationState::Preparing {
                return Err(EnvironmentError::Conflict);
            }
            if phase < record.phase {
                return Err(EnvironmentError::Corrupt);
            }
            record.phase = phase;
            record.log = log;
            Ok(())
        })
    }

    pub(crate) fn finish_ready(
        &self,
        id: &PreparationId,
        snapshot: PreparedSnapshot,
        log: PreparationLogRecord,
    ) -> Result<PreparationRecord, EnvironmentError> {
        let mut state = self.lock();
        let prep_index = state
            .preparations
            .iter()
            .position(|record| record.id == *id)
            .ok_or(EnvironmentError::Missing)?;
        let current = state.preparations[prep_index].clone();
        if current.state != PreparationState::Preparing {
            return Err(EnvironmentError::Conflict);
        }
        if !activation_allowed(&state, &current) {
            return Err(EnvironmentError::Conflict);
        }
        let now = now_ms();
        let mut next = state.clone_state();
        let env_index = next
            .environments
            .iter()
            .position(|record| record.id == current.environment_id)
            .ok_or(EnvironmentError::Missing)?;
        next.preparations[prep_index].state = PreparationState::Ready;
        next.preparations[prep_index].phase = PreparationPhase::Finished;
        next.preparations[prep_index].finished_at_ms =
            Some(now.max(current.started_at_ms.unwrap_or(now)));
        next.preparations[prep_index].log = log;
        next.preparations[prep_index].failure = None;
        next.preparations[prep_index].snapshot = Some(snapshot);
        next.environments[env_index].ready_preparation = Some(current.id);
        persist(self.path.as_deref(), &next)?;
        let record = next.preparations[prep_index].clone();
        *state = next;
        drop(state);
        self.bump_refresh();
        Ok(record)
    }

    pub(crate) fn finish_failed(
        &self,
        id: &PreparationId,
        category: FailureCategory,
        log: PreparationLogRecord,
    ) -> Result<PreparationRecord, EnvironmentError> {
        self.mutate_preparation(id, |record| {
            if record.state != PreparationState::Preparing {
                return Err(EnvironmentError::Conflict);
            }
            let now = now_ms();
            record.state = PreparationState::Failed;
            record.finished_at_ms = Some(now.max(record.started_at_ms.unwrap_or(now)));
            record.log = log;
            record.failure = Some(PreparationFailure::new(category));
            record.snapshot = None;
            Ok(())
        })
    }

    pub(crate) fn finish_superseded(
        &self,
        id: &PreparationId,
        log: PreparationLogRecord,
    ) -> Result<PreparationRecord, EnvironmentError> {
        self.mutate_preparation(id, |record| {
            if !record.state.is_active() {
                return Err(EnvironmentError::Conflict);
            }
            let now = now_ms();
            record.state = PreparationState::Superseded;
            record.phase = PreparationPhase::Finished;
            record.finished_at_ms = Some(now.max(record.started_at_ms.unwrap_or(now)));
            record.log = log;
            record.failure = None;
            record.snapshot = None;
            Ok(())
        })
    }

    pub(crate) fn is_current(&self, preparation: &PreparationRecord) -> bool {
        activation_allowed(&self.lock(), preparation)
    }

    pub(crate) fn log_path(&self, id: &PreparationId) -> Result<PathBuf, EnvironmentError> {
        let dir = self.log_dir.as_deref().ok_or(EnvironmentError::Persist)?;
        let name = format!("{}.log", id.as_hex());
        storage::confined_child(dir, &name).map_err(|_| EnvironmentError::Persist)
    }

    pub(crate) fn open_logger(
        &self,
        id: &PreparationId,
    ) -> Result<BoundedLogger, EnvironmentError> {
        let path = self.log_path(id)?;
        BoundedLogger::create(path).map_err(|_| EnvironmentError::Persist)
    }

    pub(crate) fn log_projection(&self, record: &PreparationRecord) -> (String, bool, bool) {
        let Ok(path) = self.log_path(&record.id) else {
            return (String::new(), record.log.truncated, false);
        };
        let bytes = fs::read(&path).unwrap_or_default();
        let browser_truncated = bytes.len() > BROWSER_LOG_TAIL_BYTES;
        let tail = if browser_truncated {
            &bytes[bytes.len() - BROWSER_LOG_TAIL_BYTES..]
        } else {
            &bytes
        };
        (
            String::from_utf8_lossy(tail).into_owned(),
            record.log.truncated,
            browser_truncated,
        )
    }

    pub(crate) fn refresh_cursor(&self) -> RefreshCursor {
        RefreshCursor {
            generation: self.refresh.generation,
            sequence: *lock_mutex(&self.refresh.sequence),
        }
    }

    pub(crate) fn parse_refresh_cursor(value: &str) -> Option<RefreshCursor> {
        let (generation, sequence) = value.split_once('-')?;
        if generation.len() != 16 || sequence.is_empty() || sequence.len() > 20 {
            return None;
        }
        if !generation
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return None;
        }
        if !sequence.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        Some(RefreshCursor {
            generation: u64::from_str_radix(generation, 16).ok()?,
            sequence: sequence.parse().ok()?,
        })
    }

    pub(crate) fn cursor_token(cursor: RefreshCursor) -> String {
        format!("{:016x}-{}", cursor.generation, cursor.sequence)
    }

    pub(crate) fn cursor_is_current(&self, cursor: RefreshCursor) -> bool {
        cursor.generation == self.refresh.generation
            && cursor.sequence == *lock_mutex(&self.refresh.sequence)
    }

    pub(crate) fn cursor_is_stale(&self, cursor: Option<RefreshCursor>) -> bool {
        let Some(cursor) = cursor else {
            return true;
        };
        cursor.generation != self.refresh.generation
            || cursor.sequence != *lock_mutex(&self.refresh.sequence)
    }

    pub(crate) async fn wait_while_current(&self, cursor: RefreshCursor, hold: Duration) {
        if !self.cursor_is_current(cursor) {
            return;
        }
        let notified = self.refresh.notify.notified();
        if !self.cursor_is_current(cursor) {
            return;
        }
        let _ = tokio::time::timeout(hold, notified).await;
    }

    pub(crate) fn bump_refresh(&self) {
        let mut sequence = lock_mutex(&self.refresh.sequence);
        *sequence = sequence.saturating_add(1);
        drop(sequence);
        self.refresh.notify.notify_waiters();
    }

    fn mutate_preparation(
        &self,
        id: &PreparationId,
        mutate: impl FnOnce(&mut PreparationRecord) -> Result<(), EnvironmentError>,
    ) -> Result<PreparationRecord, EnvironmentError> {
        let mut state = self.lock();
        let index = state
            .preparations
            .iter()
            .position(|record| record.id == *id)
            .ok_or(EnvironmentError::Missing)?;
        let mut next = state.clone_state();
        mutate(&mut next.preparations[index])?;
        persist(self.path.as_deref(), &next)?;
        let record = next.preparations[index].clone();
        *state = next;
        drop(state);
        self.bump_refresh();
        Ok(record)
    }

    fn lock(&self) -> MutexGuard<'_, CatalogueState> {
        lock_mutex(&self.inner)
    }
}

impl CatalogueState {
    fn clone_state(&self) -> Self {
        Self {
            applied_seeds: self.applied_seeds.clone(),
            retired_environment_ids: self.retired_environment_ids.clone(),
            environments: self.environments.clone(),
            preparations: self.preparations.clone(),
        }
    }
}

impl Clone for AppliedEnvironmentSeed {
    fn clone(&self) -> Self {
        Self {
            key: self.key.clone(),
            environment_id: self.environment_id,
        }
    }
}

impl RefreshState {
    fn new() -> Self {
        Self {
            generation: now_ms().max(1),
            sequence: Mutex::new(0),
            notify: Notify::new(),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct CatalogueFile {
    file_version: u32,
    applied_seeds: Vec<AppliedSeedFile>,
    retired_environment_ids: Vec<String>,
    environments: Vec<EnvironmentRecordFile>,
    preparations: Vec<PreparationRecordFile>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct AppliedSeedFile {
    key: String,
    environment_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct EnvironmentRecordFile {
    id: String,
    revision: u64,
    name: String,
    recipe: RecipeFile,
    recipe_version: String,
    ready_preparation: Option<String>,
    latest_preparation: String,
    created_at_ms: u64,
    updated_at_ms: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct RecipeFile {
    oci_image: String,
    setup_script: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct PreparationRecordFile {
    id: String,
    environment_id: String,
    ordinal: u64,
    environment_revision: u64,
    recipe_version: String,
    state: String,
    phase: String,
    requested_at_ms: u64,
    started_at_ms: Option<u64>,
    finished_at_ms: Option<u64>,
    log: LogFile,
    failure: Option<String>,
    snapshot: Option<SnapshotFile>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct LogFile {
    captured_bytes: u64,
    truncated: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct SnapshotFile {
    artifact_key: String,
    snapshot_digest: String,
    image_reference: String,
    image_manifest_digest: String,
    upper_integrity: IntegrityFile,
    upper_size_bytes: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct IntegrityFile {
    algorithm: String,
    value: String,
}

fn empty_state() -> CatalogueState {
    CatalogueState {
        applied_seeds: Vec::new(),
        retired_environment_ids: Vec::new(),
        environments: Vec::new(),
        preparations: Vec::new(),
    }
}

fn load_path(path: &Path, log_dir: &Path) -> Result<CatalogueState, EnvironmentError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(empty_state()),
        Err(_) => return Err(EnvironmentError::Corrupt),
    };
    if bytes.len() > MAXIMUM_CATALOGUE_BYTES {
        return Err(EnvironmentError::Corrupt);
    }
    let file: CatalogueFile =
        serde_json::from_slice(&bytes).map_err(|_| EnvironmentError::Corrupt)?;
    state_from_file(file, log_dir)
}

fn state_from_file(
    file: CatalogueFile,
    log_dir: &Path,
) -> Result<CatalogueState, EnvironmentError> {
    if file.file_version != CATALOGUE_FILE_VERSION {
        return Err(EnvironmentError::Corrupt);
    }
    if file.environments.len() > MAXIMUM_ENVIRONMENTS
        || file.preparations.len() > MAXIMUM_PREPARATIONS
        || file.retired_environment_ids.len() > MAXIMUM_RETIRED_IDS
        || file.applied_seeds.len() > MAXIMUM_APPLIED_SEEDS
    {
        return Err(EnvironmentError::Corrupt);
    }
    let mut retired_environment_ids = Vec::new();
    for raw in file.retired_environment_ids {
        let id = EnvironmentId::parse(&raw).ok_or(EnvironmentError::Corrupt)?;
        if retired_environment_ids.contains(&id) {
            return Err(EnvironmentError::Corrupt);
        }
        retired_environment_ids.push(id);
    }
    let mut preparations = Vec::new();
    let mut preparation_ids = Vec::new();
    let mut artifact_keys = Vec::new();
    for record in file.preparations {
        let loaded = preparation_from_file(record, log_dir)?;
        if preparation_ids.contains(&loaded.id) {
            return Err(EnvironmentError::Corrupt);
        }
        if let Some(snapshot) = &loaded.snapshot {
            if artifact_keys.contains(&snapshot.artifact_key) {
                return Err(EnvironmentError::Corrupt);
            }
            artifact_keys.push(snapshot.artifact_key.clone());
        }
        preparation_ids.push(loaded.id);
        preparations.push(loaded);
    }
    let mut environments = Vec::new();
    let mut names = Vec::new();
    for record in file.environments {
        let loaded = environment_from_file(record, &preparations, &retired_environment_ids)?;
        if environments
            .iter()
            .any(|item: &EnvironmentRecord| item.id == loaded.id)
            || retired_environment_ids.contains(&loaded.id)
        {
            return Err(EnvironmentError::Corrupt);
        }
        if names
            .iter()
            .any(|name: &String| name.eq_ignore_ascii_case(&loaded.name))
        {
            return Err(EnvironmentError::Corrupt);
        }
        names.push(loaded.name.clone());
        environments.push(loaded);
    }
    reject_duplicate_ordinals(&preparations)?;
    let mut applied_seeds = Vec::new();
    let mut seed_keys = Vec::new();
    let mut seed_ids = Vec::new();
    for seed in file.applied_seeds {
        let key = SeedKey::parse(&seed.key).ok_or(EnvironmentError::Corrupt)?;
        let environment_id =
            EnvironmentId::parse(&seed.environment_id).ok_or(EnvironmentError::Corrupt)?;
        if seed_keys.contains(&key) || seed_ids.contains(&environment_id) {
            return Err(EnvironmentError::Corrupt);
        }
        let known = environments
            .iter()
            .any(|record| record.id == environment_id)
            || retired_environment_ids.contains(&environment_id);
        if !known {
            return Err(EnvironmentError::Corrupt);
        }
        seed_keys.push(key.clone());
        seed_ids.push(environment_id);
        applied_seeds.push(AppliedEnvironmentSeed {
            key,
            environment_id,
        });
    }
    Ok(CatalogueState {
        applied_seeds,
        retired_environment_ids,
        environments,
        preparations,
    })
}

fn environment_from_file(
    file: EnvironmentRecordFile,
    preparations: &[PreparationRecord],
    retired: &[EnvironmentId],
) -> Result<EnvironmentRecord, EnvironmentError> {
    if file.revision == 0 || file.updated_at_ms < file.created_at_ms {
        return Err(EnvironmentError::Corrupt);
    }
    let id = EnvironmentId::parse(&file.id).ok_or(EnvironmentError::Corrupt)?;
    if retired.contains(&id) {
        return Err(EnvironmentError::Corrupt);
    }
    let name = super::recipe::normalise_name(&file.name).map_err(|_| EnvironmentError::Corrupt)?;
    let recipe = EnvironmentRecipe {
        oci_image: OciImageReference::parse(&file.recipe.oci_image)
            .map_err(|_| EnvironmentError::Corrupt)?,
        setup_script: file.recipe.setup_script,
    };
    let stored_version =
        EnvironmentRecipeVersion::parse(&file.recipe_version).ok_or(EnvironmentError::Corrupt)?;
    if recipe.version() != stored_version {
        return Err(EnvironmentError::Corrupt);
    }
    let latest = PreparationId::parse(&file.latest_preparation).ok_or(EnvironmentError::Corrupt)?;
    let latest_record = preparations
        .iter()
        .find(|record| record.id == latest)
        .ok_or(EnvironmentError::Corrupt)?;
    if latest_record.environment_id != id {
        return Err(EnvironmentError::Corrupt);
    }
    let ready_preparation = match file.ready_preparation {
        Some(raw) => {
            let ready = PreparationId::parse(&raw).ok_or(EnvironmentError::Corrupt)?;
            let ready_record = preparations
                .iter()
                .find(|record| record.id == ready)
                .ok_or(EnvironmentError::Corrupt)?;
            if ready_record.environment_id != id || ready_record.state != PreparationState::Ready {
                return Err(EnvironmentError::Corrupt);
            }
            Some(ready)
        }
        None => None,
    };
    Ok(EnvironmentRecord {
        id,
        revision: file.revision,
        name,
        recipe,
        recipe_version: stored_version,
        ready_preparation,
        latest_preparation: latest,
        created_at_ms: file.created_at_ms,
        updated_at_ms: file.updated_at_ms,
    })
}

fn preparation_from_file(
    file: PreparationRecordFile,
    log_dir: &Path,
) -> Result<PreparationRecord, EnvironmentError> {
    let id = PreparationId::parse(&file.id).ok_or(EnvironmentError::Corrupt)?;
    let environment_id =
        EnvironmentId::parse(&file.environment_id).ok_or(EnvironmentError::Corrupt)?;
    let recipe_version =
        EnvironmentRecipeVersion::parse(&file.recipe_version).ok_or(EnvironmentError::Corrupt)?;
    let state = PreparationState::parse(&file.state).ok_or(EnvironmentError::Corrupt)?;
    let phase = PreparationPhase::parse(&file.phase).ok_or(EnvironmentError::Corrupt)?;
    let failure = match file.failure {
        Some(raw) => Some(PreparationFailure::new(
            FailureCategory::parse(&raw).ok_or(EnvironmentError::Corrupt)?,
        )),
        None => None,
    };
    let snapshot = match file.snapshot {
        Some(raw) => {
            let snapshot = snapshot_from_file(raw)?;
            if snapshot.artifact_key != SnapshotArtifactKey::from_preparation(&id) {
                return Err(EnvironmentError::Corrupt);
            }
            Some(snapshot)
        }
        None => None,
    };
    if file.log.captured_bytes > 0 {
        let name = format!("{}.log", id.as_hex());
        let path =
            storage::confined_child(log_dir, &name).map_err(|_| EnvironmentError::Corrupt)?;
        if !path.is_file() {
            return Err(EnvironmentError::Corrupt);
        }
    }
    let record = PreparationRecord {
        id,
        environment_id,
        ordinal: file.ordinal,
        environment_revision: file.environment_revision,
        recipe_version,
        state,
        phase,
        requested_at_ms: file.requested_at_ms,
        started_at_ms: file.started_at_ms,
        finished_at_ms: file.finished_at_ms,
        log: PreparationLogRecord {
            captured_bytes: file.log.captured_bytes,
            truncated: file.log.truncated,
        },
        failure,
        snapshot,
    };
    if !record.validate_combination() {
        return Err(EnvironmentError::Corrupt);
    }
    Ok(record)
}

fn snapshot_from_file(file: SnapshotFile) -> Result<PreparedSnapshot, EnvironmentError> {
    if file.upper_integrity.algorithm.is_empty() || file.upper_integrity.value.is_empty() {
        return Err(EnvironmentError::Corrupt);
    }
    Ok(PreparedSnapshot {
        artifact_key: SnapshotArtifactKey::parse(&file.artifact_key)
            .ok_or(EnvironmentError::Corrupt)?,
        snapshot_digest: SnapshotDigest::parse(&file.snapshot_digest)
            .ok_or(EnvironmentError::Corrupt)?,
        image_reference: file.image_reference,
        image_manifest_digest: OciManifestDigest::parse(&file.image_manifest_digest)
            .ok_or(EnvironmentError::Corrupt)?,
        upper_integrity: RecordedIntegrity {
            algorithm: file.upper_integrity.algorithm,
            value: file.upper_integrity.value,
        },
        upper_size_bytes: file.upper_size_bytes,
    })
}

fn apply_absent_seeds(
    state: &mut CatalogueState,
    seeds: &[EnvironmentSeed],
) -> Result<bool, EnvironmentError> {
    let mut changed = false;
    let mut seen_keys = Vec::new();
    for seed in seeds {
        if seen_keys.contains(&seed.key) {
            return Err(EnvironmentError::Corrupt);
        }
        seen_keys.push(seed.key.clone());
        if state
            .applied_seeds
            .iter()
            .any(|applied| applied.key == seed.key)
        {
            continue;
        }
        if state.applied_seeds.len() >= MAXIMUM_APPLIED_SEEDS
            || state.environments.len() >= MAXIMUM_ENVIRONMENTS
            || state.preparations.len() >= MAXIMUM_PREPARATIONS
        {
            return Err(EnvironmentError::Full);
        }
        let (name, recipe) = EnvironmentRecipe::from_draft(&seed.draft)?;
        reject_duplicate_name(&state.environments, None, &name)?;
        let environment_id = unused_environment_id(state)?;
        let preparation_id = unused_preparation_id(state)?;
        let now = now_ms();
        let recipe_version = recipe.version();
        state.preparations.push(PreparationRecord::queued(
            preparation_id,
            environment_id,
            1,
            1,
            recipe_version,
            now,
        ));
        state.environments.push(EnvironmentRecord {
            id: environment_id,
            revision: 1,
            name,
            recipe,
            recipe_version,
            ready_preparation: None,
            latest_preparation: preparation_id,
            created_at_ms: now,
            updated_at_ms: now,
        });
        state.applied_seeds.push(AppliedEnvironmentSeed {
            key: seed.key.clone(),
            environment_id,
        });
        changed = true;
    }
    Ok(changed)
}

fn interrupt_preparing(state: &mut CatalogueState) -> bool {
    let now = now_ms();
    let mut changed = false;
    for preparation in &mut state.preparations {
        if preparation.state != PreparationState::Preparing {
            continue;
        }
        preparation.state = PreparationState::Interrupted;
        preparation.finished_at_ms = Some(now.max(preparation.started_at_ms.unwrap_or(now)));
        preparation.failure = Some(PreparationFailure::new(FailureCategory::ProcessRestarted));
        preparation.snapshot = None;
        changed = true;
    }
    changed
}

fn unused_environment_id(state: &CatalogueState) -> Result<EnvironmentId, EnvironmentError> {
    for _ in 0..16 {
        let id = EnvironmentId::generate().map_err(|error| match error {
            IdError::RandomUnavailable => EnvironmentError::Random,
        })?;
        if !environment_taken(state, &id) {
            return Ok(id);
        }
    }
    Err(EnvironmentError::Random)
}

fn unused_preparation_id(state: &CatalogueState) -> Result<PreparationId, EnvironmentError> {
    for _ in 0..16 {
        let id = PreparationId::generate().map_err(|error| match error {
            IdError::RandomUnavailable => EnvironmentError::Random,
        })?;
        if !state.preparations.iter().any(|record| record.id == id) {
            return Ok(id);
        }
    }
    Err(EnvironmentError::Random)
}

fn environment_taken(state: &CatalogueState, id: &EnvironmentId) -> bool {
    state.environments.iter().any(|record| record.id == *id)
        || state.retired_environment_ids.contains(id)
        || state
            .applied_seeds
            .iter()
            .any(|seed| seed.environment_id == *id)
}

fn reject_duplicate_name(
    environments: &[EnvironmentRecord],
    current: Option<EnvironmentId>,
    name: &str,
) -> Result<(), EnvironmentError> {
    if environments
        .iter()
        .any(|record| current != Some(record.id) && record.name.eq_ignore_ascii_case(name))
    {
        return Err(EnvironmentError::DuplicateName);
    }
    Ok(())
}

fn reject_duplicate_ordinals(preparations: &[PreparationRecord]) -> Result<(), EnvironmentError> {
    for (index, preparation) in preparations.iter().enumerate() {
        if preparations.iter().skip(index + 1).any(|other| {
            other.environment_id == preparation.environment_id
                && other.ordinal == preparation.ordinal
        }) {
            return Err(EnvironmentError::Corrupt);
        }
    }
    Ok(())
}

fn next_ordinal(state: &CatalogueState, id: EnvironmentId) -> Result<u64, EnvironmentError> {
    let current = state
        .preparations
        .iter()
        .filter(|record| record.environment_id == id)
        .map(|record| record.ordinal)
        .max()
        .unwrap_or(0);
    current.checked_add(1).ok_or(EnvironmentError::Revision)
}

fn supersede_queued(state: &mut CatalogueState, id: EnvironmentId, now: u64) {
    for preparation in &mut state.preparations {
        if preparation.environment_id == id && preparation.state == PreparationState::Queued {
            preparation.state = PreparationState::Superseded;
            preparation.finished_at_ms = Some(now.max(preparation.requested_at_ms));
        }
    }
}

fn activation_allowed(state: &CatalogueState, preparation: &PreparationRecord) -> bool {
    let Some(environment) = state
        .environments
        .iter()
        .find(|record| record.id == preparation.environment_id)
    else {
        return false;
    };
    environment.latest_preparation == preparation.id
        && environment.recipe_version == preparation.recipe_version
        && environment.revision >= preparation.environment_revision
}

fn persist(path: Option<&Path>, state: &CatalogueState) -> Result<(), EnvironmentError> {
    let Some(path) = path else {
        return Ok(());
    };
    let file = CatalogueFile {
        file_version: CATALOGUE_FILE_VERSION,
        applied_seeds: state
            .applied_seeds
            .iter()
            .map(|seed| AppliedSeedFile {
                key: seed.key.as_str().to_owned(),
                environment_id: seed.environment_id.as_hex(),
            })
            .collect(),
        retired_environment_ids: state
            .retired_environment_ids
            .iter()
            .map(EnvironmentId::as_hex)
            .collect(),
        environments: state.environments.iter().map(environment_to_file).collect(),
        preparations: state.preparations.iter().map(preparation_to_file).collect(),
    };
    let bytes = serde_json::to_vec_pretty(&file).map_err(|_| EnvironmentError::Persist)?;
    if bytes.len() > MAXIMUM_CATALOGUE_BYTES {
        return Err(EnvironmentError::Full);
    }
    let dir = path.parent().ok_or(EnvironmentError::Persist)?;
    storage::ensure_private_dir(dir).map_err(|_| EnvironmentError::Persist)?;
    storage::write_private(path, &bytes).map_err(|_| EnvironmentError::Persist)
}

fn environment_to_file(record: &EnvironmentRecord) -> EnvironmentRecordFile {
    EnvironmentRecordFile {
        id: record.id.as_hex(),
        revision: record.revision,
        name: record.name.clone(),
        recipe: RecipeFile {
            oci_image: record.recipe.oci_image.as_str().to_owned(),
            setup_script: record.recipe.setup_script.clone(),
        },
        recipe_version: record.recipe_version.as_hex(),
        ready_preparation: record.ready_preparation.map(|id| id.as_hex()),
        latest_preparation: record.latest_preparation.as_hex(),
        created_at_ms: record.created_at_ms,
        updated_at_ms: record.updated_at_ms,
    }
}

fn preparation_to_file(record: &PreparationRecord) -> PreparationRecordFile {
    PreparationRecordFile {
        id: record.id.as_hex(),
        environment_id: record.environment_id.as_hex(),
        ordinal: record.ordinal,
        environment_revision: record.environment_revision,
        recipe_version: record.recipe_version.as_hex(),
        state: record.state.as_str().to_owned(),
        phase: record.phase.as_str().to_owned(),
        requested_at_ms: record.requested_at_ms,
        started_at_ms: record.started_at_ms,
        finished_at_ms: record.finished_at_ms,
        log: LogFile {
            captured_bytes: record.log.captured_bytes,
            truncated: record.log.truncated,
        },
        failure: record
            .failure
            .map(|failure| failure.category.as_str().to_owned()),
        snapshot: record.snapshot.as_ref().map(snapshot_to_file),
    }
}

fn snapshot_to_file(snapshot: &PreparedSnapshot) -> SnapshotFile {
    SnapshotFile {
        artifact_key: snapshot.artifact_key.as_str().to_owned(),
        snapshot_digest: snapshot.snapshot_digest.as_str().to_owned(),
        image_reference: snapshot.image_reference.clone(),
        image_manifest_digest: snapshot.image_manifest_digest.as_str().to_owned(),
        upper_integrity: IntegrityFile {
            algorithm: snapshot.upper_integrity.algorithm.clone(),
            value: snapshot.upper_integrity.value.clone(),
        },
        upper_size_bytes: snapshot.upper_size_bytes,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn lock_mutex<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
