//! Persistent global prompt templates in the data-directory `prompts` folder.
//!
//! A template is one Markdown file directly inside the folder. The file name
//! supplies the command name. The store validates each write with the same
//! parser that discovery uses, so a saved file is selectable in the composer.
//! Writes are private and atomic. Symbolic links are rejected.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

use super::{
    MAXIMUM_PROMPT_BODY_BYTES, MAXIMUM_PROMPT_ENTRIES, MAXIMUM_PROMPTS, PROMPT_EXTENSION,
    PromptError, parse_document, reserved_name, valid_name,
};
use crate::execution::resources::content_hash;

#[cfg(test)]
mod tests;

pub(crate) struct PromptStore {
    dir: PathBuf,
    mutation: Mutex<()>,
    #[cfg(test)]
    _temporary: Option<tempfile::TempDir>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PromptRecord {
    pub(crate) name: String,
    pub(crate) fingerprint: String,
    pub(crate) description: String,
    pub(crate) argument_hint: String,
    pub(crate) body: String,
    pub(crate) markdown: String,
    pub(crate) problem: Option<PromptStoreError>,
}

#[derive(Default, Debug, Eq, PartialEq)]
pub(crate) struct PromptListing {
    pub(crate) records: Vec<PromptRecord>,
    pub(crate) unavailable: Vec<(String, PromptStoreError)>,
}

impl PromptStore {
    pub(crate) fn open(dir: PathBuf) -> Result<Self, PromptStoreError> {
        crate::storage::ensure_private_dir(&dir).map_err(|_| PromptStoreError::Unsafe)?;
        Ok(Self {
            dir,
            mutation: Mutex::new(()),
            #[cfg(test)]
            _temporary: None,
        })
    }

    pub(crate) fn host_dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn list(&self) -> Result<PromptListing, PromptStoreError> {
        let _guard = self.lock();
        let mut listing = PromptListing::default();
        for entry in self.entries()? {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let Some(name) = file_name.strip_suffix(&format!(".{PROMPT_EXTENSION}")) else {
                continue;
            };
            if !valid_name(name) || reserved_name(name) {
                listing
                    .unavailable
                    .push((file_name, PromptStoreError::Name));
                continue;
            }
            match self.read(name) {
                Ok(record) => listing.records.push(record),
                Err(error) => listing.unavailable.push((file_name, error)),
            }
            if listing.records.len() > MAXIMUM_PROMPTS {
                return Err(PromptStoreError::Full);
            }
        }
        let mut names = std::collections::BTreeSet::new();
        let mut duplicates = std::collections::BTreeSet::new();
        for record in &listing.records {
            if record.problem.is_none() && !names.insert(record.name.to_ascii_lowercase()) {
                duplicates.insert(record.name.to_ascii_lowercase());
            }
        }
        for record in &mut listing.records {
            if duplicates.contains(&record.name.to_ascii_lowercase()) {
                record.problem = Some(PromptStoreError::Conflict);
            }
        }
        listing
            .records
            .sort_by(|left, right| left.name.cmp(&right.name));
        listing
            .unavailable
            .sort_by(|left, right| left.0.cmp(&right.0));
        Ok(listing)
    }

    pub(crate) fn get(&self, name: &str) -> Result<PromptRecord, PromptStoreError> {
        let _guard = self.lock();
        self.read(name)
    }

    /// Create one new template. An existing file is never replaced, so a
    /// duplicate name reports a conflict instead of an overwrite.
    pub(crate) fn create(
        &self,
        name: &str,
        markdown: String,
    ) -> Result<PromptRecord, PromptStoreError> {
        let record = parse_record(name, markdown)?;
        let _guard = self.lock();
        let entries = self.entries()?;
        let mut templates = 0;
        for entry in &entries {
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            let Some(existing) = file_name.strip_suffix(&format!(".{PROMPT_EXTENSION}")) else {
                continue;
            };
            if existing.eq_ignore_ascii_case(name) {
                let kind = entry.file_type().map_err(|_| PromptStoreError::Persist)?;
                return Err(if kind.is_symlink() {
                    PromptStoreError::Unsafe
                } else {
                    PromptStoreError::Conflict
                });
            }
            if valid_name(existing) && !reserved_name(existing) {
                templates += 1;
            }
        }
        if templates >= MAXIMUM_PROMPTS || entries.len() >= MAXIMUM_PROMPT_ENTRIES {
            return Err(PromptStoreError::Full);
        }
        self.persist(&record)?;
        Ok(record)
    }

    pub(crate) fn update(
        &self,
        name: &str,
        fingerprint: &str,
        markdown: String,
    ) -> Result<PromptRecord, PromptStoreError> {
        let record = parse_record(name, markdown)?;
        let _guard = self.lock();
        self.require_current(name, fingerprint)?;
        self.persist(&record)?;
        Ok(record)
    }

