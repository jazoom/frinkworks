use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::environments::EnvironmentId;

use super::definition::{
    DefinitionFile, DefinitionVersion, MAXIMUM_NAME_BYTES, PinnedWorkflowDefinition, StepAction,
    WorkflowDefinition,
};
use super::id::{IdError, WorkflowId};
use super::run::now_ms;
use super::seeds::{SeedKey, WorkflowSeed};

pub(crate) const CATALOGUE_FILE_VERSION: u32 = 1;
pub(crate) const MAXIMUM_WORKFLOWS: usize = 32;
pub(crate) const MAXIMUM_RETIRED_IDS: usize = 4_096;
pub(crate) const MAXIMUM_APPLIED_SEEDS: usize = 64;
pub(crate) const MAXIMUM_CATALOGUE_BYTES: usize = 24 * 1024 * 1024;
pub(crate) const SELECTION_TOKEN_BYTES: usize = 97;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkflowRecord {
    pub(crate) id: WorkflowId,
    pub(crate) revision: u64,
    pub(crate) definition_version: DefinitionVersion,
    pub(crate) definition: WorkflowDefinition,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WorkflowSelection {
    pub(crate) workflow_id: WorkflowId,
    pub(crate) definition_version: DefinitionVersion,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedWorkflow {
    pub(crate) record_revision: u64,
    pub(crate) pinned: PinnedWorkflowDefinition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CatalogueError {
    Corrupt,
    Persist,
    Random,
    Full,
    DuplicateName,
    Missing,
    Conflict,
    Revision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResolveWorkflowError {
    Missing,
    Changed,
    Invalid,
}

#[derive(Clone)]
struct AppliedWorkflowSeed {
    key: SeedKey,
    workflow_id: WorkflowId,
}

struct CatalogueState {
    applied_seeds: Vec<AppliedWorkflowSeed>,
    retired_workflow_ids: Vec<WorkflowId>,
    workflows: Vec<WorkflowRecord>,
}

pub(crate) struct WorkflowCatalogue {
    path: Option<PathBuf>,
    inner: Mutex<CatalogueState>,
    unavailable_starters: Vec<String>,
}

impl CatalogueError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Corrupt => "The workflow catalogue is unreadable.",
            Self::Persist => "Power Plant could not store that workflow. Try again.",
            Self::Random => "Power Plant could not create a workflow identifier. Try again.",
            Self::Full => "The workflow catalogue is full.",
            Self::DuplicateName => "A workflow with that name already exists.",
            Self::Missing => "That workflow is no longer in the catalogue.",
            Self::Conflict => "That workflow changed in another tab. Reload it.",
            Self::Revision => "Power Plant could not store another edit of that workflow.",
        }
    }
}

impl std::fmt::Display for CatalogueError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for CatalogueError {}

impl ResolveWorkflowError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Missing => "That workflow is no longer in the catalogue. Choose another.",
            Self::Changed => "That workflow changed. Choose the current version.",
            Self::Invalid => "That workflow is not valid. Choose another.",
        }
    }
}

impl WorkflowSelection {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        if value.len() != SELECTION_TOKEN_BYTES {
            return None;
        }
        let (id, rest) = value.split_at(32);
        let (separator, version) = rest.split_at(1);
        if separator != ":" {
            return None;
        }
        Some(Self {
            workflow_id: WorkflowId::parse(id)?,
            definition_version: DefinitionVersion::parse(version)?,
        })
    }

    pub(crate) fn as_token(self) -> String {
        format!(
            "{}:{}",
            self.workflow_id.as_hex(),
            self.definition_version.as_hex()
        )
    }
}

impl WorkflowCatalogue {
    pub(crate) fn open_with_seeds(
        path: PathBuf,
        seeds: &[WorkflowSeed],
    ) -> Result<Self, CatalogueError> {
        let mut state = load_path(&path)?;
        let seeded = apply_absent_seeds(&mut state, seeds)?;
        if seeded {
            persist(Some(&path), &state)?;
        }
        let unavailable_starters = unavailable_starter_names(&state, seeds);
        Ok(Self {
            path: Some(path),
            inner: Mutex::new(state),
            unavailable_starters,
        })
    }

