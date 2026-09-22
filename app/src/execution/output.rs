//! Private retained command output.
//!
//! References are server-generated. A reader resolves a reference against the
//! active conversation or workflow attempt. The store never accepts a
//! model-supplied filesystem path.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use rand::rand_core::TryRng;
use rand::rngs::SysRng;
use serde::{Deserialize, Serialize};

use crate::conversations::ConversationId;
use crate::execution::command::{CommandChunk, CommandResult};
use crate::sessions::JobId;
use crate::workflows::{AttemptId, RunId};

pub(crate) const RETAINED_OUTPUT_VERSION: u32 = 1;
/// Per-command retained storage limit. It is independent of display limits.
pub(crate) const MAXIMUM_RETAINED_BYTES: usize = 256 * 1024;
/// Aggregate retained storage limit across every record.
pub(crate) const MAXIMUM_OUTPUT_STORE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAXIMUM_OUTPUT_REFERENCES: usize = 4096;
/// Bytes shown for the first page of a command result.
pub(crate) const OUTPUT_PREVIEW_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OutputError {
    Persist,
    Corrupt,
    Missing,
    Forbidden,
    Full,
    Cursor,
}

impl OutputError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Persist => "Power Plant could not store command output.",
            Self::Corrupt => "The retained command output is unreadable.",
            Self::Missing => "That command output is not available.",
            Self::Forbidden => "That command output belongs to another conversation.",
            Self::Full => "Power Plant cannot retain more command output.",
            Self::Cursor => "That output offset is not valid.",
        }
    }
}

/// The scope that owns a retained output. Reads compare the scope only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutputScope {
    pub(crate) conversation: Option<ConversationId>,
    pub(crate) run: Option<RunId>,
    pub(crate) attempt: Option<AttemptId>,
}

impl OutputScope {
    pub(crate) fn conversation(conversation: ConversationId) -> Self {
        Self {
            conversation: Some(conversation),
            run: None,
            attempt: None,
        }
    }

    /// A reader that names a conversation matches any record in it. A reader
    /// without a conversation must name the exact run and attempt.
    fn matches(&self, record: &OutputRecord) -> bool {
        if let Some(conversation) = self.conversation {
            if record.conversation.as_deref() != Some(conversation.to_string().as_str()) {
                return false;
            }
        } else if self.run.is_none() || self.attempt.is_none() {
            return false;
        }
        if let Some(run) = self.run
            && record.run.as_deref() != Some(run.to_string().as_str())
        {
            return false;
        }
        if let Some(attempt) = self.attempt
            && record.attempt.as_deref() != Some(attempt.to_string().as_str())
        {
            return false;
        }
        true
    }
}

/// Provenance for one stored command result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutputKey {
    pub(crate) scope: OutputScope,
    pub(crate) job: JobId,
    pub(crate) tool_call: String,
    /// True when the model must not read this record, even with the exact
    /// reference. Local output views remain available.
    pub(crate) model_hidden: bool,
}

