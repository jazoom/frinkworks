impl super::WorkflowArtefactRepository {
    pub(crate) fn in_memory() -> Self {
        Self {
            root: None,
            inner: Mutex::new(MemoryObjects {
                objects: Vec::new(),
            }),
            fail_publish_after: Mutex::new(None),
        }
    }
    pub(crate) fn fail_publish_after(&self, successful_publications: usize) {
        *self
            .fail_publish_after
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(successful_publications);
    }
}

use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn failed_publication_exposes_no_object() {
    let store = WorkflowArtefactRepository::in_memory();
    store.fail_publish_after(0);
    assert!(store.publish(b"new plan").is_err());
    assert!(store.get(&ObjectHash::of(b"new plan")).is_err());
}

#[test]
fn publication_is_atomic_and_deduplicates_identical_bytes() {
    let dir = tempfile::tempdir().expect("dir");
    let store = WorkflowArtefactRepository::open(dir.path().to_path_buf()).expect("open");
    let hash = store.publish(b"hello").expect("publish");
    assert_eq!(store.get(&hash).expect("get"), b"hello");
    assert_eq!(store.publish(b"hello").expect("again"), hash);
    let path = {
        let (fanout, rest) = hash.fanout();
        dir.path()
            .join("objects")
            .join("sha256")
            .join(fanout)
            .join(rest)
    };
    let mode = fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn a_tampered_object_fails_integrity() {
    let dir = tempfile::tempdir().expect("dir");
    let store = WorkflowArtefactRepository::open(dir.path().to_path_buf()).expect("open");
    let hash = store.publish(b"hello").expect("publish");
    let (fanout, rest) = hash.fanout();
    let path = dir
        .path()
        .join("objects")
        .join("sha256")
        .join(fanout)
        .join(rest);
    fs::write(&path, b"other").expect("tamper");
    assert_eq!(store.get(&hash).err(), Some(ArtefactStoreError::Integrity));
}

#[test]
fn stale_temporary_files_are_removed_on_open() {
    let dir = tempfile::tempdir().expect("dir");
    let tmp = dir.path().join("tmp");
    fs::create_dir_all(&tmp).expect("tmp");
    fs::write(tmp.join(".abc.tmp"), b"leftover").expect("write");
    let _ = WorkflowArtefactRepository::open(dir.path().to_path_buf()).expect("open");
    assert!(fs::read_dir(&tmp).expect("read").next().is_none());
}
