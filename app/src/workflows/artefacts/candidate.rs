use std::path::{Path, PathBuf};

use super::id::{CandidateHash, ObjectHash};
use super::store::{ArtefactStoreError, WorkflowArtefactRepository};

pub(crate) const CANDIDATE_SCHEMA: u32 = 1;
pub(crate) const MAXIMUM_ENTRIES: usize = 100_000;
pub(crate) const MAXIMUM_PATH_BYTES: usize = 4_096;
pub(crate) const MAXIMUM_FILE_BYTES: u64 = 512 * 1024 * 1024;
pub(crate) const MAXIMUM_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub(crate) const MAXIMUM_MANIFEST_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAXIMUM_PREVIEW_PATHS: usize = 200;
pub(crate) const MAXIMUM_PREVIEW_BYTES: usize = 1024 * 1024;
const CANDIDATE_DOMAIN: &[u8] = b"frinkworks.candidate.v1";
const GIT_ADMIN_DOMAIN: &[u8] = b"frinkworks.git-admin.v1";
const CANDIDATE_SET_DOMAIN: &[u8] = b"frinkworks.candidate-set.v1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CandidateRevisionArtefact {
    pub(crate) format_version: u32,
    pub(crate) candidate_hash: CandidateHash,
    pub(crate) ordinary: bool,
    pub(crate) repository: Option<RepositoryAnchor>,
    pub(crate) git_admin: Option<GitAdministrativeFingerprint>,
    pub(crate) exclusions: Vec<String>,
    pub(crate) entries: Vec<CandidateEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RepositoryAnchor {
    pub(crate) object_format: GitObjectFormat,
    pub(crate) head: Option<GitObjectId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GitObjectFormat {
    Sha1,
    Sha256,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GitObjectId(pub(crate) String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GitAdministrativeFingerprint(String);

impl GitAdministrativeFingerprint {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        ObjectHash::parse(value).map(|hash| Self(hash.as_str()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CandidateEntry {
    pub(crate) path: String,
    pub(crate) kind: CandidateEntryKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CandidateEntryKind {
    Regular {
        executable: bool,
        mode: u32,
        bytes: u64,
        blob: ObjectHash,
    },
    Symlink {
        target: String,
        blob: ObjectHash,
    },
    Directory {
        mode: u32,
    },
    Gitlink {
        commit: GitObjectId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CaptureError {
    SourceNotGit,
    SourceUnsupported,
    SourceTooLarge,
    SourceChanged,
    SourceRead,
    ArtefactWrite,
    ArtefactIntegrity,
}

impl CaptureError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::SourceNotGit => "The project is not a supported Git worktree.",
            Self::SourceUnsupported => {
                "That directory contains an unsupported entry or source state."
            }
            Self::SourceTooLarge => "The directory is too large to capture.",
            Self::SourceChanged => "The directory changed during source capture.",
            Self::SourceRead => "Frinkworks could not read the directory files.",
            Self::ArtefactWrite => "Frinkworks could not store the candidate. Try again.",
            Self::ArtefactIntegrity => "The stored candidate failed an integrity check.",
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CandidateManifestFile {
    format_version: u32,
    candidate_hash: String,
    ordinary: bool,
    repository: Option<RepositoryAnchorFile>,
    git_admin: Option<String>,
    exclusions: Vec<String>,
    entries: Vec<CandidateEntryFile>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RepositoryAnchorFile {
    object_format: String,
    head: Option<String>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "kebab-case")]
enum CandidateEntryFile {
    Regular {
        path: String,
        executable: bool,
        mode: u32,
        bytes: u64,
        blob: String,
    },
    Symlink {
        path: String,
        target: String,
        blob: String,
    },
    Directory {
        path: String,
        mode: u32,
    },
    Gitlink {
        path: String,
        commit: String,
    },
}

impl CandidateRevisionArtefact {
    pub(crate) fn manifest_bytes(&self) -> Result<Vec<u8>, CaptureError> {
        let file = CandidateManifestFile {
            format_version: self.format_version,
            candidate_hash: self.candidate_hash.as_str(),
            ordinary: self.ordinary,
            repository: self
                .repository
                .as_ref()
                .map(|repository| RepositoryAnchorFile {
                    object_format: match repository.object_format {
                        GitObjectFormat::Sha1 => "sha1".to_owned(),
                        GitObjectFormat::Sha256 => "sha256".to_owned(),
                    },
                    head: repository.head.as_ref().map(|id| id.0.clone()),
                }),
            git_admin: self
                .git_admin
                .as_ref()
                .map(|value| value.as_str().to_owned()),
            exclusions: self.exclusions.clone(),
            entries: self
                .entries
                .iter()
                .map(|entry| match &entry.kind {
                    CandidateEntryKind::Regular {
                        executable,
                        mode,
                        bytes,
                        blob,
                    } => CandidateEntryFile::Regular {
                        path: entry.path.clone(),
                        executable: *executable,
                        mode: *mode,
                        bytes: *bytes,
                        blob: blob.as_str(),
                    },
                    CandidateEntryKind::Symlink { target, blob } => CandidateEntryFile::Symlink {
                        path: entry.path.clone(),
                        target: target.clone(),
                        blob: blob.as_str(),
                    },
                    CandidateEntryKind::Directory { mode } => CandidateEntryFile::Directory {
                        path: entry.path.clone(),
                        mode: *mode,
                    },
                    CandidateEntryKind::Gitlink { commit } => CandidateEntryFile::Gitlink {
                        path: entry.path.clone(),
                        commit: commit.0.clone(),
                    },
                })
                .collect(),
        };
        let bytes = serde_json::to_vec(&file).map_err(|_| CaptureError::ArtefactWrite)?;
        if bytes.len() > MAXIMUM_MANIFEST_BYTES {
            return Err(CaptureError::SourceTooLarge);
        }
        Ok(bytes)
    }

    pub(crate) fn from_manifest_bytes(bytes: &[u8]) -> Option<Self> {
        let file: CandidateManifestFile = serde_json::from_slice(bytes).ok()?;
        if file.format_version != CANDIDATE_SCHEMA {
            return None;
        }
        let mut entries = Vec::new();
        for entry in file.entries {
            entries.push(match entry {
                CandidateEntryFile::Regular {
                    path,
                    executable,
                    mode,
                    bytes,
                    blob,
                } => CandidateEntry {
                    path,
                    kind: CandidateEntryKind::Regular {
                        executable,
                        mode,
                        bytes,
                        blob: ObjectHash::parse(&blob)?,
                    },
                },
                CandidateEntryFile::Symlink { path, target, blob } => CandidateEntry {
                    path,
                    kind: CandidateEntryKind::Symlink {
                        target,
                        blob: ObjectHash::parse(&blob)?,
                    },
                },
                CandidateEntryFile::Directory { path, mode } => CandidateEntry {
                    path,
                    kind: CandidateEntryKind::Directory { mode },
                },
                CandidateEntryFile::Gitlink { path, commit } => CandidateEntry {
                    path,
                    kind: CandidateEntryKind::Gitlink {
                        commit: GitObjectId(commit),
                    },
                },
            });
        }
        let repository = match file.repository {
            Some(repository) => Some(RepositoryAnchor {
                object_format: match repository.object_format.as_str() {
                    "sha1" => GitObjectFormat::Sha1,
                    "sha256" => GitObjectFormat::Sha256,
                    _ => return None,
                },
                head: repository.head.map(GitObjectId),
            }),
            None => None,
        };
        if repository.is_some() != file.git_admin.is_some() {
            return None;
        }
        let git_admin = match file.git_admin {
            Some(value) => Some(GitAdministrativeFingerprint::parse(&value)?),
            None => None,
        };
        let artefact = Self {
            format_version: file.format_version,
            candidate_hash: CandidateHash::parse(&file.candidate_hash)?,
            ordinary: file.ordinary,
            repository,
            git_admin,
            exclusions: file.exclusions,
            entries,
        };
        validate_candidate_shape(&artefact).ok()?;
        if hash_candidate(&artefact.entries, &artefact.exclusions) != artefact.candidate_hash {
            return None;
        }
        Some(artefact)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CandidateSetArtefact {
    pub(crate) format_version: u32,
    pub(crate) candidate_hash: CandidateHash,
    pub(crate) roots: Vec<CandidateSetRoot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CandidateSetRoot {
    pub(crate) grant_id: crate::execution::DirectoryGrantId,
    pub(crate) alias: String,
    pub(crate) identity: crate::execution::CanonicalDirectoryIdentity,
    pub(crate) candidate: CandidateRevisionArtefact,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CandidateSetFile {
    format_version: u32,
    candidate_hash: String,
    roots: Vec<CandidateSetRootFile>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CandidateSetRootFile {
    grant_id: String,
    alias: String,
    identity: crate::execution::CanonicalDirectoryIdentity,
    candidate: serde_json::Value,
}

impl CandidateSetArtefact {
    pub(crate) fn from_roots(roots: Vec<CandidateSetRoot>) -> Result<Self, CaptureError> {
        if roots.is_empty() || roots.len() > 8 {
            return Err(CaptureError::SourceUnsupported);
        }
        for (index, root) in roots.iter().enumerate() {
            validate_candidate_shape(&root.candidate)?;
            if !root.candidate.ordinary
                || !crate::execution::valid_alias(&root.alias)
                || roots[..index].iter().any(|previous| {
                    previous.grant_id == root.grant_id
                        || previous.alias == root.alias
                        || previous.identity == root.identity
                })
            {
                return Err(CaptureError::SourceUnsupported);
            }
        }
        let candidate_hash = hash_candidate_set(&roots);
        Ok(Self {
            format_version: CANDIDATE_SCHEMA,
            candidate_hash,
            roots,
        })
    }

    pub(crate) fn manifest_bytes(&self) -> Result<Vec<u8>, CaptureError> {
        if self.format_version != CANDIDATE_SCHEMA
            || self.candidate_hash != hash_candidate_set(&self.roots)
        {
            return Err(CaptureError::ArtefactIntegrity);
        }
        let roots = self
            .roots
            .iter()
            .map(|root| {
                let bytes = root.candidate.manifest_bytes()?;
                let candidate =
                    serde_json::from_slice(&bytes).map_err(|_| CaptureError::ArtefactWrite)?;
                Ok(CandidateSetRootFile {
                    grant_id: root.grant_id.as_hex(),
                    alias: root.alias.clone(),
                    identity: root.identity,
                    candidate,
                })
            })
            .collect::<Result<Vec<_>, CaptureError>>()?;
        let bytes = serde_json::to_vec(&CandidateSetFile {
            format_version: self.format_version,
            candidate_hash: self.candidate_hash.as_str(),
            roots,
        })
        .map_err(|_| CaptureError::ArtefactWrite)?;
        if bytes.len() > MAXIMUM_MANIFEST_BYTES {
            return Err(CaptureError::SourceTooLarge);
        }
        Ok(bytes)
    }

    pub(crate) fn from_manifest_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > MAXIMUM_MANIFEST_BYTES {
            return None;
        }
        let file: CandidateSetFile = serde_json::from_slice(bytes).ok()?;
        if file.format_version != CANDIDATE_SCHEMA {
            return None;
        }
        let roots = file
            .roots
            .into_iter()
            .map(|root| {
                let bytes = serde_json::to_vec(&root.candidate).ok()?;
                Some(CandidateSetRoot {
                    grant_id: crate::execution::DirectoryGrantId::parse(&root.grant_id)?,
                    alias: root.alias,
                    identity: root.identity,
                    candidate: CandidateRevisionArtefact::from_manifest_bytes(&bytes)?,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let artefact = Self::from_roots(roots).ok()?;
        (artefact.candidate_hash.as_str() == file.candidate_hash).then_some(artefact)
    }

    pub(crate) fn entries(&self) -> impl Iterator<Item = (&str, &CandidateEntry)> {
        self.roots.iter().flat_map(|root| {
            root.candidate
                .entries
                .iter()
                .map(move |entry| (root.alias.as_str(), entry))
        })
    }
}

fn hash_candidate_set(roots: &[CandidateSetRoot]) -> CandidateHash {
    let mut encoded = Vec::from(CANDIDATE_SET_DOMAIN);
    encoded.push(0);
    encoded.extend_from_slice(&CANDIDATE_SCHEMA.to_be_bytes());
    encoded.extend_from_slice(&(roots.len() as u64).to_be_bytes());
    for root in roots {
        push_len_bytes(&mut encoded, root.grant_id.as_hex().as_bytes());
        push_len_bytes(&mut encoded, root.alias.as_bytes());
        encoded.extend_from_slice(&root.identity.device.to_be_bytes());
        encoded.extend_from_slice(&root.identity.inode.to_be_bytes());
        push_len_bytes(
            &mut encoded,
            root.candidate.candidate_hash.as_str().as_bytes(),
        );
    }
    CandidateHash::of(&encoded)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CandidatePayload {
    Revision(CandidateRevisionArtefact),
    Set(CandidateSetArtefact),
}

impl CandidatePayload {
    pub(crate) fn from_manifest_bytes(bytes: &[u8]) -> Option<Self> {
        CandidateSetArtefact::from_manifest_bytes(bytes)
            .map(Self::Set)
            .or_else(|| CandidateRevisionArtefact::from_manifest_bytes(bytes).map(Self::Revision))
    }

    pub(crate) fn manifest_bytes(&self) -> Result<Vec<u8>, CaptureError> {
        match self {
            Self::Revision(value) => value.manifest_bytes(),
            Self::Set(value) => value.manifest_bytes(),
        }
    }

    pub(crate) fn revision(&self) -> Option<&CandidateRevisionArtefact> {
        match self {
            Self::Revision(value) => Some(value),
            Self::Set(_) => None,
        }
    }

    pub(crate) fn candidate_hash(&self) -> CandidateHash {
        match self {
            Self::Revision(value) => value.candidate_hash,
            Self::Set(value) => value.candidate_hash,
        }
    }

    pub(crate) fn entry_count(&self) -> u64 {
        match self {
            Self::Revision(value) => value.entries.len() as u64,
            Self::Set(value) => value.entries().count() as u64,
        }
    }

    pub(crate) fn byte_count(&self) -> u64 {
        let bytes = |entry: &CandidateEntry| match entry.kind {
            CandidateEntryKind::Regular { bytes, .. } => bytes,
            _ => 0,
        };
        match self {
            Self::Revision(value) => value.entries.iter().map(bytes).sum(),
            Self::Set(value) => value.entries().map(|(_, entry)| bytes(entry)).sum(),
        }
    }
}

pub(crate) struct CandidateCapture;

impl CandidateCapture {
    pub(crate) fn capture_set(
        grants: &[crate::execution::DirectoryGrant],
        data_root: &Path,
        store: &WorkflowArtefactRepository,
    ) -> Result<CandidateSetArtefact, CaptureError> {
        let mut roots = Vec::new();
        let reviewed = grants
            .iter()
            .any(|grant| grant.access == crate::execution::DirectoryAccess::ReviewBeforeApply);
        // Read-only inspection can pin a source set. Capture never authorises application.
        for grant in grants.iter().filter(|grant| {
            !reviewed || grant.access == crate::execution::DirectoryAccess::ReviewBeforeApply
        }) {
            grant
                .revalidate()
                .map_err(|_| CaptureError::SourceChanged)?;
            let exclusions = crate::workflows::workspace::reviewed_capture_exclusions(
                &grant.host_path,
                data_root,
            );
            roots.push(CandidateSetRoot {
                grant_id: grant.id,
                alias: grant.alias.clone(),
                identity: grant.identity,
                candidate: Self::capture_directory(&grant.host_path, &exclusions, store)?,
            });
        }
        CandidateSetArtefact::from_roots(roots)
    }

    pub(crate) fn capture_isolated_set(
        workspace: &crate::workflows::workspace::AttemptWorkspace,
        baseline: &CandidateSetArtefact,
        grants: &[crate::execution::DirectoryGrant],
        store: &WorkflowArtefactRepository,
    ) -> Result<CandidateSetArtefact, CaptureError> {
        let mut roots = Vec::new();
        for source in &baseline.roots {
            let grant = grants
                .iter()
                .find(|grant| grant.id == source.grant_id && grant.alias == source.alias)
                .ok_or(CaptureError::SourceChanged)?;
            grant
                .revalidate()
                .map_err(|_| CaptureError::SourceChanged)?;
            if grant.identity != source.identity {
                return Err(CaptureError::SourceChanged);
            }
            let isolated = workspace
                .reviewed_root(&source.alias)
                .map_err(|_| CaptureError::SourceRead)?;
            roots.push(CandidateSetRoot {
                grant_id: source.grant_id,
                alias: source.alias.clone(),
                identity: source.identity,
                candidate: Self::capture_isolated(
                    &isolated,
                    &source.candidate,
                    &grant.host_path.join(".git"),
                    store,
                )?,
            });
        }
        CandidateSetArtefact::from_roots(roots)
    }

    pub(crate) fn capture_host(
        root: &Path,
        store: &WorkflowArtefactRepository,
    ) -> Result<CandidateRevisionArtefact, CaptureError> {
        let git_dir = root.join(".git");
        capture_twice(root, &git_dir, None, store)
    }

    pub(crate) fn capture_directory(
        root: &Path,
        exclusions: &[String],
        store: &WorkflowArtefactRepository,
    ) -> Result<CandidateRevisionArtefact, CaptureError> {
        validate_exclusions_for_manifest(exclusions)?;
        let git = inspect_optional_git(root)?;
        let first = discover_directory(root, exclusions, git.as_ref().map(|(a, f)| (a, f)), store)?;
        let second =
            discover_directory(root, exclusions, git.as_ref().map(|(a, f)| (a, f)), store)?;
        if first != second || inspect_optional_git(root)? != git {
            return Err(CaptureError::SourceChanged);
        }
        Ok(first)
    }

    pub(crate) fn capture_isolated(
        root: &Path,
        baseline: &CandidateRevisionArtefact,
        host_git_dir: &Path,
        store: &WorkflowArtefactRepository,
    ) -> Result<CandidateRevisionArtefact, CaptureError> {
        if !baseline.ordinary {
            let expected = baseline
                .git_admin
                .as_ref()
                .ok_or(CaptureError::SourceUnsupported)?;
            return capture_twice(root, host_git_dir, Some(expected), store);
        }
        if let Some(expected) = baseline.git_admin.as_ref()
            && git_fingerprint(host_git_dir)? != *expected
        {
            return Err(CaptureError::SourceChanged);
        }
        let git = baseline
            .repository
            .as_ref()
            .zip(baseline.git_admin.as_ref());
        let first = discover_directory(root, &baseline.exclusions, git, store)?;
        let second = discover_directory(root, &baseline.exclusions, git, store)?;
        if first != second {
            return Err(CaptureError::SourceChanged);
        }
        if let Some(expected) = baseline.git_admin.as_ref()
            && git_fingerprint(host_git_dir)? != *expected
        {
            return Err(CaptureError::SourceChanged);
        }
        Ok(first)
    }
}

fn capture_twice(
    worktree: &Path,
    git_dir: &Path,
    expected_git: Option<&GitAdministrativeFingerprint>,
    store: &WorkflowArtefactRepository,
) -> Result<CandidateRevisionArtefact, CaptureError> {
    let first = discover(worktree, git_dir, expected_git, store)?;
    let second = discover(worktree, git_dir, first.git_admin.as_ref(), store)?;
    if first != second {
        return Err(CaptureError::SourceChanged);
    }
    Ok(first)
}

pub(super) fn validate_candidate_shape(
    artefact: &CandidateRevisionArtefact,
) -> Result<(), CaptureError> {
    if artefact.entries.len() > MAXIMUM_ENTRIES {
        return Err(CaptureError::SourceTooLarge);
    }
    if !artefact.ordinary
        && (artefact.repository.is_none()
            || artefact.git_admin.is_none()
            || !artefact.exclusions.is_empty())
    {
        return Err(CaptureError::SourceUnsupported);
    }
    validate_exclusions_for_manifest(&artefact.exclusions)?;
    let mut previous: Option<&CandidateEntry> = None;
    let mut total = 0u64;
    for entry in &artefact.entries {
        parse_path(entry.path.as_bytes())?;
        if artefact.exclusions.iter().any(|excluded| {
            entry.path == *excluded || entry.path.starts_with(&format!("{excluded}/"))
        }) {
            return Err(CaptureError::SourceUnsupported);
        }
        for (index, _) in entry.path.match_indices('/') {
            let parent = &entry.path[..index];
            if let Ok(index) = artefact
                .entries
                .binary_search_by(|entry| entry.path.as_str().cmp(parent))
                && !matches!(
                    artefact.entries[index].kind,
                    CandidateEntryKind::Directory { .. }
                )
            {
                return Err(CaptureError::SourceUnsupported);
            }
        }
        if let Some(previous) = previous
            && previous.path.as_bytes() >= entry.path.as_bytes()
        {
            return Err(CaptureError::SourceUnsupported);
        }
        match &entry.kind {
            CandidateEntryKind::Regular {
                executable,
                mode,
                bytes,
                ..
            } => {
                if *mode > 0o777
                    || *executable != (*mode & 0o111 != 0)
                    || *bytes > MAXIMUM_FILE_BYTES
                {
                    return Err(CaptureError::SourceUnsupported);
                }
                total = total
                    .checked_add(*bytes)
                    .ok_or(CaptureError::SourceTooLarge)?;
            }
            CandidateEntryKind::Symlink { target, .. } => {
                if target.len() > MAXIMUM_PATH_BYTES || target.as_bytes().contains(&0) {
                    return Err(CaptureError::SourceTooLarge);
                }
            }
            CandidateEntryKind::Directory { mode } if *mode > 0o777 => {
                return Err(CaptureError::SourceUnsupported);
            }
            CandidateEntryKind::Directory { .. } | CandidateEntryKind::Gitlink { .. } => {}
        }
        previous = Some(entry);
    }
    if total > MAXIMUM_TOTAL_BYTES {
        return Err(CaptureError::SourceTooLarge);
    }
    Ok(())
}

pub(super) fn validate_exclusions_for_manifest(exclusions: &[String]) -> Result<(), CaptureError> {
    let mut previous: Option<&String> = None;
    for exclusion in exclusions {
        parse_path(exclusion.as_bytes())?;
        if previous.is_some_and(|value| value.as_bytes() >= exclusion.as_bytes()) {
            return Err(CaptureError::SourceUnsupported);
        }
        previous = Some(exclusion);
    }
    Ok(())
}

fn inspect_optional_git(
    root: &Path,
) -> Result<Option<(RepositoryAnchor, GitAdministrativeFingerprint)>, CaptureError> {
    let git_dir = root.join(".git");
    match std::fs::symlink_metadata(&git_dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(CaptureError::SourceRead),
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            Err(CaptureError::SourceUnsupported)
        }
        Ok(_) => {
            let (fingerprint, _) = inspect_worktree(root, &git_dir, None)?;
            let object_format =
                match git_text(&git_dir, root, &["rev-parse", "--show-object-format"])
                    .ok()
                    .as_deref()
                {
                    Some("sha256") => GitObjectFormat::Sha256,
                    _ => GitObjectFormat::Sha1,
                };
            let head = git_text(&git_dir, root, &["rev-parse", "HEAD"])
                .ok()
                .filter(|text| !text.is_empty() && text != "HEAD")
                .map(GitObjectId);
            Ok(Some((
                RepositoryAnchor {
                    object_format,
                    head,
                },
                fingerprint,
            )))
        }
    }
}

fn discover_directory(
    root: &Path,
    exclusions: &[String],
    git: Option<(&RepositoryAnchor, &GitAdministrativeFingerprint)>,
    store: &WorkflowArtefactRepository,
) -> Result<CandidateRevisionArtefact, CaptureError> {
    let workspace = super::confine::WorkspaceDir::open(root)?;
    let mut entries = Vec::new();
    let mut total = 0u64;
    let mut traversal_exclusions = exclusions.to_vec();
    if git.is_some() {
        traversal_exclusions.push(".git".to_owned());
        traversal_exclusions.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    }
    let paths = workspace.collect_paths_excluding(&traversal_exclusions)?;
    if paths.len() > MAXIMUM_ENTRIES {
        return Err(CaptureError::SourceTooLarge);
    }
    for path in paths {
        if path == ".git" || path.starts_with(".git/") {
            return Err(CaptureError::SourceUnsupported);
        }
        let kind = match workspace.kind(&path)? {
            super::confine::WorkspaceKind::File { executable } => {
                let (data, opened_executable, bytes) = workspace.read_file(&path)?;
                if executable != opened_executable || bytes > MAXIMUM_FILE_BYTES {
                    return Err(CaptureError::SourceChanged);
                }
                total = total
                    .checked_add(bytes)
                    .ok_or(CaptureError::SourceTooLarge)?;
                if total > MAXIMUM_TOTAL_BYTES {
                    return Err(CaptureError::SourceTooLarge);
                }
                let blob = store.publish(&data).map_err(map_store)?;
                CandidateEntryKind::Regular {
                    executable,
                    mode: workspace.mode(&path)?,
                    bytes,
                    blob,
                }
            }
            super::confine::WorkspaceKind::Symlink => {
                let target = workspace.read_link(&path)?;
                if target.len() > MAXIMUM_PATH_BYTES {
                    return Err(CaptureError::SourceTooLarge);
                }
                let blob = store.publish(target.as_bytes()).map_err(map_store)?;
                CandidateEntryKind::Symlink { target, blob }
            }
            super::confine::WorkspaceKind::Directory => CandidateEntryKind::Directory {
                mode: workspace.mode(&path)?,
            },
            super::confine::WorkspaceKind::Other => return Err(CaptureError::SourceUnsupported),
        };
        entries.push(CandidateEntry { path, kind });
    }
    entries.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
    let candidate_hash = hash_candidate(&entries, exclusions);
    Ok(CandidateRevisionArtefact {
        format_version: CANDIDATE_SCHEMA,
        candidate_hash,
        ordinary: true,
        repository: git.map(|(anchor, _)| anchor.clone()),
        git_admin: git.map(|(_, fingerprint)| fingerprint.clone()),
        exclusions: exclusions.to_vec(),
        entries,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CandidateChange {
    Added,
    Removed,
    Modified,
    ModeChanged,
    LinkChanged,
    GitlinkChanged,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CandidatePreview {
    pub(crate) changes: Vec<PreviewRow>,
    pub(crate) omitted_paths: usize,
    pub(crate) truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreviewRow {
    pub(crate) path: String,
    pub(crate) change: CandidateChange,
    pub(crate) detail: String,
}

#[cfg(test)]
pub(crate) fn hash_entries(entries: &[CandidateEntry]) -> CandidateHash {
    hash_candidate(entries, &[])
}

pub(crate) fn hash_candidate(entries: &[CandidateEntry], exclusions: &[String]) -> CandidateHash {
    let mut encoded = Vec::from(CANDIDATE_DOMAIN);
    encoded.push(0);
    encoded.extend_from_slice(&CANDIDATE_SCHEMA.to_be_bytes());
    encoded.extend_from_slice(&(entries.len() as u64).to_be_bytes());
    for entry in entries {
        let path = entry.path.as_bytes();
        encoded.extend_from_slice(&(path.len() as u32).to_be_bytes());
        encoded.extend_from_slice(path);
        match &entry.kind {
            CandidateEntryKind::Regular {
                executable,
                mode,
                bytes,
                blob,
            } => {
                encoded.push(1);
                encoded.push(if *executable { 1 } else { 0 });
                encoded.extend_from_slice(&mode.to_be_bytes());
                encoded.extend_from_slice(&bytes.to_be_bytes());
                encoded.extend_from_slice(blob.bytes());
            }
            CandidateEntryKind::Symlink { target, blob } => {
                encoded.push(2);
                let bytes = target.as_bytes();
                encoded.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
                encoded.extend_from_slice(bytes);
                encoded.extend_from_slice(blob.bytes());
            }
            CandidateEntryKind::Directory { mode } => {
                encoded.push(3);
                encoded.extend_from_slice(&mode.to_be_bytes());
            }
            CandidateEntryKind::Gitlink { commit } => {
                encoded.push(4);
                let bytes = commit.0.as_bytes();
                encoded.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
                encoded.extend_from_slice(bytes);
            }
        }
    }
    encoded.extend_from_slice(&(exclusions.len() as u64).to_be_bytes());
    for exclusion in exclusions {
        push_len_bytes(&mut encoded, exclusion.as_bytes());
    }
    CandidateHash::of(&encoded)
}

pub(crate) fn compare_candidates(
    before: &[CandidateEntry],
    after: &[CandidateEntry],
) -> Vec<(String, CandidateChange)> {
    let mut changes = Vec::new();
    let mut before_index = 0;
    let mut after_index = 0;
    while before_index < before.len() || after_index < after.len() {
        match (before.get(before_index), after.get(after_index)) {
            (Some(left), Some(right)) if left.path == right.path => {
                if left.kind != right.kind {
                    changes.push((left.path.clone(), change_kind(&left.kind, &right.kind)));
                }
                before_index += 1;
                after_index += 1;
            }
            (Some(left), Some(right)) if left.path.as_bytes() < right.path.as_bytes() => {
                changes.push((left.path.clone(), CandidateChange::Removed));
                before_index += 1;
            }
            (Some(_), Some(right)) => {
                changes.push((right.path.clone(), CandidateChange::Added));
                after_index += 1;
            }
            (Some(left), None) => {
                changes.push((left.path.clone(), CandidateChange::Removed));
                before_index += 1;
            }
            (None, Some(right)) => {
                changes.push((right.path.clone(), CandidateChange::Added));
                after_index += 1;
            }
            (None, None) => break,
        }
    }
    changes
}

pub(crate) fn preview_changes(
    store: &WorkflowArtefactRepository,
    before: &[CandidateEntry],
    after: &[CandidateEntry],
) -> CandidatePreview {
    let changes = compare_candidates(before, after);
    let omitted = changes.len().saturating_sub(MAXIMUM_PREVIEW_PATHS);
    let mut rows = Vec::new();
    let mut rendered = 0usize;
    let mut truncated = omitted > 0;
    for (path, change) in changes.into_iter().take(MAXIMUM_PREVIEW_PATHS) {
        let detail = preview_detail(store, before, after, &path, change, &mut rendered);
        if rendered > MAXIMUM_PREVIEW_BYTES {
            truncated = true;
        }
        rows.push(PreviewRow {
            path,
            change,
            detail,
        });
        if truncated && rendered > MAXIMUM_PREVIEW_BYTES {
            break;
        }
    }
    CandidatePreview {
        changes: rows,
        omitted_paths: omitted,
        truncated,
    }
}

pub(crate) fn preview_plain(
    store: &WorkflowArtefactRepository,
    before: &[CandidateEntry],
    after: &[CandidateEntry],
) -> (String, bool) {
    let preview = preview_changes(store, before, after);
    let mut text = String::new();
    for row in &preview.changes {
        text.push_str(&row.path);
        text.push(' ');
        text.push_str(&row.detail);
        text.push('\n');
    }
    if preview.omitted_paths > 0 {
        text.push_str(&format!(
            "Omitted {} changed paths.\n",
            preview.omitted_paths
        ));
    }
    (text, preview.truncated)
}

fn preview_detail(
    store: &WorkflowArtefactRepository,
    before: &[CandidateEntry],
    after: &[CandidateEntry],
    path: &str,
    change: CandidateChange,
    rendered: &mut usize,
) -> String {
    let left = before.iter().find(|entry| entry.path == path);
    let right = after.iter().find(|entry| entry.path == path);
    match (
        left.map(|entry| &entry.kind),
        right.map(|entry| &entry.kind),
    ) {
        (
            Some(CandidateEntryKind::Regular { blob: old, .. }),
            Some(CandidateEntryKind::Regular {
                blob: new, bytes, ..
            }),
        ) if change == CandidateChange::Modified => {
            let Ok(old_bytes) = store.get(old) else {
                return "Binary or unread file".to_owned();
            };
            let Ok(new_bytes) = store.get(new) else {
                return "Binary or unread file".to_owned();
            };
            if !is_text(&old_bytes) || !is_text(&new_bytes) {
                return format!("Binary file ({bytes} bytes)");
            }
            let old_text = String::from_utf8_lossy(&old_bytes);
            let new_text = String::from_utf8_lossy(&new_bytes);
            let diff = similar::TextDiff::from_lines(old_text.as_ref(), new_text.as_ref());
            let mut unified = diff.unified_diff().header(path, path).to_string();
            if *rendered + unified.len() > MAXIMUM_PREVIEW_BYTES {
                unified.truncate(MAXIMUM_PREVIEW_BYTES.saturating_sub(*rendered));
                *rendered = MAXIMUM_PREVIEW_BYTES;
                return unified;
            }
            *rendered += unified.len();
            unified
        }
        (None, Some(CandidateEntryKind::Regular { bytes, .. })) => {
            format!("Added file ({bytes} bytes)")
        }
        (Some(CandidateEntryKind::Regular { bytes, .. }), None) => {
            format!("Removed file ({bytes} bytes)")
        }
        (_, Some(CandidateEntryKind::Symlink { target, .. })) => {
            format!("Symbolic link → {target}")
        }
        (Some(CandidateEntryKind::Symlink { target, .. }), None) => {
            format!("Removed symbolic link → {target}")
        }
        (_, Some(CandidateEntryKind::Gitlink { commit })) => {
            format!("Gitlink {}", commit.0)
        }
        _ => change_label(change).to_owned(),
    }
}

fn change_kind(before: &CandidateEntryKind, after: &CandidateEntryKind) -> CandidateChange {
    match (before, after) {
        (
            CandidateEntryKind::Regular {
                executable: left, ..
            },
            CandidateEntryKind::Regular {
                executable: right,
                blob,
                ..
            },
        ) => {
            let CandidateEntryKind::Regular { blob: old_blob, .. } = before else {
                return CandidateChange::Modified;
            };
            if old_blob != blob {
                CandidateChange::Modified
            } else if left != right {
                CandidateChange::ModeChanged
            } else {
                CandidateChange::Modified
            }
        }
        (CandidateEntryKind::Symlink { .. }, CandidateEntryKind::Symlink { .. }) => {
            CandidateChange::LinkChanged
        }
        (CandidateEntryKind::Gitlink { .. }, CandidateEntryKind::Gitlink { .. }) => {
            CandidateChange::GitlinkChanged
        }
        _ => CandidateChange::Modified,
    }
}

fn change_label(change: CandidateChange) -> &'static str {
    match change {
        CandidateChange::Added => "Added",
        CandidateChange::Removed => "Removed",
        CandidateChange::Modified => "Modified",
        CandidateChange::ModeChanged => "Mode changed",
        CandidateChange::LinkChanged => "Link changed",
        CandidateChange::GitlinkChanged => "Gitlink changed",
    }
}

fn is_text(bytes: &[u8]) -> bool {
    !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok()
}

pub(crate) fn inspect_supported_worktree(worktree: &Path) -> Result<(), CaptureError> {
    inspect_worktree(worktree, &worktree.join(".git"), None).map(|_| ())
}

fn inspect_worktree(
    worktree: &Path,
    git_dir: &Path,
    expected_git: Option<&GitAdministrativeFingerprint>,
) -> Result<(GitAdministrativeFingerprint, super::confine::WorkspaceDir), CaptureError> {
    let git_meta = std::fs::symlink_metadata(git_dir).map_err(|_| CaptureError::SourceNotGit)?;
    if git_meta.file_type().is_symlink() || !git_meta.is_dir() {
        return Err(CaptureError::SourceUnsupported);
    }
    let fingerprint = git_fingerprint(git_dir)?;
    if let Some(expected) = expected_git
        && expected != &fingerprint
    {
        return Err(CaptureError::SourceChanged);
    }
    let workspace = super::confine::WorkspaceDir::open(worktree)?;
    let unmerged = git_output(git_dir, worktree, &["ls-files", "-u", "-z"])?;
    if !unmerged.is_empty() {
        return Err(CaptureError::SourceUnsupported);
    }
    Ok((fingerprint, workspace))
}

fn discover(
    worktree: &Path,
    git_dir: &Path,
    expected_git: Option<&GitAdministrativeFingerprint>,
    store: &WorkflowArtefactRepository,
) -> Result<CandidateRevisionArtefact, CaptureError> {
    let (fingerprint, workspace) = inspect_worktree(worktree, git_dir, expected_git)?;
    let object_format = match git_text(git_dir, worktree, &["rev-parse", "--show-object-format"])
        .ok()
        .as_deref()
    {
        Some("sha256") => GitObjectFormat::Sha256,
        _ => GitObjectFormat::Sha1,
    };
    let head = git_text(git_dir, worktree, &["rev-parse", "HEAD"])
        .ok()
        .and_then(|text| {
            let id = text.trim();
            if id.is_empty() || id == "HEAD" {
                None
            } else {
                Some(GitObjectId(id.to_owned()))
            }
        });
    let staged = git_output(git_dir, worktree, &["ls-files", "-z", "--stage"])?;
    let others = git_output(
        git_dir,
        worktree,
        &["ls-files", "-z", "--others", "--exclude-standard"],
    )?;
    let mut entries = Vec::new();
    let mut total = 0u64;
    parse_staged(&workspace, store, &staged, &mut entries, &mut total)?;
    parse_untracked(&workspace, store, &others, &mut entries, &mut total)?;
    entries.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
    if entries.len() > MAXIMUM_ENTRIES {
        return Err(CaptureError::SourceTooLarge);
    }
    let after = git_fingerprint(git_dir)?;
    if after != fingerprint {
        return Err(CaptureError::SourceChanged);
    }
    Ok(CandidateRevisionArtefact {
        format_version: CANDIDATE_SCHEMA,
        candidate_hash: hash_candidate(&entries, &[]),
        ordinary: false,
        repository: Some(RepositoryAnchor {
            object_format,
            head,
        }),
        git_admin: Some(fingerprint),
        exclusions: Vec::new(),
        entries,
    })
}

fn parse_staged(
    workspace: &super::confine::WorkspaceDir,
    store: &WorkflowArtefactRepository,
    bytes: &[u8],
    entries: &mut Vec<CandidateEntry>,
    total: &mut u64,
) -> Result<(), CaptureError> {
    for record in split_nul(bytes) {
        if record.is_empty() {
            continue;
        }
        let Some((meta, path)) = split_once_space_tab(record) else {
            return Err(CaptureError::SourceUnsupported);
        };
        let path = parse_path(path)?;
        if path_under_symlink(workspace, &path) {
            continue;
        }
        let meta = std::str::from_utf8(meta).map_err(|_| CaptureError::SourceUnsupported)?;
        let mut parts = meta.split(' ');
        let mode = parts.next().ok_or(CaptureError::SourceUnsupported)?;
        let object = parts.next().ok_or(CaptureError::SourceUnsupported)?;
        let stage = parts.next().ok_or(CaptureError::SourceUnsupported)?;
        if stage != "0" {
            return Err(CaptureError::SourceUnsupported);
        }
        if let Some(entry) = capture_tracked(workspace, store, &path, mode, object, total)? {
            entries.push(entry);
        }
    }
    Ok(())
}

fn capture_tracked(
    workspace: &super::confine::WorkspaceDir,
    store: &WorkflowArtefactRepository,
    path: &str,
    index_mode: &str,
    index_object: &str,
    total: &mut u64,
) -> Result<Option<CandidateEntry>, CaptureError> {
    use super::confine::WorkspaceKind;
    if !workspace.exists(path) {
        return Ok(None);
    }
    match workspace.kind(path)? {
        WorkspaceKind::File { executable } => {
            if index_mode == "160000" {
                return Err(CaptureError::SourceUnsupported);
            }
            let (bytes, opened_executable, size) = workspace.read_file(path)?;
            if opened_executable != executable {
                return Err(CaptureError::SourceChanged);
            }
            let blob = store.publish(&bytes).map_err(map_store)?;
            *total = total.saturating_add(size);
            if *total > MAXIMUM_TOTAL_BYTES || size > MAXIMUM_FILE_BYTES {
                return Err(CaptureError::SourceTooLarge);
            }
            Ok(Some(CandidateEntry {
                path: path.to_owned(),
                kind: CandidateEntryKind::Regular {
                    executable,
                    mode: workspace.mode(path)?,
                    bytes: size,
                    blob,
                },
            }))
        }
        WorkspaceKind::Symlink => {
            if index_mode == "160000" {
                return Err(CaptureError::SourceUnsupported);
            }
            let target = workspace.read_link(path)?;
            let blob = store.publish(target.as_bytes()).map_err(map_store)?;
            Ok(Some(CandidateEntry {
                path: path.to_owned(),
                kind: CandidateEntryKind::Symlink { target, blob },
            }))
        }
        WorkspaceKind::Directory => {
            if index_mode != "160000" {
                return Ok(None);
            }
            if !workspace.dir_is_empty(path)? {
                return Err(CaptureError::SourceUnsupported);
            }
            Ok(Some(CandidateEntry {
                path: path.to_owned(),
                kind: CandidateEntryKind::Gitlink {
                    commit: GitObjectId(index_object.to_owned()),
                },
            }))
        }
        WorkspaceKind::Other => Err(CaptureError::SourceUnsupported),
    }
}

fn parse_untracked(
    workspace: &super::confine::WorkspaceDir,
    store: &WorkflowArtefactRepository,
    bytes: &[u8],
    entries: &mut Vec<CandidateEntry>,
    total: &mut u64,
) -> Result<(), CaptureError> {
    for record in split_nul(bytes) {
        if record.is_empty() {
            continue;
        }
        let path = parse_path(record)?;
        if entries.iter().any(|entry| entry.path == path) {
            continue;
        }
        if path_under_symlink(workspace, &path) {
            continue;
        }
        if !workspace.exists(&path) {
            continue;
        }
        match workspace.kind(&path)? {
            super::confine::WorkspaceKind::Symlink => {
                let target = workspace.read_link(&path)?;
                let blob = store.publish(target.as_bytes()).map_err(map_store)?;
                entries.push(CandidateEntry {
                    path,
                    kind: CandidateEntryKind::Symlink { target, blob },
                });
            }
            super::confine::WorkspaceKind::File { executable } => {
                let (bytes, opened_executable, size) = match workspace.read_file(&path) {
                    Ok(value) => value,
                    Err(CaptureError::SourceRead)
                        if workspace.kind(&path).ok()
                            == Some(super::confine::WorkspaceKind::Symlink) =>
                    {
                        let target = workspace.read_link(&path)?;
                        let blob = store.publish(target.as_bytes()).map_err(map_store)?;
                        entries.push(CandidateEntry {
                            path,
                            kind: CandidateEntryKind::Symlink { target, blob },
                        });
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                if opened_executable != executable {
                    return Err(CaptureError::SourceChanged);
                }
                let blob = store.publish(&bytes).map_err(map_store)?;
                *total = total.saturating_add(size);
                if *total > MAXIMUM_TOTAL_BYTES || size > MAXIMUM_FILE_BYTES {
                    return Err(CaptureError::SourceTooLarge);
                }
                let mode = workspace.mode(&path)?;
                entries.push(CandidateEntry {
                    path,
                    kind: CandidateEntryKind::Regular {
                        executable,
                        mode,
                        bytes: size,
                        blob,
                    },
                });
            }
            super::confine::WorkspaceKind::Directory => continue,
            super::confine::WorkspaceKind::Other => return Err(CaptureError::SourceUnsupported),
        }
    }
    Ok(())
}

fn path_under_symlink(workspace: &super::confine::WorkspaceDir, path: &str) -> bool {
    let mut current = PathBuf::new();
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 2 {
        return false;
    }
    for part in &parts[..parts.len() - 1] {
        current.push(part);
        let relative = current.to_string_lossy();
        if workspace.exists(relative.as_ref())
            && workspace.kind(relative.as_ref()).ok()
                == Some(super::confine::WorkspaceKind::Symlink)
        {
            return true;
        }
    }
    false
}

pub(crate) fn git_fingerprint(
    git_dir: &Path,
) -> Result<GitAdministrativeFingerprint, CaptureError> {
    let index = read_optional_git_file(git_dir, "index")?.unwrap_or_default();
    let head = read_git_file(git_dir, "HEAD")?;
    let config = read_git_file(git_dir, "config")?;
    reject_config_includes(&config)?;
    if git_dir.join("config.worktree").exists() {
        return Err(CaptureError::SourceUnsupported);
    }
    let exclude = read_optional_git_file(git_dir, "info/exclude")?;
    let resolved = std::process::Command::new("git")
        .args(["--git-dir"])
        .arg(git_dir)
        .args([
            "--no-optional-locks",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "alias.rev-parse=",
            "rev-parse",
            "HEAD",
        ])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| output.stdout)
        .unwrap_or_default();
    let format = std::process::Command::new("git")
        .args(["--git-dir"])
        .arg(git_dir)
        .args([
            "--no-optional-locks",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "alias.rev-parse=",
            "rev-parse",
            "--show-object-format",
        ])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| output.stdout)
        .unwrap_or_default();
    let mut encoded = Vec::from(GIT_ADMIN_DOMAIN);
    encoded.push(0);
    push_len_bytes(&mut encoded, &index);
    push_len_bytes(&mut encoded, &head);
    push_len_bytes(&mut encoded, &resolved);
    push_len_bytes(&mut encoded, &format);
    push_len_bytes(&mut encoded, &config);
    match exclude {
        Some(bytes) => {
            encoded.push(1);
            push_len_bytes(&mut encoded, &bytes);
        }
        None => encoded.push(0),
    }
    Ok(GitAdministrativeFingerprint(
        ObjectHash::of(&encoded).as_str(),
    ))
}

fn push_len_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn read_git_file(git_dir: &Path, relative: &str) -> Result<Vec<u8>, CaptureError> {
    read_optional_git_file(git_dir, relative)?.ok_or(CaptureError::SourceRead)
}

fn read_optional_git_file(git_dir: &Path, relative: &str) -> Result<Option<Vec<u8>>, CaptureError> {
    let path = git_dir.join(relative);
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(CaptureError::SourceRead),
    };
    if meta.file_type().is_symlink() {
        return Err(CaptureError::SourceUnsupported);
    }
    Ok(Some(
        std::fs::read(&path).map_err(|_| CaptureError::SourceRead)?,
    ))
}

fn reject_config_includes(bytes: &[u8]) -> Result<(), CaptureError> {
    let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    if text.contains("[include]") || text.contains("[includeif") || text.contains("worktreeconfig")
    {
        return Err(CaptureError::SourceUnsupported);
    }
    Ok(())
}

fn git_text(git_dir: &Path, worktree: &Path, args: &[&str]) -> Result<String, CaptureError> {
    let bytes = git_output(git_dir, worktree, args)?;
    String::from_utf8(bytes)
        .map(|text| text.trim().to_owned())
        .map_err(|_| CaptureError::SourceUnsupported)
}

fn git_output(git_dir: &Path, worktree: &Path, args: &[&str]) -> Result<Vec<u8>, CaptureError> {
    let output = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .arg("--work-tree")
        .arg(worktree)
        .args([
            "--no-optional-locks",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=",
            "-c",
            "core.excludesFile=/dev/null",
            "-c",
            "alias.rev-parse=",
            "-c",
            "alias.ls-files=",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|_| CaptureError::SourceUnsupported)?;
    if !output.status.success() {
        return Err(CaptureError::SourceNotGit);
    }
    Ok(output.stdout)
}

fn parse_path(bytes: &[u8]) -> Result<String, CaptureError> {
    if bytes.len() > MAXIMUM_PATH_BYTES || bytes.contains(&0) {
        return Err(CaptureError::SourceTooLarge);
    }
    let path = std::str::from_utf8(bytes).map_err(|_| CaptureError::SourceUnsupported)?;
    if path.is_empty() || path.starts_with('/') || path.contains('\0') {
        return Err(CaptureError::SourceUnsupported);
    }
    if path == ".git"
        || path.starts_with(".git/")
        || path.split('/').any(|part| matches!(part, "" | "." | ".."))
    {
        return Err(CaptureError::SourceUnsupported);
    }
    Ok(path.to_owned())
}

fn split_nul(bytes: &[u8]) -> Vec<&[u8]> {
    bytes.split(|byte| *byte == 0).collect()
}

fn split_once_space_tab(record: &[u8]) -> Option<(&[u8], &[u8])> {
    let index = record.iter().position(|byte| *byte == b'\t')?;
    Some((&record[..index], &record[index + 1..]))
}

fn map_store(error: ArtefactStoreError) -> CaptureError {
    match error {
        ArtefactStoreError::Integrity => CaptureError::ArtefactIntegrity,
        ArtefactStoreError::Persist | ArtefactStoreError::Missing => CaptureError::ArtefactWrite,
    }
}

#[cfg(test)]
mod tests;
