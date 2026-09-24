#[cfg(test)]
mod tests;

use super::candidate::{
    CandidateEntry, CandidateEntryKind, CandidatePayload, CandidateRevisionArtefact,
};
use super::{ArtefactReference, CandidateHash, ObjectHash, WorkflowArtefactRepository};
use crate::workflows::WorkflowRun;

pub(crate) const MANIFEST_PAGE_SIZE: usize = 8;
pub(crate) const TEXT_PAGE_FRAGMENTS: usize = 256;
const FRAGMENT_BYTES: usize = 128;
const MAXIMUM_TEXT_INPUT_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct CandidateDiff {
    pub(crate) base: CandidateHash,
    pub(crate) target: CandidateHash,
    roots: Vec<DiffRoot>,
    exclusions: Vec<String>,
}

#[derive(Clone, Debug)]
struct DiffRoot {
    directory: String,
    base: CandidateRevisionArtefact,
    target: CandidateRevisionArtefact,
}

#[derive(Clone, Debug)]
pub(crate) struct DiffChange {
    pub(crate) directory: String,
    pub(crate) path: String,
    pub(crate) status: &'static str,
    pub(crate) old: Option<EntryFacts>,
    pub(crate) new: Option<EntryFacts>,
    pub(crate) text: Option<Vec<TextFragment>>,
    pub(crate) binary: bool,
    pub(crate) text_too_large: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct EntryFacts {
    pub(crate) kind: &'static str,
    pub(crate) executable: bool,
    pub(crate) bytes: Option<u64>,
    pub(crate) object: Option<ObjectHash>,
    pub(crate) detail: String,
}

#[derive(Clone, Debug)]
pub(crate) struct TextFragment {
    pub(crate) text: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) continued: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiffError {
    Missing,
    Integrity,
    CrossRun,
    Index,
    Side,
}

impl CandidateDiff {
    pub(crate) fn load(
        run: &WorkflowRun,
        base: &ArtefactReference,
        target: &ArtefactReference,
        store: &WorkflowArtefactRepository,
    ) -> Result<Self, DiffError> {
        if base.kind != crate::workflows::definition::ArtefactKind::CandidateRevision
            || target.kind != crate::workflows::definition::ArtefactKind::CandidateRevision
        {
            return Err(DiffError::CrossRun);
        }
        let base_candidate = load_candidate(run, base, store)?;
        let target_candidate = load_candidate(run, target, store)?;
        Self::from_payloads(base_candidate, target_candidate)
    }

    pub(crate) fn from_payloads(
        base_candidate: CandidatePayload,
        target_candidate: CandidatePayload,
    ) -> Result<Self, DiffError> {
        let (roots, exclusions) = match (&base_candidate, &target_candidate) {
            (CandidatePayload::Revision(base), CandidatePayload::Revision(target)) => (
                vec![DiffRoot {
                    directory: String::new(),
                    base: base.clone(),
                    target: target.clone(),
                }],
                target.exclusions.clone(),
            ),
            (CandidatePayload::Set(base), CandidatePayload::Set(target)) => {
                if base.roots.len() != target.roots.len() {
                    return Err(DiffError::Integrity);
                }
                let mut roots = Vec::new();
                let mut exclusions = Vec::new();
                for (left, right) in base.roots.iter().zip(&target.roots) {
                    if left.grant_id != right.grant_id
                        || left.alias != right.alias
                        || left.identity != right.identity
                        || left.candidate.exclusions != right.candidate.exclusions
                    {
                        return Err(DiffError::Integrity);
                    }
                    exclusions.extend(
                        right
                            .candidate
                            .exclusions
                            .iter()
                            .map(|path| format!("{}/{}", right.alias, path)),
                    );
                    roots.push(DiffRoot {
                        directory: right.alias.clone(),
                        base: left.candidate.clone(),
                        target: right.candidate.clone(),
                    });
                }
                (roots, exclusions)
            }
            _ => return Err(DiffError::Integrity),
        };
        Ok(Self {
            base: base_candidate.candidate_hash(),
            target: target_candidate.candidate_hash(),
            roots,
            exclusions,
        })
    }

    pub(crate) fn exclusions(&self) -> &[String] {
        &self.exclusions
    }

    pub(crate) fn manifest_page(
        &self,
        start: usize,
        limit: usize,
    ) -> Result<(usize, Vec<DiffChange>), DiffError> {
        let end = start.checked_add(limit).ok_or(DiffError::Index)?;
        let mut total = 0;
        let mut page = Vec::with_capacity(limit);
        self.for_each_change(|directory, old, new| {
            if (start..end).contains(&total) {
                page.push(change(directory, old, new, None)?);
            }
            total += 1;
            Ok(())
        })?;
        if start > total {
            return Err(DiffError::Index);
        }
        Ok((total, page))
    }

