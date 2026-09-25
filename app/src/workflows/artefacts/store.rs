use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::id::ObjectHash;
use crate::storage;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ArtefactStoreError {
    Persist,
    Integrity,
    Missing,
}

impl ArtefactStoreError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Persist => "Frinkworks could not store that artefact. Try again.",
            Self::Integrity => "An artefact object failed an integrity check.",
            Self::Missing => "An artefact object is missing.",
        }
    }
}

pub(crate) struct WorkflowArtefactRepository {
    root: Option<PathBuf>,
    inner: Mutex<MemoryObjects>,
    #[cfg(test)]
    fail_publish_after: Mutex<Option<usize>>,
}

struct MemoryObjects {
    objects: Vec<(ObjectHash, Vec<u8>)>,
}

impl WorkflowArtefactRepository {
    pub(crate) fn open(root: PathBuf) -> Result<Self, ArtefactStoreError> {
        let objects = root.join("objects").join("sha256");
        storage::ensure_private_dir(&objects).map_err(|_| ArtefactStoreError::Persist)?;
        let tmp = root.join("tmp");
        storage::ensure_private_dir(&tmp).map_err(|_| ArtefactStoreError::Persist)?;
        remove_tmp(&tmp)?;
        Ok(Self {
            root: Some(root),
            inner: Mutex::new(MemoryObjects {
                objects: Vec::new(),
            }),
            #[cfg(test)]
            fail_publish_after: Mutex::new(None),
        })
    }

    pub(crate) fn publish(&self, bytes: &[u8]) -> Result<ObjectHash, ArtefactStoreError> {
        #[cfg(test)]
        {
            let mut failure = self
                .fail_publish_after
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(remaining) = failure.as_mut() {
                if *remaining == 0 {
                    *failure = None;
                    return Err(ArtefactStoreError::Persist);
                }
                *remaining -= 1;
            }
        }
        let hash = ObjectHash::of(bytes);
        if let Some(root) = &self.root {
            publish_disk(root, hash, bytes)?;
        } else {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some((_, stored)) = inner.objects.iter().find(|(item, _)| *item == hash) {
                if stored.as_slice() != bytes {
                    return Err(ArtefactStoreError::Integrity);
                }
            } else {
                inner.objects.push((hash, bytes.to_vec()));
            }
        }
        Ok(hash)
    }

    pub(crate) fn get(&self, hash: &ObjectHash) -> Result<Vec<u8>, ArtefactStoreError> {
        if let Some(root) = &self.root {
            return read_disk(root, hash);
        }
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner
            .objects
            .iter()
            .find(|(item, _)| item == hash)
            .map(|(_, bytes)| bytes.clone())
            .ok_or(ArtefactStoreError::Missing)
    }
}

fn object_path(root: &Path, hash: &ObjectHash) -> Result<PathBuf, ArtefactStoreError> {
    let (fanout, rest) = hash.fanout();
    let dir = root.join("objects").join("sha256").join(fanout);
    Ok(dir.join(rest))
}

fn publish_disk(root: &Path, hash: ObjectHash, bytes: &[u8]) -> Result<(), ArtefactStoreError> {
    let path = object_path(root, &hash)?;
    if path.exists() {
        let stored = fs::read(&path).map_err(|_| ArtefactStoreError::Persist)?;
        if stored.as_slice() != bytes || ObjectHash::of(&stored) != hash {
            return Err(ArtefactStoreError::Integrity);
        }
        return Ok(());
    }
    let dir = path.parent().ok_or(ArtefactStoreError::Persist)?;
    storage::ensure_private_dir(dir).map_err(|_| ArtefactStoreError::Persist)?;
    let tmp_dir = root.join("tmp");
    storage::ensure_private_dir(&tmp_dir).map_err(|_| ArtefactStoreError::Persist)?;
    let (tmp, mut file) = create_tmp(&tmp_dir)?;
    let result: io::Result<()> = (|| {
        restrict(&file)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, &path)?;
        File::open(dir)?.sync_all()
    })();
    match result {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = fs::remove_file(&tmp);
            Err(ArtefactStoreError::Persist)
        }
    }
}

fn read_disk(root: &Path, hash: &ObjectHash) -> Result<Vec<u8>, ArtefactStoreError> {
    let path = object_path(root, hash)?;
    let mut file = File::open(&path).map_err(|_| ArtefactStoreError::Missing)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| ArtefactStoreError::Persist)?;
    if ObjectHash::of(&bytes) != *hash {
        return Err(ArtefactStoreError::Integrity);
    }
    Ok(bytes)
}

fn create_tmp(dir: &Path) -> Result<(PathBuf, File), ArtefactStoreError> {
    use rand::rand_core::TryRng;
    use rand::rngs::SysRng;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for _ in 0..16 {
        let mut bytes = [0u8; 8];
        SysRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| ArtefactStoreError::Persist)?;
        let mut name = String::from(".");
        for byte in bytes {
            name.push(HEX[(byte >> 4) as usize] as char);
            name.push(HEX[(byte & 0x0f) as usize] as char);
        }
        name.push_str(".tmp");
        let path = dir.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(ArtefactStoreError::Persist),
        }
    }
    Err(ArtefactStoreError::Persist)
}

fn restrict(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = file.metadata()?.permissions();
        permissions.set_mode(0o600);
        file.set_permissions(permissions)?;
    }
    #[cfg(not(unix))]
    {
        let _ = file;
    }
    Ok(())
}

fn remove_tmp(dir: &Path) -> Result<(), ArtefactStoreError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(ArtefactStoreError::Persist),
    };
    for entry in entries {
        let entry = entry.map_err(|_| ArtefactStoreError::Persist)?;
        let _ = fs::remove_file(entry.path());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