    fn require_current(&self, name: &str, fingerprint: &str) -> Result<(), PromptStoreError> {
        if self.read(name)?.fingerprint != fingerprint {
            return Err(PromptStoreError::Conflict);
        }
        Ok(())
    }

    fn path(&self, name: &str) -> Result<PathBuf, PromptStoreError> {
        if !valid_name(name) || reserved_name(name) {
            return Err(PromptStoreError::Name);
        }
        Ok(self.dir.join(format!("{name}.{PROMPT_EXTENSION}")))
    }

    fn entries(&self) -> Result<Vec<fs::DirEntry>, PromptStoreError> {
        self.regular_dir()?;
        let entries = fs::read_dir(&self.dir)
            .map_err(|_| PromptStoreError::Persist)?
            .take(MAXIMUM_PROMPT_ENTRIES + 1)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| PromptStoreError::Persist)?;
        if entries.len() > MAXIMUM_PROMPT_ENTRIES {
            return Err(PromptStoreError::Full);
        }
        Ok(entries)
    }

    fn read(&self, name: &str) -> Result<PromptRecord, PromptStoreError> {
        self.regular_dir()?;
        let path = self.path(name)?;
        let metadata = fs::symlink_metadata(&path).map_err(read_error)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(PromptStoreError::Unsafe);
        }
        if metadata.len() > MAXIMUM_PROMPT_BODY_BYTES as u64 {
            return Err(PromptStoreError::Bound);
        }
        let bytes = crate::storage::read_private_bounded(&path, MAXIMUM_PROMPT_BODY_BYTES)
            .map_err(|_| PromptStoreError::Persist)?;
        let markdown = String::from_utf8(bytes).map_err(|_| PromptStoreError::Malformed)?;
        // A bounded regular UTF-8 file stays editable when its prompt syntax is
        // invalid. Its raw hash still protects repair against stale writes.
        match parse_record(name, markdown.clone()) {
            Ok(record) => Ok(record),
            Err(problem) => Ok(PromptRecord {
                name: name.to_owned(),
                fingerprint: content_hash(markdown.as_bytes()),
                description: String::new(),
                argument_hint: String::new(),
                body: markdown.clone(),
                markdown,
                problem: Some(problem),
            }),
        }
    }

    fn persist(&self, record: &PromptRecord) -> Result<(), PromptStoreError> {
        self.regular_dir()?;
        let path = self.path(&record.name)?;
        crate::storage::write_private(&path, record.markdown.as_bytes())
            .map_err(|_| PromptStoreError::Persist)
    }

    fn regular_dir(&self) -> Result<(), PromptStoreError> {
        let metadata = fs::symlink_metadata(&self.dir).map_err(read_error)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(PromptStoreError::Unsafe);
        }
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.mutation
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

fn parse_record(name: &str, markdown: String) -> Result<PromptRecord, PromptStoreError> {
    if !valid_name(name) || reserved_name(name) {
        return Err(PromptStoreError::Name);
    }
    if markdown.len() > MAXIMUM_PROMPT_BODY_BYTES {
        return Err(PromptStoreError::Bound);
    }
    let document = parse_document(&markdown).map_err(|error| match error {
        PromptError::Malformed => PromptStoreError::Malformed,
        PromptError::Bound => PromptStoreError::Bound,
    })?;
    Ok(PromptRecord {
        name: name.to_owned(),
        fingerprint: content_hash(markdown.as_bytes()),
        description: document.description,
        argument_hint: document.argument_hint,
        body: document.body,
        markdown,
        problem: None,
    })
}

fn read_error(error: io::Error) -> PromptStoreError {
    if error.kind() == io::ErrorKind::NotFound {
        PromptStoreError::Missing
    } else {
        PromptStoreError::Persist
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PromptStoreError {
    Persist,
    Unsafe,
    Full,
    Missing,
    Conflict,
    Bound,
    Malformed,
    Name,
}

impl PromptStoreError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Persist => {
                "Power Plant cannot read or store a prompt file. Make sure that the global prompt directory is accessible."
            }
            Self::Unsafe => "A prompt path is not a regular file. Symbolic links are not allowed.",
            Self::Full => "The prompt directory exceeds the limit of 64 templates or 256 files.",
            Self::Missing => "That prompt template does not exist.",
            Self::Conflict => {
                "That prompt template changed or its name is already used. Reload before you save."
            }
            Self::Bound => "The complete prompt file must not exceed 64 KiB.",
            Self::Malformed => {
                "Use a non-empty body. If a file starts a frontmatter block, it must close with ---."
            }
            Self::Name => {
                "Use a name of at most 128 letters, digits, hyphens, underscores or dots. Start the name with a letter or digit. The name skill is reserved."
            }
        }
    }
}