    pub(crate) fn change(
        &self,
        index: usize,
        store: &WorkflowArtefactRepository,
    ) -> Result<DiffChange, DiffError> {
        let (directory, old, new) = self.change_entries(index)?;
        change(directory, old, new, Some(store))
    }

    pub(crate) fn object(
        &self,
        index: usize,
        side: &str,
        store: &WorkflowArtefactRepository,
    ) -> Result<(String, Vec<u8>), DiffError> {
        let (_, old, new) = self.change_entries(index)?;
        let entry = match side {
            "base" => old,
            "target" => new,
            _ => return Err(DiffError::Side),
        }
        .ok_or(DiffError::Side)?;
        let (hash, expected_bytes, expected_value) = match &entry.kind {
            CandidateEntryKind::Regular { blob, bytes, .. } => (blob, *bytes, None),
            CandidateEntryKind::Symlink { blob, target } => {
                (blob, target.len() as u64, Some(target.as_bytes()))
            }
            CandidateEntryKind::Directory { .. } | CandidateEntryKind::Gitlink { .. } => {
                return Err(DiffError::Side);
            }
        };
        let bytes = store.get(hash).map_err(|_| DiffError::Integrity)?;
        if ObjectHash::of(&bytes) != *hash
            || bytes.len() as u64 != expected_bytes
            || expected_value.is_some_and(|expected| bytes != expected)
        {
            return Err(DiffError::Integrity);
        }
        let filename = entry
            .path
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or("object")
            .to_owned();
        Ok((filename, bytes))
    }

    fn change_entries(
        &self,
        index: usize,
    ) -> Result<(&str, Option<&CandidateEntry>, Option<&CandidateEntry>), DiffError> {
        let mut current = 0;
        let mut found = None;
        self.for_each_change(|directory, old, new| {
            if current == index {
                found = Some((directory, old, new));
            }
            current += 1;
            Ok(())
        })?;
        found.ok_or(DiffError::Index)
    }

    fn for_each_change<'a>(
        &'a self,
        mut visit: impl FnMut(
            &'a str,
            Option<&'a CandidateEntry>,
            Option<&'a CandidateEntry>,
        ) -> Result<(), DiffError>,
    ) -> Result<(), DiffError> {
        for root in &self.roots {
            let mut left = 0;
            let mut right = 0;
            let base = &root.base.entries;
            let target = &root.target.entries;
            while left < base.len() || right < target.len() {
                match (base.get(left), target.get(right)) {
                    (Some(old), Some(new)) if old.path == new.path => {
                        if old.kind != new.kind {
                            visit(&root.directory, Some(old), Some(new))?;
                        }
                        left += 1;
                        right += 1;
                    }
                    (Some(old), Some(new)) if old.path.as_bytes() < new.path.as_bytes() => {
                        visit(&root.directory, Some(old), None)?;
                        left += 1;
                    }
                    (Some(_), Some(new)) => {
                        visit(&root.directory, None, Some(new))?;
                        right += 1;
                    }
                    (Some(old), None) => {
                        visit(&root.directory, Some(old), None)?;
                        left += 1;
                    }
                    (None, Some(new)) => {
                        visit(&root.directory, None, Some(new))?;
                        right += 1;
                    }
                    (None, None) => break,
                }
            }
        }
        Ok(())
    }
}

fn load_candidate(
    run: &WorkflowRun,
    reference: &ArtefactReference,
    store: &WorkflowArtefactRepository,
) -> Result<CandidatePayload, DiffError> {
    let record = run.artefact(&reference.id).ok_or(DiffError::Missing)?;
    if record.artefact_hash != reference.artefact_hash || record.provenance.run_id != run.id {
        return Err(DiffError::CrossRun);
    }
    let bytes = store
        .get(&record.object_hash)
        .map_err(|_| DiffError::Missing)?;
    if ObjectHash::of(&bytes) != record.object_hash {
        return Err(DiffError::Integrity);
    }
    let candidate = CandidatePayload::from_manifest_bytes(&bytes).ok_or(DiffError::Integrity)?;
    let hash = super::artefact_hash_for(record.kind, super::CANDIDATE_SCHEMA, &bytes);
    if hash != record.artefact_hash {
        return Err(DiffError::Integrity);
    }
    Ok(candidate)
}