/// Reference and truncation facts that travel with a command result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct RetainedOutput {
    pub(crate) reference: String,
    /// Retained text reached the per-command limit before the process ended.
    pub(crate) truncated: bool,
    pub(crate) bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutputPage {
    pub(crate) chunks: Vec<CommandChunk>,
    pub(crate) next: Option<u64>,
    pub(crate) truncated: bool,
    pub(crate) line_truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct OutputRecord {
    version: u32,
    reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    conversation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attempt: Option<String>,
    job: String,
    tool_call: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    model_hidden: bool,
    chunks: Vec<CommandChunk>,
    truncated: bool,
}

impl OutputRecord {
    fn bytes(&self) -> usize {
        self.chunks.iter().map(|chunk| chunk.text.len()).sum()
    }
}

pub(crate) struct OutputStore {
    dir: Option<PathBuf>,
    inner: Mutex<OutputInner>,
}

#[derive(Default)]
struct OutputInner {
    records: BTreeMap<String, OutputRecord>,
    bytes: usize,
}

impl OutputStore {
    pub(crate) fn open(dir: PathBuf) -> Result<Self, OutputError> {
        crate::storage::ensure_private_dir(&dir).map_err(|_| OutputError::Persist)?;
        let records = load_dir(&dir)?;
        let bytes = records.values().map(OutputRecord::bytes).sum();
        Ok(Self {
            dir: Some(dir),
            inner: Mutex::new(OutputInner { records, bytes }),
        })
    }

    /// An in-memory store for tests that do not touch the filesystem.
    #[cfg(test)]
    pub(crate) fn ephemeral() -> Self {
        Self {
            dir: None,
            inner: Mutex::new(OutputInner::default()),
        }
    }

    /// Store redacted ordered chunks. The caller redacts before this call.
    pub(crate) fn store(
        &self,
        key: &OutputKey,
        result: &CommandResult,
    ) -> Result<RetainedOutput, OutputError> {
        let (chunks, bounded) = bound_chunks(&result.chunks, MAXIMUM_RETAINED_BYTES);
        let truncated =
            bounded || result.termination == crate::execution::CommandTermination::ResourceLimit;
        let bytes = chunks.iter().map(|chunk| chunk.text.len()).sum::<usize>();
        let mut inner = self.lock();
        if !inner.records.is_empty()
            && (inner.records.len() >= MAXIMUM_OUTPUT_REFERENCES
                || inner.bytes.saturating_add(bytes) > MAXIMUM_OUTPUT_STORE_BYTES)
        {
            return Err(OutputError::Full);
        }
        let reference = loop {
            let candidate = generate_reference()?;
            if !inner.records.contains_key(&candidate) {
                break candidate;
            }
        };
        let record = OutputRecord {
            version: RETAINED_OUTPUT_VERSION,
            reference: reference.clone(),
            conversation: key.scope.conversation.map(|id| id.to_string()),
            run: key.scope.run.map(|id| id.to_string()),
            attempt: key.scope.attempt.map(|id| id.to_string()),
            job: key.job.to_string(),
            tool_call: key.tool_call.clone(),
            model_hidden: key.model_hidden,
            chunks,
            truncated,
        };
        persist_record(self.dir.as_deref(), &record)?;
        inner.bytes = inner.bytes.saturating_add(bytes);
        inner.records.insert(reference.clone(), record);
        Ok(RetainedOutput {
            reference,
            truncated,
            bytes,
        })
    }

    /// Copy one retained record into a new ownership scope. A fork uses this
    /// to give the copied result destination provenance without a path from a
    /// model. The source scope is checked, so a fork cannot read another
    /// conversation's output.
    pub(crate) fn rebind(
        &self,
        reference: &str,
        source: &OutputScope,
        destination: &OutputKey,
    ) -> Result<RetainedOutput, OutputError> {
        let mut inner = self.lock();
        let record = inner
            .records
            .get(reference)
            .cloned()
            .ok_or(OutputError::Missing)?;
        if !source.matches(&record) {
            return Err(OutputError::Forbidden);
        }
        let (chunks, bounded) = bound_chunks(&record.chunks, MAXIMUM_RETAINED_BYTES);
        let truncated = record.truncated || bounded;
        let bytes = chunks.iter().map(|chunk| chunk.text.len()).sum::<usize>();
        if !inner.records.is_empty()
            && (inner.records.len() >= MAXIMUM_OUTPUT_REFERENCES
                || inner.bytes.saturating_add(bytes) > MAXIMUM_OUTPUT_STORE_BYTES)
        {
            return Err(OutputError::Full);
        }
        let reference = loop {
            let candidate = generate_reference()?;
            if !inner.records.contains_key(&candidate) {
                break candidate;
            }
        };
        let copied = OutputRecord {
            version: RETAINED_OUTPUT_VERSION,
            reference: reference.clone(),
            conversation: destination.scope.conversation.map(|id| id.to_string()),
            run: destination.scope.run.map(|id| id.to_string()),
            attempt: destination.scope.attempt.map(|id| id.to_string()),
            job: destination.job.to_string(),
            tool_call: destination.tool_call.clone(),
            // A rebind preserves the source access policy.
            model_hidden: record.model_hidden || destination.model_hidden,
            chunks,
            truncated,
        };
        persist_record(self.dir.as_deref(), &copied)?;
        inner.bytes = inner.bytes.saturating_add(bytes);
        inner.records.insert(reference.clone(), copied);
        Ok(RetainedOutput {
            reference,
            truncated,
            bytes,
        })
    }

    pub(crate) fn checkpoint(
        &self,
        reference: &str,
        result: &CommandResult,
    ) -> Result<RetainedOutput, OutputError> {
        let mut inner = self.lock();
        let mut record = inner
            .records
            .get(reference)
            .cloned()
            .ok_or(OutputError::Missing)?;
        let old_bytes = record.bytes();
        let (chunks, bounded) = bound_chunks(&result.chunks, MAXIMUM_RETAINED_BYTES);
        record.chunks = chunks;
        record.truncated =
            bounded || result.termination == crate::execution::CommandTermination::ResourceLimit;
        let bytes = record.bytes();
        let total = inner.bytes.saturating_sub(old_bytes).saturating_add(bytes);
        if total > MAXIMUM_OUTPUT_STORE_BYTES {
            return Err(OutputError::Full);
        }
        persist_record(self.dir.as_deref(), &record)?;
        inner.bytes = total;
        let retained = RetainedOutput {
            reference: reference.to_owned(),
            bytes,
            truncated: record.truncated,
        };
        inner.records.insert(reference.to_owned(), record);
        Ok(retained)
    }

    /// One bounded page of retained output for the model. A record that was
    /// excluded from model context is refused even with the exact reference.
    pub(crate) fn model_page(
        &self,
        reference: &str,
        scope: &OutputScope,
        request: crate::tools::read::PageRequest,
    ) -> Result<OutputPage, OutputError> {
        let inner = self.lock();
        let record = inner.records.get(reference).ok_or(OutputError::Missing)?;
        if !scope.matches(record) {
            return Err(OutputError::Forbidden);
        }
        if record.model_hidden {
            return Err(OutputError::Forbidden);
        }
        match crate::tools::read::page_chunks(&record.chunks, request) {
            Ok((chunks, next, line_truncated)) => Ok(OutputPage {
                chunks,
                next,
                truncated: record.truncated,
                line_truncated,
            }),
            Err(_) => Err(OutputError::Cursor),
        }
    }

    pub(crate) fn has_scope(&self, scope: &OutputScope) -> bool {
        self.lock()
            .records
            .values()
            .any(|record| scope.matches(record))
    }

    /// One bounded page of retained output. Offset values are 1-based lines.
    pub(crate) fn page(
        &self,
        reference: &str,
        scope: &OutputScope,
        request: crate::tools::read::PageRequest,
    ) -> Result<OutputPage, OutputError> {
        let inner = self.lock();
        let record = inner.records.get(reference).ok_or(OutputError::Missing)?;
        if !scope.matches(record) {
            // Do not disclose another conversation's output.
            return Err(OutputError::Forbidden);
        }
        match crate::tools::read::page_chunks(&record.chunks, request) {
            Ok((chunks, next, line_truncated)) => Ok(OutputPage {
                chunks,
                next,
                truncated: record.truncated,
                line_truncated,
            }),
            Err(_) => Err(OutputError::Cursor),
        }
    }

    pub(crate) fn remove_scope(&self, scope: &OutputScope) {
        let mut inner = self.lock();
        let removed: Vec<String> = inner
            .records
            .iter()
            .filter(|(_, record)| scope.matches(record))
            .map(|(reference, _)| reference.clone())
            .collect();
        for reference in removed {
            if remove_record(self.dir.as_deref(), &reference).is_ok()
                && let Some(record) = inner.records.remove(&reference)
            {
                inner.bytes = inner.bytes.saturating_sub(record.bytes());
            }
        }
    }

    pub(crate) fn remove_abandoned(
        &self,
        exists: impl Fn(&OutputScope) -> bool,
    ) -> Result<(), OutputError> {
        let mut inner = self.lock();
        let references: Vec<_> = inner
            .records
            .values()
            .filter(|record| {
                !exists(&OutputScope {
                    conversation: record
                        .conversation
                        .as_deref()
                        .and_then(ConversationId::parse),
                    run: record.run.as_deref().and_then(RunId::parse),
                    attempt: record.attempt.as_deref().and_then(AttemptId::parse),
                })
            })
            .map(|record| record.reference.clone())
            .collect();
        for reference in references {
            remove_record(self.dir.as_deref(), &reference)?;
            if let Some(record) = inner.records.remove(&reference) {
                inner.bytes = inner.bytes.saturating_sub(record.bytes());
            }
        }
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, OutputInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn generate_reference() -> Result<String, OutputError> {
    let mut bytes = [0u8; 16];
    SysRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| OutputError::Persist)?;
    Ok(crate::hex::encode(&bytes))
}

fn bound_chunks(chunks: &[CommandChunk], maximum: usize) -> (Vec<CommandChunk>, bool) {
    let total = chunks.iter().map(|chunk| chunk.text.len()).sum::<usize>();
    if total <= maximum {
        return (chunks.to_vec(), false);
    }
    let mut retained = Vec::new();
    let mut bytes = 0usize;
    for chunk in chunks {
        if bytes >= maximum {
            break;
        }
        let remaining = maximum - bytes;
        let text = if chunk.text.len() <= remaining {
            chunk.text.clone()
        } else {
            let mut end = remaining;
            while end > 0 && !chunk.text.is_char_boundary(end) {
                end -= 1;
            }
            chunk.text[..end].to_owned()
        };
        bytes += text.len();
        retained.push(CommandChunk {
            stream: chunk.stream,
            text,
        });
    }
    (retained, true)
}

fn load_dir(dir: &Path) -> Result<BTreeMap<String, OutputRecord>, OutputError> {
    let mut records = BTreeMap::new();
    let mut total_bytes = 0usize;
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(records),
        Err(_) => return Err(OutputError::Persist),
    };
    for entry in entries {
        let entry = entry.map_err(|_| OutputError::Persist)?;
        let path = entry.path();
        if path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|name| {
                name.strip_prefix('.')
                    .and_then(|name| name.strip_suffix(".tmp"))
                    .is_some_and(|name| {
                        name.len() == 16 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
            })
        {
            crate::storage::remove_private(&path).map_err(|_| OutputError::Persist)?;
            continue;
        }
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(std::ffi::OsStr::to_str) else {
            continue;
        };
        let bytes = crate::storage::read_private_bounded(&path, 8 * MAXIMUM_RETAINED_BYTES)
            .map_err(|_| OutputError::Persist)?;
        let record: OutputRecord =
            serde_json::from_slice(&bytes).map_err(|_| OutputError::Corrupt)?;
        total_bytes = total_bytes.saturating_add(record.bytes());
        if record.version != RETAINED_OUTPUT_VERSION
            || record.reference != stem
            || !crate::tools::valid_output_reference(&record.reference)
            || record.bytes() > MAXIMUM_RETAINED_BYTES
            || record.chunks.len() > crate::execution::command::MAXIMUM_COMMAND_CHUNKS
            || records.len() >= MAXIMUM_OUTPUT_REFERENCES
            || total_bytes > MAXIMUM_OUTPUT_STORE_BYTES
        {
            return Err(OutputError::Corrupt);
        }
        records.insert(record.reference.clone(), record);
    }
    Ok(records)
}

fn persist_record(dir: Option<&Path>, record: &OutputRecord) -> Result<(), OutputError> {
    let Some(dir) = dir else {
        return Ok(());
    };
    let bytes = serde_json::to_vec(record).map_err(|_| OutputError::Persist)?;
    crate::storage::write_private(&dir.join(format!("{}.json", record.reference)), &bytes)
        .map_err(|_| OutputError::Persist)
}

fn remove_record(dir: Option<&Path>, reference: &str) -> Result<(), OutputError> {
    if let Some(dir) = dir {
        crate::storage::remove_private(&dir.join(format!("{reference}.json")))
            .map_err(|_| OutputError::Persist)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