    pub(crate) fn list(&self) -> Vec<WorkflowRecord> {
        let mut records = self.lock().workflows.clone();
        records.sort_by(|left, right| {
            left.definition
                .name()
                .to_lowercase()
                .cmp(&right.definition.name().to_lowercase())
                .then(left.id.cmp(&right.id))
        });
        records
    }

    pub(crate) fn get(&self, id: &WorkflowId) -> Option<WorkflowRecord> {
        self.lock()
            .workflows
            .iter()
            .find(|record| record.id == *id)
            .cloned()
    }

    pub(crate) fn unavailable_starters(&self) -> Vec<String> {
        self.unavailable_starters.clone()
    }

    pub(crate) fn referencing(&self, environment: &EnvironmentId) -> Vec<WorkflowRecord> {
        let mut records: Vec<_> = self
            .lock()
            .workflows
            .iter()
            .filter(|record| {
                record
                    .definition
                    .referenced_environments()
                    .contains(environment)
            })
            .cloned()
            .collect();
        records.sort_by(|left, right| {
            left.definition
                .name()
                .to_lowercase()
                .cmp(&right.definition.name().to_lowercase())
                .then(left.id.cmp(&right.id))
        });
        records
    }

    pub(crate) fn create(
        &self,
        draft: WorkflowDefinition,
    ) -> Result<WorkflowRecord, CatalogueError> {
        let mut state = self.lock();
        if state.workflows.len() >= MAXIMUM_WORKFLOWS {
            return Err(CatalogueError::Full);
        }
        reject_duplicate_name(&state.workflows, None, draft.name())?;
        let id = unused_identifier(&state)?;
        let now = now_ms();
        let record = WorkflowRecord {
            id,
            revision: 1,
            definition_version: draft.version(),
            definition: draft,
            created_at_ms: now,
            updated_at_ms: now,
        };
        let mut next = state.clone_state();
        next.workflows.push(record.clone());
        persist(self.path.as_deref(), &next)?;
        *state = next;
        Ok(record)
    }

    pub(crate) fn update(
        &self,
        id: &WorkflowId,
        expected_revision: u64,
        draft: WorkflowDefinition,
    ) -> Result<WorkflowRecord, CatalogueError> {
        let mut state = self.lock();
        let index = state
            .workflows
            .iter()
            .position(|record| record.id == *id)
            .ok_or(CatalogueError::Missing)?;
        let current = state.workflows[index].clone();
        if current.revision != expected_revision {
            return Err(CatalogueError::Conflict);
        }
        if current.definition == draft {
            return Ok(current);
        }
        reject_duplicate_name(&state.workflows, Some(*id), draft.name())?;
        let revision = current
            .revision
            .checked_add(1)
            .ok_or(CatalogueError::Revision)?;
        let record = WorkflowRecord {
            id: current.id,
            revision,
            definition_version: draft.version(),
            definition: draft,
            created_at_ms: current.created_at_ms,
            updated_at_ms: now_ms(),
        };
        let mut next = state.clone_state();
        next.workflows[index] = record.clone();
        persist(self.path.as_deref(), &next)?;
        *state = next;
        Ok(record)
    }

    pub(crate) fn delete(
        &self,
        id: &WorkflowId,
        expected_revision: u64,
    ) -> Result<(), CatalogueError> {
        let mut state = self.lock();
        let index = state
            .workflows
            .iter()
            .position(|record| record.id == *id)
            .ok_or(CatalogueError::Missing)?;
        if state.workflows[index].revision != expected_revision {
            return Err(CatalogueError::Conflict);
        }
        if state.retired_workflow_ids.len() >= MAXIMUM_RETIRED_IDS {
            return Err(CatalogueError::Full);
        }
        let mut next = state.clone_state();
        next.workflows.remove(index);
        next.retired_workflow_ids.push(*id);
        persist(self.path.as_deref(), &next)?;
        *state = next;
        Ok(())
    }

