use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use super::id::AgentId;
use super::record::{AgentDraft, AgentError, AgentFile, AgentRecord, MAXIMUM_AGENTS};
#[cfg(test)]
mod tests;

pub(crate) struct AgentStore {
    dir: Option<PathBuf>,
    inner: Mutex<BTreeMap<AgentId, AgentRecord>>,
}

impl AgentStore {
    pub(crate) fn open(dir: PathBuf) -> Result<Self, AgentError> {
        fs::create_dir_all(&dir).map_err(|_| AgentError::Persist)?;
        let agents = load_dir(&dir)?;
        Ok(Self {
            dir: Some(dir),
            inner: Mutex::new(agents),
        })
    }

    pub(crate) fn list(&self) -> Vec<AgentRecord> {
        self.lock().values().cloned().collect()
    }

    pub(crate) fn get(&self, id: &AgentId) -> Option<AgentRecord> {
        self.lock().get(id).cloned()
    }

    pub(crate) fn create(&self, draft: AgentDraft) -> Result<AgentRecord, AgentError> {
        let draft = draft.validate()?;
        let mut agents = self.lock();
        insert_created(self.dir.as_deref(), &mut agents, draft)
    }

    pub(crate) fn update(
        &self,
        id: &AgentId,
        expected_revision: u32,
        draft: AgentDraft,
    ) -> Result<AgentRecord, AgentError> {
        let draft = draft.validate()?;
        let mut agents = self.lock();
        let Some(current) = agents.get(id).cloned() else {
            return Err(AgentError::Missing);
        };
        if current.revision != expected_revision {
            return Err(AgentError::Conflict);
        }
        let revision = current
            .revision
            .checked_add(1)
            .ok_or(AgentError::Revision)?;
        let record = AgentRecord {
            id: current.id,
            revision,
            name: draft.name,
            instructions: draft.instructions,
            selection: draft.selection,
            tools: draft.tools,
            network: draft.network,
            directories: draft.directories,
            primary_directory: draft.primary_directory,
        };
        persist(self.dir.as_deref(), &record)?;
        agents.insert(record.id, record.clone());
        Ok(record)
    }

    pub(crate) fn delete(&self, id: &AgentId, expected_revision: u32) -> Result<(), AgentError> {
        let mut agents = self.lock();
        let Some(current) = agents.get(id) else {
            return Err(AgentError::Missing);
        };
        if current.revision != expected_revision {
            return Err(AgentError::Conflict);
        }
        if let Some(dir) = &self.dir {
            remove_file(&dir.join(format!("{}.json", id.as_hex())))?;
        }
        agents.remove(id);
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<AgentId, AgentRecord>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn load_dir(dir: &Path) -> Result<BTreeMap<AgentId, AgentRecord>, AgentError> {
    let mut agents = BTreeMap::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(agents),
        Err(_) => return Err(AgentError::Persist),
    };
    for entry in entries {
        let entry = entry.map_err(|_| AgentError::Persist)?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let bytes = fs::read(&path).map_err(|_| AgentError::Corrupt)?;
        let file: AgentFile = serde_json::from_slice(&bytes).map_err(|_| AgentError::Corrupt)?;
        let record = AgentRecord::from_file(file)?;
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or(AgentError::Corrupt)?;
        if stem != record.id.as_hex() {
            return Err(AgentError::Corrupt);
        }
        if agents.len() >= MAXIMUM_AGENTS || agents.insert(record.id, record).is_some() {
            return Err(AgentError::Corrupt);
        }
    }
    Ok(agents)
}

fn insert_created(
    dir: Option<&Path>,
    agents: &mut BTreeMap<AgentId, AgentRecord>,
    draft: AgentDraft,
) -> Result<AgentRecord, AgentError> {
    if agents.len() >= MAXIMUM_AGENTS {
        return Err(AgentError::Full);
    }
    let id = AgentId::generate().map_err(|_| AgentError::Random)?;
    let record = AgentRecord {
        id,
        revision: 1,
        name: draft.name,
        instructions: draft.instructions,
        selection: draft.selection,
        tools: draft.tools,
        network: draft.network,
        directories: draft.directories,
        primary_directory: draft.primary_directory,
    };
    persist(dir, &record)?;
    agents.insert(record.id, record.clone());
    Ok(record)
}

fn persist(dir: Option<&Path>, record: &AgentRecord) -> Result<(), AgentError> {
    let Some(dir) = dir else {
        return Ok(());
    };
    fs::create_dir_all(dir).map_err(|_| AgentError::Persist)?;
    let path = dir.join(format!("{}.json", record.id.as_hex()));
    let bytes = serde_json::to_vec_pretty(&record.to_file()).map_err(|_| AgentError::Persist)?;
    crate::storage::write_private(&path, &bytes).map_err(|_| AgentError::Persist)
}

fn remove_file(path: &Path) -> Result<(), AgentError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(AgentError::Persist),
    }
}
