use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use super::id::RunId;
use super::run::{RunRecordError, WorkflowRun, now_ms};
use crate::{conversations::ConversationId, projects::ProjectId};

pub(crate) const BROWSER_SUMMARY_LIMIT: usize = 50;

#[derive(Clone, Debug)]
pub(crate) struct RunSummary {
    pub(crate) id: RunId,
    pub(crate) project_id: Option<ProjectId>,
    pub(crate) name: String,
    pub(crate) state: String,
    pub(crate) created_at_ms: u64,
    pub(crate) current_step: String,
}

pub(crate) struct WorkflowRunStore {
    dir: Option<PathBuf>,
    inner: Mutex<BTreeMap<RunId, WorkflowRun>>,
    #[cfg(test)]
    fail_next_mutation: Mutex<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StoreError {
    Persist,
    Corrupt,
    Missing,
    Conflict,
}

impl StoreError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Persist => "Power Plant could not store the workflow run. Try again.",
            Self::Corrupt => "A workflow run record is unreadable.",
            Self::Missing => "That workflow run does not exist.",
            Self::Conflict => "Power Plant could not update that workflow run.",
        }
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for StoreError {}

impl WorkflowRunStore {
    pub(crate) fn open(dir: PathBuf) -> Result<Self, StoreError> {
        crate::storage::ensure_private_dir(&dir).map_err(|_| StoreError::Persist)?;
        let runs = load_dir(&dir)?;
        Ok(Self {
            dir: Some(dir),
            inner: Mutex::new(runs),
            #[cfg(test)]
            fail_next_mutation: Mutex::new(false),
        })
    }

    pub(crate) fn create(&self, run: WorkflowRun) -> Result<WorkflowRun, StoreError> {
        let mut runs = self.lock();
        if runs.contains_key(&run.id) {
            return Err(StoreError::Conflict);
        }
        persist(self.dir.as_deref(), &run)?;
        runs.insert(run.id, run.clone());
        Ok(run)
    }

    pub(crate) fn get(&self, id: &RunId) -> Option<WorkflowRun> {
        self.lock().get(id).cloned()
    }

    pub(crate) fn active_runs(&self) -> Vec<WorkflowRun> {
        self.lock()
            .values()
            .filter(|run| run.is_active())
            .cloned()
            .collect()
    }

    pub(crate) fn for_conversation(&self, conversation: &ConversationId) -> Vec<WorkflowRun> {
        let mut runs: Vec<_> = self
            .lock()
            .values()
            .filter(|run| run.conversation_id == Some(*conversation))
            .cloned()
            .collect();
        runs.sort_by(|left, right| {
            right
                .created_at_ms
                .cmp(&left.created_at_ms)
                .then(right.id.cmp(&left.id))
        });
        runs
    }

    pub(crate) fn interrupt_active(&self) -> Result<(), StoreError> {
        let mut runs = self.lock();
        interrupt_active(&mut runs, self.dir.as_deref())
    }