    pub(crate) fn resolve(
        &self,
        selection: &WorkflowSelection,
    ) -> Result<ResolvedWorkflow, ResolveWorkflowError> {
        let state = self.lock();
        let Some(record) = state
            .workflows
            .iter()
            .find(|record| record.id == selection.workflow_id)
        else {
            return Err(ResolveWorkflowError::Missing);
        };
        if record.definition_version != selection.definition_version {
            return Err(ResolveWorkflowError::Changed);
        }
        let definition = WorkflowDefinition::from_file(record.definition.to_file())
            .map_err(|_| ResolveWorkflowError::Invalid)?;
        if definition.version() != record.definition_version {
            return Err(ResolveWorkflowError::Invalid);
        }
        Ok(ResolvedWorkflow {
            record_revision: record.revision,
            pinned: PinnedWorkflowDefinition {
                workflow_id: Some(record.id),
                version: record.definition_version,
                definition,
            },
        })
    }

    fn lock(&self) -> MutexGuard<'_, CatalogueState> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl CatalogueState {
    fn clone_state(&self) -> Self {
        Self {
            applied_seeds: self.applied_seeds.clone(),
            retired_workflow_ids: self.retired_workflow_ids.clone(),
            workflows: self.workflows.clone(),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct CatalogueFile {
    file_version: u32,
    applied_seeds: Vec<AppliedSeedFile>,
    retired_workflow_ids: Vec<String>,
    workflows: Vec<WorkflowRecordFile>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct AppliedSeedFile {
    key: String,
    workflow_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct WorkflowRecordFile {
    id: String,
    revision: u64,
    definition_version: String,
    definition: DefinitionFile,
    created_at_ms: u64,
    updated_at_ms: u64,
}

fn empty_state() -> CatalogueState {
    CatalogueState {
        applied_seeds: Vec::new(),
        retired_workflow_ids: Vec::new(),
        workflows: Vec::new(),
    }
}

fn load_path(path: &Path) -> Result<CatalogueState, CatalogueError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(empty_state()),
        Err(_) => return Err(CatalogueError::Corrupt),
    };
    if bytes.len() > MAXIMUM_CATALOGUE_BYTES {
        return Err(CatalogueError::Corrupt);
    }
    let file: CatalogueFile =
        serde_json::from_slice(&bytes).map_err(|_| CatalogueError::Corrupt)?;
    state_from_file(file)
}

fn state_from_file(file: CatalogueFile) -> Result<CatalogueState, CatalogueError> {
    if file.file_version != CATALOGUE_FILE_VERSION {
        return Err(CatalogueError::Corrupt);
    }
    if file.workflows.len() > MAXIMUM_WORKFLOWS
        || file.retired_workflow_ids.len() > MAXIMUM_RETIRED_IDS
        || file.applied_seeds.len() > MAXIMUM_APPLIED_SEEDS
    {
        return Err(CatalogueError::Corrupt);
    }
    let mut retired_workflow_ids = Vec::new();
    for raw in file.retired_workflow_ids {
        let id = WorkflowId::parse(&raw).ok_or(CatalogueError::Corrupt)?;
        if retired_workflow_ids.contains(&id) {
            return Err(CatalogueError::Corrupt);
        }
        retired_workflow_ids.push(id);
    }
    let mut workflows = Vec::new();
    for record in file.workflows {
        let loaded = record_from_file(record)?;
        if retired_workflow_ids.contains(&loaded.id)
            || workflows
                .iter()
                .any(|item: &WorkflowRecord| item.id == loaded.id)
        {
            return Err(CatalogueError::Corrupt);
        }
        workflows.push(loaded);
    }
    let mut applied_seeds = Vec::new();
    let mut seed_keys = Vec::new();
    let mut seed_ids = Vec::new();
    for seed in file.applied_seeds {
        let key = SeedKey::parse(&seed.key).ok_or(CatalogueError::Corrupt)?;
        let workflow_id = WorkflowId::parse(&seed.workflow_id).ok_or(CatalogueError::Corrupt)?;
        if seed_keys.contains(&key) || seed_ids.contains(&workflow_id) {
            return Err(CatalogueError::Corrupt);
        }
        let known = workflows.iter().any(|record| record.id == workflow_id)
            || retired_workflow_ids.contains(&workflow_id);
        if !known {
            return Err(CatalogueError::Corrupt);
        }
        seed_keys.push(key.clone());
        seed_ids.push(workflow_id);
        applied_seeds.push(AppliedWorkflowSeed { key, workflow_id });
    }
    Ok(CatalogueState {
        applied_seeds,
        retired_workflow_ids,
        workflows,
    })
}

fn record_from_file(file: WorkflowRecordFile) -> Result<WorkflowRecord, CatalogueError> {
    if file.revision == 0 || file.updated_at_ms < file.created_at_ms {
        return Err(CatalogueError::Corrupt);
    }
    let id = WorkflowId::parse(&file.id).ok_or(CatalogueError::Corrupt)?;
    let stored_version =
        DefinitionVersion::parse(&file.definition_version).ok_or(CatalogueError::Corrupt)?;
    let definition =
        WorkflowDefinition::from_file(file.definition).map_err(|_| CatalogueError::Corrupt)?;
    if definition.version() != stored_version {
        return Err(CatalogueError::Corrupt);
    }
    Ok(WorkflowRecord {
        id,
        revision: file.revision,
        definition_version: stored_version,
        definition,
        created_at_ms: file.created_at_ms,
        updated_at_ms: file.updated_at_ms,
    })
}

fn apply_absent_seeds(
    state: &mut CatalogueState,
    seeds: &[WorkflowSeed],
) -> Result<bool, CatalogueError> {
    let mut changed = false;
    let mut seen_keys = Vec::new();
    for seed in seeds {
        if seen_keys.contains(&seed.key) {
            return Err(CatalogueError::Corrupt);
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
            || state.workflows.len() >= MAXIMUM_WORKFLOWS
        {
            continue;
        }
        let definition = named_seed_definition(&state.workflows, &seed.definition)?;
        let id = unused_identifier(state)?;
        let now = now_ms();
        state.workflows.push(WorkflowRecord {
            id,
            revision: 1,
            definition_version: definition.version(),
            definition,
            created_at_ms: now,
            updated_at_ms: now,
        });
        state.applied_seeds.push(AppliedWorkflowSeed {
            key: seed.key.clone(),
            workflow_id: id,
        });
        match encode_state(state) {
            Ok(_) => changed = true,
            Err(CatalogueError::Full) => {
                state.workflows.pop();
                state.applied_seeds.pop();
            }
            Err(error) => return Err(error),
        }
    }
    Ok(changed)
}

fn unavailable_starter_names(state: &CatalogueState, seeds: &[WorkflowSeed]) -> Vec<String> {
    seeds
        .iter()
        .filter(|seed| {
            !state
                .applied_seeds
                .iter()
                .any(|applied| applied.key == seed.key)
        })
        .map(|seed| seed.definition.name().to_owned())
        .collect()
}

fn unused_identifier(state: &CatalogueState) -> Result<WorkflowId, CatalogueError> {
    for _ in 0..16 {
        let id = WorkflowId::generate().map_err(|error| match error {
            IdError::RandomUnavailable => CatalogueError::Random,
        })?;
        if !identifier_taken(state, &id) {
            return Ok(id);
        }
    }
    Err(CatalogueError::Random)
}

fn identifier_taken(state: &CatalogueState, id: &WorkflowId) -> bool {
    state.workflows.iter().any(|record| record.id == *id)
        || state.retired_workflow_ids.contains(id)
        || state
            .applied_seeds
            .iter()
            .any(|seed| seed.workflow_id == *id)
}

fn reject_duplicate_name(
    workflows: &[WorkflowRecord],
    current: Option<WorkflowId>,
    name: &str,
) -> Result<(), CatalogueError> {
    if name_taken(workflows, current, name) {
        return Err(CatalogueError::DuplicateName);
    }
    Ok(())
}

fn name_taken(workflows: &[WorkflowRecord], current: Option<WorkflowId>, name: &str) -> bool {
    workflows.iter().any(|record| {
        current != Some(record.id) && record.definition.name().eq_ignore_ascii_case(name)
    })
}

const MAXIMUM_SEED_NAME_SUFFIX: u32 = 99;

fn named_seed_definition(
    workflows: &[WorkflowRecord],
    definition: &WorkflowDefinition,
) -> Result<WorkflowDefinition, CatalogueError> {
    let name = unique_seed_name(workflows, definition.name())?;
    if name == definition.name() {
        return Ok(definition.clone());
    }
    WorkflowDefinition::from_parts(
        name,
        definition.default_environment(),
        definition.roles().to_vec(),
        definition.steps().to_vec(),
    )
    .map_err(|_| CatalogueError::Corrupt)
}

fn unique_seed_name(workflows: &[WorkflowRecord], base: &str) -> Result<String, CatalogueError> {
    if !name_taken(workflows, None, base) {
        return Ok(base.to_owned());
    }
    for suffix in 2..=MAXIMUM_SEED_NAME_SUFFIX {
        let name = format!("{base} {suffix}");
        if name.len() > MAXIMUM_NAME_BYTES {
            break;
        }
        if !name_taken(workflows, None, &name) {
            return Ok(name);
        }
    }
    Err(CatalogueError::DuplicateName)
}

fn persist(path: Option<&Path>, state: &CatalogueState) -> Result<(), CatalogueError> {
    let Some(path) = path else {
        return Ok(());
    };
    let bytes = encode_state(state)?;
    let dir = path.parent().ok_or(CatalogueError::Persist)?;
    crate::storage::ensure_private_dir(dir).map_err(|_| CatalogueError::Persist)?;
    crate::storage::write_private(path, &bytes).map_err(|_| CatalogueError::Persist)
}

fn encode_state(state: &CatalogueState) -> Result<Vec<u8>, CatalogueError> {
    let file = CatalogueFile {
        file_version: CATALOGUE_FILE_VERSION,
        applied_seeds: state
            .applied_seeds
            .iter()
            .map(|seed| AppliedSeedFile {
                key: seed.key.as_str().to_owned(),
                workflow_id: seed.workflow_id.as_hex(),
            })
            .collect(),
        retired_workflow_ids: state
            .retired_workflow_ids
            .iter()
            .map(WorkflowId::as_hex)
            .collect(),
        workflows: state.workflows.iter().map(record_to_file).collect(),
    };
    let bytes = serde_json::to_vec_pretty(&file).map_err(|_| CatalogueError::Persist)?;
    if bytes.len() > MAXIMUM_CATALOGUE_BYTES {
        return Err(CatalogueError::Full);
    }
    Ok(bytes)
}

fn record_to_file(record: &WorkflowRecord) -> WorkflowRecordFile {
    WorkflowRecordFile {
        id: record.id.as_hex(),
        revision: record.revision,
        definition_version: record.definition_version.as_hex(),
        definition: record.definition.to_file(),
        created_at_ms: record.created_at_ms,
        updated_at_ms: record.updated_at_ms,
    }
}

pub(crate) fn definition_fits_agent(
    definition: &WorkflowDefinition,
    tools: &[crate::agents::ToolId],
    directories: &[(String, crate::agents::AccessMode)],
    primary_directory: &str,
) -> bool {
    definition.steps().iter().all(|step| match &step.action {
        StepAction::Agent(action) => {
            let primary_fits = directories.iter().any(|(alias, access)| {
                alias == primary_directory
                    && (!action.candidate_authority.access().is_writable() || access.is_writable())
            });
            primary_fits
                && !action
                    .authority
                    .directories
                    .iter()
                    .any(|directory| directory.alias == primary_directory)
                && action.authority.allowed_by(
                    tools,
                    directories
                        .iter()
                        .map(|(alias, access)| (alias.as_str(), *access)),
                )
        }
        StepAction::SystemCommand(action) => {
            action.command.contract().source_effect
                == super::commands::CommandSourceEffect::ReadOnly
                || directories
                    .iter()
                    .any(|(alias, access)| alias == primary_directory && access.is_writable())
        }
        StepAction::HumanGate(_) => true,
    })
}
