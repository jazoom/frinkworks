use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

use super::{SKILL_FILE_NAME, SkillError, SkillRecord};
use crate::execution::resources::MAXIMUM_SKILL_BODY_BYTES;
use crate::execution::resources::MAXIMUM_SKILLS;

#[cfg(test)]
mod tests;

pub(crate) struct SkillStore {
    dir: PathBuf,
    mutation: Mutex<()>,
    #[cfg(test)]
    _temporary: Option<tempfile::TempDir>,
}

impl SkillStore {
    pub(crate) fn open(dir: PathBuf) -> Result<Self, SkillError> {
        let new = !dir.try_exists().map_err(|_| SkillError::Persist)?;
        crate::storage::ensure_private_dir(&dir).map_err(|_| SkillError::Unsafe)?;
        let store = Self {
            dir,
            mutation: Mutex::new(()),
            #[cfg(test)]
            _temporary: None,
        };
        // Seed only a new directory. Deleted defaults and user files stay untouched on restart.
        if new {
            for markdown in [
                include_str!("defaults/code-review/SKILL.md"),
                include_str!("defaults/debugging/SKILL.md"),
                include_str!("defaults/test-design/SKILL.md"),
            ] {
                store.create(markdown.to_owned())?;
            }
        }
        Ok(store)
    }

    pub(crate) fn host_dir(&self) -> Option<&Path> {
        Some(&self.dir)
    }

    pub(crate) fn list(&self) -> Result<Vec<SkillRecord>, SkillError> {
        let _guard = self.lock();
        self.read_all()
    }

    pub(crate) fn get(&self, directory: &str) -> Result<SkillRecord, SkillError> {
        let _guard = self.lock();
        self.read(directory)
    }

    pub(crate) fn create(&self, markdown: String) -> Result<SkillRecord, SkillError> {
        let mut record = SkillRecord::parse(String::new(), markdown)?;
        let directory = record
            .name
            .to_ascii_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>();
        record.directory = directory.trim_matches('-').to_owned();
        if record.directory.is_empty() {
            record.directory = "skill".to_owned();
        }
        let _guard = self.lock();
        if self.entry_count()? >= MAXIMUM_SKILLS {
            return Err(SkillError::Full);
        }
        let path = self.path(&record.directory)?;
        fs::create_dir(&path).map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                SkillError::Conflict
            } else {
                SkillError::Persist
            }
        })?;
        if let Err(error) = self.persist(&record) {
            let _ = fs::remove_dir(&path);
            return Err(error);
        }
        Ok(record)
    }

    pub(crate) fn update(
        &self,
        directory: &str,
        fingerprint: &str,
        markdown: String,
    ) -> Result<SkillRecord, SkillError> {
        let record = SkillRecord::parse(directory.to_owned(), markdown)?;
        let _guard = self.lock();
        self.require_current(directory, fingerprint)?;
        self.persist(&record)?;
        Ok(record)
    }

    pub(crate) fn delete(&self, directory: &str, fingerprint: &str) -> Result<(), SkillError> {
        let _guard = self.lock();
        self.require_current(directory, fingerprint)?;
        crate::storage::remove_tree_nofollow(&self.path(directory)?)
            .map_err(|_| SkillError::Persist)
    }

    fn require_current(&self, directory: &str, fingerprint: &str) -> Result<(), SkillError> {
        if self.read(directory)?.fingerprint != fingerprint {
            return Err(SkillError::Conflict);
        }
        Ok(())
    }

    fn path(&self, directory: &str) -> Result<PathBuf, SkillError> {
        if directory.is_empty()
            || directory.len() > 128
            || matches!(directory, "." | "..")
            || !directory
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        {
            return Err(SkillError::Directory);
        }
        Ok(self.dir.join(directory))
    }

    fn entry_count(&self) -> Result<usize, SkillError> {
        regular_directory(&self.dir)?;
        let entries = fs::read_dir(&self.dir).map_err(|_| SkillError::Persist)?;
        let mut count = 0;
        for entry in entries.take(MAXIMUM_SKILLS + 1) {
            entry.map_err(|_| SkillError::Persist)?;
            count += 1;
        }
        Ok(count)
    }

    fn read_all(&self) -> Result<Vec<SkillRecord>, SkillError> {
        regular_directory(&self.dir)?;
        let mut records = Vec::new();
        for (index, entry) in fs::read_dir(&self.dir)
            .map_err(|_| SkillError::Persist)?
            .enumerate()
        {
            if index >= MAXIMUM_SKILLS {
                return Err(SkillError::Full);
            }
            let entry = entry.map_err(|_| SkillError::Persist)?;
            let kind = entry.file_type().map_err(|_| SkillError::Persist)?;
            if kind.is_symlink() {
                return Err(SkillError::Unsafe);
            }
            if !kind.is_dir() {
                continue;
            }
            let directory = entry
                .file_name()
                .into_string()
                .map_err(|_| SkillError::Directory)?;
            match self.read(&directory) {
                Ok(record) => records.push(record),
                Err(SkillError::Missing) => {}
                Err(error) => return Err(error),
            }
        }
        records.sort_by(|left, right| left.directory.cmp(&right.directory));
        Ok(records)
    }

    fn read(&self, directory: &str) -> Result<SkillRecord, SkillError> {
        regular_directory(&self.dir)?;
        let path = self.path(directory)?;
        regular_directory(&path)?;
        let file = path.join(SKILL_FILE_NAME);
        let metadata = fs::symlink_metadata(&file).map_err(read_error)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(SkillError::Unsafe);
        }
        if metadata.len() > MAXIMUM_SKILL_BODY_BYTES as u64 {
            return Err(SkillError::Bound);
        }
        let bytes = crate::storage::read_private_bounded(&file, MAXIMUM_SKILL_BODY_BYTES)
            .map_err(|_| SkillError::Persist)?;
        let markdown = String::from_utf8(bytes).map_err(|_| SkillError::Metadata)?;
        SkillRecord::parse(directory.to_owned(), markdown)
    }

    fn persist(&self, record: &SkillRecord) -> Result<(), SkillError> {
        regular_directory(&self.dir)?;
        let path = self.path(&record.directory)?;
        crate::storage::ensure_private_dir(&path).map_err(|_| SkillError::Unsafe)?;
        crate::storage::write_private(&path.join(SKILL_FILE_NAME), record.markdown.as_bytes())
            .map_err(|_| SkillError::Persist)
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.mutation
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

fn regular_directory(path: &Path) -> Result<(), SkillError> {
    let metadata = fs::symlink_metadata(path).map_err(read_error)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(SkillError::Unsafe);
    }
    Ok(())
}

fn read_error(error: io::Error) -> SkillError {
    if error.kind() == io::ErrorKind::NotFound {
        SkillError::Missing
    } else {
        SkillError::Persist
    }
}