fn change(
    directory: &str,
    old: Option<&CandidateEntry>,
    new: Option<&CandidateEntry>,
    store: Option<&WorkflowArtefactRepository>,
) -> Result<DiffChange, DiffError> {
    let path = old.or(new).ok_or(DiffError::Integrity)?.path.clone();
    let old_facts = old.map(facts);
    let new_facts = new.map(facts);
    let status = match (old, new) {
        (None, Some(_)) => "Added",
        (Some(_), None) => "Removed",
        (Some(old), Some(new))
            if std::mem::discriminant(&old.kind) != std::mem::discriminant(&new.kind) =>
        {
            "Type changed"
        }
        _ => "Changed",
    };
    let mut binary = false;
    let mut text_too_large = false;
    let mut text = None;
    if let Some(store) = store {
        let declared_bytes = regular_len(old).saturating_add(regular_len(new));
        text_too_large = declared_bytes > MAXIMUM_TEXT_INPUT_BYTES as u64;
        if !text_too_large {
            let old_bytes = regular_bytes(old, store)?;
            let new_bytes = regular_bytes(new, store)?;
            binary = old_bytes
                .as_ref()
                .is_some_and(|bytes| std::str::from_utf8(bytes).is_err())
                || new_bytes
                    .as_ref()
                    .is_some_and(|bytes| std::str::from_utf8(bytes).is_err());
            if !binary && (old_bytes.is_some() || new_bytes.is_some()) {
                let old_text = old_bytes
                    .as_deref()
                    .and_then(|bytes| std::str::from_utf8(bytes).ok())
                    .unwrap_or("");
                let new_text = new_bytes
                    .as_deref()
                    .and_then(|bytes| std::str::from_utf8(bytes).ok())
                    .unwrap_or("");
                let unified = similar::TextDiff::from_lines(old_text, new_text)
                    .unified_diff()
                    .header(&path, &path)
                    .to_string();
                text = Some(fragment(&unified));
            }
        }
    }
    Ok(DiffChange {
        directory: directory.to_owned(),
        path,
        status,
        old: old_facts,
        new: new_facts,
        text,
        binary,
        text_too_large,
    })
}

fn regular_len(entry: Option<&CandidateEntry>) -> u64 {
    match entry.map(|entry| &entry.kind) {
        Some(CandidateEntryKind::Regular { bytes, .. }) => *bytes,
        _ => 0,
    }
}

fn regular_bytes(
    entry: Option<&CandidateEntry>,
    store: &WorkflowArtefactRepository,
) -> Result<Option<Vec<u8>>, DiffError> {
    match entry.map(|entry| &entry.kind) {
        Some(CandidateEntryKind::Regular { blob, bytes, .. }) => {
            let value = store.get(blob).map_err(|_| DiffError::Missing)?;
            if ObjectHash::of(&value) != *blob || value.len() as u64 != *bytes {
                return Err(DiffError::Integrity);
            }
            Ok(Some(value))
        }
        _ => Ok(None),
    }
}

fn facts(entry: &CandidateEntry) -> EntryFacts {
    match &entry.kind {
        CandidateEntryKind::Regular {
            executable,
            mode,
            bytes,
            blob,
        } => EntryFacts {
            kind: "File",
            executable: *executable,
            bytes: Some(*bytes),
            object: Some(*blob),
            detail: format!("mode {:04o}", mode),
        },
        CandidateEntryKind::Symlink { target, blob } => EntryFacts {
            kind: "Symbolic link",
            executable: false,
            bytes: Some(target.len() as u64),
            object: Some(*blob),
            detail: target.clone(),
        },
        CandidateEntryKind::Directory { mode } => EntryFacts {
            kind: "Directory",
            executable: false,
            bytes: None,
            object: None,
            detail: format!("mode {:04o}", mode),
        },
        CandidateEntryKind::Gitlink { commit } => EntryFacts {
            kind: "Gitlink",
            executable: false,
            bytes: None,
            object: None,
            detail: commit.0.clone(),
        },
    }
}

fn fragment(text: &str) -> Vec<TextFragment> {
    let mut result = Vec::new();
    for line in text.split_inclusive('\n') {
        let mut start = 0;
        while start < line.len() {
            let mut end = (start + FRAGMENT_BYTES).min(line.len());
            while end > start && !line.is_char_boundary(end) {
                end -= 1;
            }
            result.push(TextFragment {
                text: line[start..end].to_owned(),
                start,
                end,
                continued: end < line.len(),
            });
            start = end;
        }
    }
    result
}
