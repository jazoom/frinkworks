use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::{Mutex, atomic::AtomicBool, atomic::Ordering};

use microsandbox::snapshot::{
    Snapshot, SnapshotFormat, SnapshotScope, UpperIntegrity, UpperVerifyStatus,
};

use super::id::{EnvironmentId, PreparationId};
use super::recipe::EnvironmentRecipeVersion;
use crate::storage;

const DIGEST_PREFIX: &str = "sha256:";
const DIGEST_HEX_LEN: usize = 64;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SnapshotArtifactKey(String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SnapshotDigest(String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OciManifestDigest(String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecordedIntegrity {
    pub(crate) algorithm: String,
    pub(crate) value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparedSnapshot {
    pub(crate) artifact_key: SnapshotArtifactKey,
    pub(crate) snapshot_digest: SnapshotDigest,
    pub(crate) image_reference: String,
    pub(crate) image_manifest_digest: OciManifestDigest,
    pub(crate) upper_integrity: RecordedIntegrity,
    pub(crate) upper_size_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SnapshotAvailability {
    Available,
    Missing,
    Corrupt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SnapshotError {
    Missing,
    Corrupt,
    Remove,
    Path,
}

pub(crate) struct EnvironmentSnapshotRepository {
    root: Option<PathBuf>,
    #[cfg(test)]
    overrides: Mutex<Vec<(SnapshotArtifactKey, SnapshotAvailability)>>,
    #[cfg(test)]
    remove_error: AtomicBool,
    #[cfg(not(test))]
    _private: (),
}

impl SnapshotArtifactKey {
    pub(crate) fn from_preparation(id: &PreparationId) -> Self {
        Self(id.as_hex())
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        PreparationId::parse(value).map(|id| Self(id.as_hex()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl SnapshotDigest {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        parse_sha256_digest(value).map(Self)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn short_hex(&self) -> String {
        let hex = self.0.strip_prefix(DIGEST_PREFIX).unwrap_or(&self.0);
        hex[..8.min(hex.len())].to_owned()
    }
}

impl OciManifestDigest {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        parse_sha256_digest(value).map(Self)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl RecordedIntegrity {
    pub(crate) fn from_upper(integrity: &UpperIntegrity) -> Option<Self> {
        match integrity {
            UpperIntegrity::Sha256 { digest } => Some(Self {
                algorithm: "sha256".to_owned(),
                value: digest.clone(),
            }),
            UpperIntegrity::SparseSha256V1 { digest } => Some(Self {
                algorithm: "msb-sparse-sha256-v1".to_owned(),
                value: digest.clone(),
            }),
            UpperIntegrity::FileMerkleBlake3V1 { root, .. } => Some(Self {
                algorithm: "msb-file-merkle-blake3-v1".to_owned(),
                value: root.clone(),
            }),
        }
    }
}

impl EnvironmentSnapshotRepository {
    pub(crate) fn open(root: PathBuf) -> Result<Self, SnapshotError> {
        storage::ensure_private_dir(&root).map_err(|_| SnapshotError::Path)?;
        Ok(Self {
            root: Some(root),
            #[cfg(test)]
            overrides: Mutex::new(Vec::new()),
            #[cfg(test)]
            remove_error: AtomicBool::new(false),
            #[cfg(not(test))]
            _private: (),
        })
    }

    pub(crate) fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub(crate) fn artifact_dir(&self, key: &SnapshotArtifactKey) -> Result<PathBuf, SnapshotError> {
        let root = self.root.as_deref().ok_or(SnapshotError::Path)?;
        storage::confined_child(root, key.as_str()).map_err(|_| SnapshotError::Path)
    }

    pub(crate) fn recorded_availability(
        &self,
        snapshot: &PreparedSnapshot,
    ) -> SnapshotAvailability {
        #[cfg(test)]
        {
            let overrides = self
                .overrides
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some((_, availability)) = overrides
                .iter()
                .rev()
                .find(|(key, _)| key == &snapshot.artifact_key)
            {
                return *availability;
            }
        }
        self.artifact_dir(&snapshot.artifact_key)
            .map_or(SnapshotAvailability::Missing, |path| {
                if path.exists() {
                    SnapshotAvailability::Available
                } else {
                    SnapshotAvailability::Missing
                }
            })
    }

    pub(crate) async fn inspect(&self, snapshot: &PreparedSnapshot) -> SnapshotAvailability {
        #[cfg(test)]
        {
            let overrides = self
                .overrides
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some((_, availability)) = overrides
                .iter()
                .rev()
                .find(|(key, _)| key == &snapshot.artifact_key)
            {
                return *availability;
            }
            if self.root.is_none() {
                return SnapshotAvailability::Missing;
            }
        }
        match self.open_recorded(snapshot).await {
            Ok(()) => SnapshotAvailability::Available,
            Err(SnapshotError::Missing) => SnapshotAvailability::Missing,
            Err(_) => SnapshotAvailability::Corrupt,
        }
    }

    pub(crate) fn restore_path(&self, key: &SnapshotArtifactKey) -> Result<PathBuf, SnapshotError> {
        if self.root.is_none() {
            return Ok(PathBuf::from(key.as_str()));
        }
        self.artifact_dir(key)
    }

    pub(crate) async fn matches_pin(
        &self,
        snapshot: &PreparedSnapshot,
    ) -> Result<(), SnapshotError> {
        #[cfg(test)]
        {
            match self.inspect(snapshot).await {
                SnapshotAvailability::Available => Ok(()),
                SnapshotAvailability::Missing => Err(SnapshotError::Missing),
                SnapshotAvailability::Corrupt => Err(SnapshotError::Corrupt),
            }
        }
        #[cfg(not(test))]
        self.open_recorded(snapshot).await
    }

    pub(crate) async fn verify(&self, snapshot: &PreparedSnapshot) -> Result<(), SnapshotError> {
        #[cfg(test)]
        {
            let overrides = self
                .overrides
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some((_, availability)) = overrides
                .iter()
                .rev()
                .find(|(key, _)| key == &snapshot.artifact_key)
            {
                return match availability {
                    SnapshotAvailability::Available => Ok(()),
                    SnapshotAvailability::Missing => Err(SnapshotError::Missing),
                    SnapshotAvailability::Corrupt => Err(SnapshotError::Corrupt),
                };
            }
            if self.root.is_none() {
                return Err(SnapshotError::Missing);
            }
        }
        let path = self.artifact_dir(&snapshot.artifact_key)?;
        let opened = Snapshot::open(path.to_string_lossy().as_ref())
            .await
            .map_err(|_| SnapshotError::Missing)?;
        let report = opened.verify().await.map_err(|_| SnapshotError::Corrupt)?;
        if report.digest != opened.digest() || report.digest != snapshot.snapshot_digest.as_str() {
            return Err(SnapshotError::Corrupt);
        }
        match report.upper {
            UpperVerifyStatus::Verified { .. } => Ok(()),
            UpperVerifyStatus::NotRecorded => Err(SnapshotError::Corrupt),
        }
    }

    pub(crate) async fn remove_unpublished(
        &self,
        key: &SnapshotArtifactKey,
    ) -> Result<(), SnapshotError> {
        #[cfg(test)]
        {
            if self.remove_error.load(Ordering::SeqCst) {
                return Err(SnapshotError::Remove);
            }
            if self.root.is_none() {
                return Ok(());
            }
        }
        let path = self.artifact_dir(key)?;
        if !path.exists() {
            return Ok(());
        }
        match Snapshot::remove(path.to_string_lossy().as_ref(), true).await {
            Ok(()) => Ok(()),
            Err(_) => {
                if path.exists() {
                    Err(SnapshotError::Remove)
                } else {
                    Ok(())
                }
            }
        }
    }

    async fn open_recorded(&self, snapshot: &PreparedSnapshot) -> Result<(), SnapshotError> {
        let path = self.artifact_dir(&snapshot.artifact_key)?;
        if !path.exists() {
            return Err(SnapshotError::Missing);
        }
        let opened = Snapshot::open(path.to_string_lossy().as_ref())
            .await
            .map_err(|_| SnapshotError::Corrupt)?;
        if opened.digest() != snapshot.snapshot_digest.as_str() {
            return Err(SnapshotError::Corrupt);
        }
        if opened.manifest().scope != SnapshotScope::Disk {
            return Err(SnapshotError::Corrupt);
        }
        let Some(file) = opened.manifest().state.as_file() else {
            return Err(SnapshotError::Corrupt);
        };
        if file.format != SnapshotFormat::Raw || file.fstype != "ext4" {
            return Err(SnapshotError::Corrupt);
        }
        if file.upper.size_bytes != snapshot.upper_size_bytes {
            return Err(SnapshotError::Corrupt);
        }
        let Some(integrity) = file.upper.integrity.as_ref() else {
            return Err(SnapshotError::Corrupt);
        };
        let Some(recorded) = RecordedIntegrity::from_upper(integrity) else {
            return Err(SnapshotError::Corrupt);
        };
        if recorded != snapshot.upper_integrity {
            return Err(SnapshotError::Corrupt);
        }
        if opened.manifest().image.reference != snapshot.image_reference
            || opened.manifest().image.manifest_digest != snapshot.image_manifest_digest.as_str()
        {
            return Err(SnapshotError::Corrupt);
        }
        Ok(())
    }
}

pub(crate) async fn create_prepared_snapshot(
    repository: &EnvironmentSnapshotRepository,
    environment_id: EnvironmentId,
    preparation_id: PreparationId,
    recipe_version: EnvironmentRecipeVersion,
    sandbox_name: &str,
) -> Result<PreparedSnapshot, super::preparation::FailureCategory> {
    use super::preparation::FailureCategory;
    let artifact_key = SnapshotArtifactKey::from_preparation(&preparation_id);
    let dest_dir = repository
        .root()
        .ok_or(FailureCategory::SnapshotCreate)?
        .to_path_buf();
    storage::ensure_private_dir(&dest_dir).map_err(|_| FailureCategory::SnapshotCreate)?;
    let snapshot = Snapshot::builder(artifact_key.as_str())
        .from_sandbox(sandbox_name)
        .dest_dir(dest_dir)
        .label("works.frinkworks.environment", environment_id.as_hex())
        .label("works.frinkworks.preparation", preparation_id.as_hex())
        .label("works.frinkworks.recipe", recipe_version.as_digest())
        .record_integrity()
        .create()
        .await
        .map_err(|_| FailureCategory::SnapshotCreate)?;
    let metadata = (|| {
        if snapshot.manifest().scope != SnapshotScope::Disk {
            return Err(FailureCategory::SnapshotIntegrity);
        }
        let file = snapshot
            .manifest()
            .state
            .as_file()
            .ok_or(FailureCategory::SnapshotIntegrity)?;
        if file.format != SnapshotFormat::Raw || file.fstype != "ext4" {
            return Err(FailureCategory::SnapshotIntegrity);
        }
        let integrity = file
            .upper
            .integrity
            .as_ref()
            .ok_or(FailureCategory::SnapshotIntegrity)?;
        let upper_integrity =
            RecordedIntegrity::from_upper(integrity).ok_or(FailureCategory::SnapshotIntegrity)?;
        let snapshot_digest =
            SnapshotDigest::parse(snapshot.digest()).ok_or(FailureCategory::SnapshotIntegrity)?;
        let image_manifest_digest =
            OciManifestDigest::parse(&snapshot.manifest().image.manifest_digest)
                .ok_or(FailureCategory::SnapshotIntegrity)?;
        Ok((
            snapshot_digest,
            snapshot.manifest().image.reference.clone(),
            image_manifest_digest,
            upper_integrity,
            file.upper.size_bytes,
        ))
    })();
    let (
        snapshot_digest,
        image_reference,
        image_manifest_digest,
        upper_integrity,
        upper_size_bytes,
    ) = match metadata {
        Ok(metadata) => metadata,
        Err(category) => {
            if repository.remove_unpublished(&artifact_key).await.is_err() {
                return Err(FailureCategory::SnapshotRemove);
            }
            return Err(category);
        }
    };
    Ok(PreparedSnapshot {
        artifact_key,
        snapshot_digest,
        image_reference,
        image_manifest_digest,
        upper_integrity,
        upper_size_bytes,
    })
}

fn parse_sha256_digest(value: &str) -> Option<String> {
    let rest = value.strip_prefix(DIGEST_PREFIX)?;
    if rest.len() != DIGEST_HEX_LEN {
        return None;
    }
    if !rest
        .bytes()
        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    if value.contains('/') || value.contains('\\') || value.contains("..") {
        return None;
    }
    Some(value.to_owned())
}

#[cfg(test)]
pub(in crate::environments) mod tests;
