#[cfg(test)]
mod tests;

use std::path::PathBuf;

use crate::storage::{self, PersistError};
use crate::workflows::id::{AttemptId, RunId};

pub(crate) struct CommitJournals {
    root: PathBuf,
    #[cfg(test)]
    _hold: Option<tempfile::TempDir>,
}

pub(crate) struct CommitJournal {
    pub(crate) root: PathBuf,
}

impl CommitJournals {
    pub(crate) fn open(root: PathBuf) -> Result<Self, PersistError> {
        storage::ensure_private_dir(&root)?;
        Ok(Self {
            root,
            #[cfg(test)]
            _hold: None,
        })
    }

    pub(crate) fn create(
        &self,
        run: RunId,
        attempt: AttemptId,
    ) -> Result<CommitJournal, PersistError> {
        let run_dir = storage::confined_child(&self.root, &run.as_hex())?;
        storage::ensure_private_dir(&run_dir)?;
        let root = storage::confined_child(&run_dir, &attempt.as_hex())?;
        storage::ensure_private_dir(&root)?;
        Ok(CommitJournal { root })
    }

    pub(crate) fn path(&self, run: RunId, attempt: AttemptId) -> Result<PathBuf, PersistError> {
        let run_dir = storage::confined_child(&self.root, &run.as_hex())?;
        storage::confined_child(&run_dir, &attempt.as_hex())
    }

    pub(crate) fn load(
        &self,
        run: RunId,
        attempt: AttemptId,
    ) -> Result<CommitJournal, PersistError> {
        let root = self.path(run, attempt)?;
        if !root.is_dir() {
            return Err(PersistError);
        }
        Ok(CommitJournal { root })
    }

    pub(crate) fn create_for_directory(
        &self,
        run: RunId,
        attempt: AttemptId,
        directory: crate::execution::DirectoryGrantId,
    ) -> Result<CommitJournal, PersistError> {
        let parent = self.create(run, attempt)?;
        let root = storage::confined_child(&parent.root, &directory.as_hex())?;
        storage::ensure_private_dir(&root)?;
        Ok(CommitJournal { root })
    }

    pub(crate) fn load_for_directory(
        &self,
        run: RunId,
        attempt: AttemptId,
        directory: crate::execution::DirectoryGrantId,
    ) -> Result<CommitJournal, PersistError> {
        let parent = self.load(run, attempt)?;
        let root = storage::confined_child(&parent.root, &directory.as_hex())?;
        if !root.is_dir() {
            return Err(PersistError);
        }
        Ok(CommitJournal { root })
    }

    pub(crate) fn remove(&self, run: RunId, attempt: AttemptId) -> Result<(), PersistError> {
        let path = self.path(run, attempt)?;
        if !path.exists() {
            return Ok(());
        }
        storage::remove_tree_nofollow(&path).map_err(|_| PersistError)
    }
}

impl CommitJournal {
    pub(crate) fn write_index_backup(&self, name: &str, bytes: &[u8]) -> Result<(), PersistError> {
        let path = self.index_path(name)?;
        storage::write_private(&path, bytes)
    }

    pub(crate) fn read_index_backup(&self, name: &str) -> Result<Vec<u8>, PersistError> {
        let path = self.index_path(name)?;
        std::fs::read(path).map_err(|_| PersistError)
    }

    fn index_path(&self, name: &str) -> Result<PathBuf, PersistError> {
        if !matches!(name, "original.index" | "target.index") {
            return Err(PersistError);
        }
        storage::confined_child(&self.root, name)
    }

    pub(crate) fn flush(&self) -> Result<(), PersistError> {
        storage::ensure_private_dir(&self.root)?;
        let dir = std::fs::File::open(&self.root).map_err(|_| PersistError)?;
        dir.sync_all().map_err(|_| PersistError)
    }
}