    pub(crate) fn pending_cleanup_attempts(&self) -> Vec<(RunId, super::id::AttemptId)> {
        self.lock()
            .values()
            .flat_map(|run| {
                run.attempts
                    .iter()
                    .filter(|attempt| {
                        matches!(attempt.cleanup, super::run::AttemptCleanupRecord::Pending)
                    })
                    .map(|attempt| (run.id, attempt.id))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    pub(crate) fn summaries(&self) -> Vec<RunSummary> {
        let mut summaries = self.all_summaries();
        summaries.truncate(BROWSER_SUMMARY_LIMIT);
        summaries
    }

    pub(crate) fn all_summaries(&self) -> Vec<RunSummary> {
        let mut summaries: Vec<RunSummary> = self.lock().values().map(summary_of).collect();
        summaries.sort_by(|left, right| {
            right
                .created_at_ms
                .cmp(&left.created_at_ms)
                .then(right.id.cmp(&left.id))
        });
        summaries
    }

    pub(crate) fn mutate<F>(&self, id: &RunId, op: F) -> Result<WorkflowRun, StoreError>
    where
        F: FnOnce(&mut WorkflowRun) -> Result<(), super::run::TransitionError>,
    {
        #[cfg(test)]
        {
            let mut failure = self
                .fail_next_mutation
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if *failure {
                *failure = false;
                return Err(StoreError::Persist);
            }
        }
        let mut runs = self.lock();
        let Some(current) = runs.get(id).cloned() else {
            return Err(StoreError::Missing);
        };
        let mut next = current.clone();
        op(&mut next).map_err(|_| StoreError::Conflict)?;
        // An ownership commit must reach the conversation catalogue before any execution transition.
        if current.pending_handoff.is_some() {
            let mut settled = current;
            settled.pending_handoff = None;
            if next != settled {
                return Err(StoreError::Conflict);
            }
        }
        if next.pending_handoff.is_some() {
            persist_ownership(self.dir.as_deref(), &next)?;
        } else {
            persist(self.dir.as_deref(), &next)?;
        }
        runs.insert(*id, next.clone());
        Ok(next)
    }

    pub(crate) fn flush_handoff(&self, expected: &WorkflowRun) -> Result<(), StoreError> {
        let runs = self.lock();
        if runs.get(&expected.id) != Some(expected) || expected.pending_handoff.is_none() {
            return Err(StoreError::Conflict);
        }
        persist(self.dir.as_deref(), expected)
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<RunId, WorkflowRun>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn summary_of(run: &WorkflowRun) -> RunSummary {
    RunSummary {
        id: run.id,
        project_id: run.project_id,
        name: run.pinned.definition.name().to_owned(),
        state: run.state.as_label().to_owned(),
        created_at_ms: run.created_at_ms,
        current_step: run.current_step_name().unwrap_or("").to_owned(),
    }
}

fn load_dir(dir: &Path) -> Result<BTreeMap<RunId, WorkflowRun>, StoreError> {
    let mut runs = BTreeMap::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(runs),
        Err(_) => return Err(StoreError::Persist),
    };
    for entry in entries {
        let entry = entry.map_err(|_| StoreError::Persist)?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let bytes = fs::read(&path).map_err(|_| StoreError::Corrupt)?;
        let file: super::run::RunFile =
            serde_json::from_slice(&bytes).map_err(|_| StoreError::Corrupt)?;
        let run = WorkflowRun::from_file(file).map_err(|error| match error {
            RunRecordError::Corrupt => StoreError::Corrupt,
        })?;
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or(StoreError::Corrupt)?;
        if stem != run.id.as_hex() {
            return Err(StoreError::Corrupt);
        }
        if runs.insert(run.id, run).is_some() {
            return Err(StoreError::Corrupt);
        }
    }
    Ok(runs)
}

fn interrupt_active(
    runs: &mut BTreeMap<RunId, WorkflowRun>,
    dir: Option<&Path>,
) -> Result<(), StoreError> {
    let at_ms = now_ms();
    for run in runs.values_mut() {
        if !run.is_active() || run.recoverable_gate() {
            continue;
        }
        run.interrupt(at_ms).map_err(|_| StoreError::Corrupt)?;
        persist(dir, run)?;
    }
    Ok(())
}

fn persist_ownership(dir: Option<&Path>, run: &WorkflowRun) -> Result<(), StoreError> {
    let Some(dir) = dir else { return Ok(()) };
    crate::storage::ensure_private_dir(dir).map_err(|_| StoreError::Persist)?;
    let path = dir.join(format!("{}.json", run.id.as_hex()));
    let bytes = serde_json::to_vec_pretty(&run.to_file()).map_err(|_| StoreError::Persist)?;
    match crate::storage::write_private_outcome(&path, &bytes) {
        Ok(()) | Err(crate::storage::PrivateWriteError::Replaced) => Ok(()),
        Err(crate::storage::PrivateWriteError::Unchanged) => Err(StoreError::Persist),
    }
}

fn persist(dir: Option<&Path>, run: &WorkflowRun) -> Result<(), StoreError> {
    let Some(dir) = dir else {
        return Ok(());
    };
    crate::storage::ensure_private_dir(dir).map_err(|_| StoreError::Persist)?;
    let path = dir.join(format!("{}.json", run.id.as_hex()));
    let bytes = serde_json::to_vec_pretty(&run.to_file()).map_err(|_| StoreError::Persist)?;
    crate::storage::write_private(&path, &bytes).map_err(|_| StoreError::Persist)
}

#[cfg(test)]
mod tests;
