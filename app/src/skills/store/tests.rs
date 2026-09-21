use super::*;

impl SkillStore {
    pub(crate) fn temporary() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Self::open(directory.path().to_owned()).unwrap();
        store._temporary = Some(directory);
        store
    }
}

fn markdown(name: &str) -> String {
    format!("---\nname: {name}\ndescription: Example task\n---\nInstructions.\n")
}

#[test]
fn copied_files_are_live_and_external_edits_invalidate_mutations() {
    let store = SkillStore::temporary();
    let folder = store.dir.join("copied-skill");
    fs::create_dir(&folder).unwrap();
    let path = folder.join(SKILL_FILE_NAME);
    fs::write(&path, markdown("external")).unwrap();
    let record = store.list().unwrap().pop().unwrap();
    assert_eq!(record.directory, "copied-skill");
    fs::write(&path, markdown("changed outside")).unwrap();
    assert_eq!(
        store.get(&record.directory).unwrap().name,
        "changed outside"
    );
    assert_eq!(
        store.update(&record.directory, &record.fingerprint, markdown("stale")),
        Err(SkillError::Conflict)
    );
    assert_eq!(
        store.delete(&record.directory, &record.fingerprint),
        Err(SkillError::Conflict)
    );
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        markdown("changed outside")
    );
}

#[test]
fn paths_and_file_sizes_are_bounded_before_reads() {
    let store = SkillStore::temporary();
    for directory in [
        ".",
        "..",
        "../outside",
        "/tmp/outside",
        "a/b",
        "a\\b",
        "bad?query",
    ] {
        assert_eq!(store.get(directory), Err(SkillError::Directory));
    }
    let folder = store.dir.join("oversized");
    fs::create_dir(&folder).unwrap();
    fs::File::create(folder.join(SKILL_FILE_NAME))
        .unwrap()
        .set_len(MAXIMUM_SKILL_BODY_BYTES as u64 + 1)
        .unwrap();
    assert_eq!(store.get("oversized"), Err(SkillError::Bound));
    assert_eq!(store.list(), Err(SkillError::Bound));
}

#[cfg(unix)]
#[test]
fn symlinked_roots_directories_and_files_are_rejected_without_external_effects() {
    use std::os::unix::fs::symlink;
    let store = SkillStore::temporary();
    let outside = tempfile::tempdir().unwrap();
    let external = outside.path().join(SKILL_FILE_NAME);
    fs::write(&external, markdown("external")).unwrap();
    symlink(outside.path(), store.dir.join("linked-directory")).unwrap();
    assert_eq!(store.get("linked-directory"), Err(SkillError::Unsafe));
    fs::remove_file(store.dir.join("linked-directory")).unwrap();
    fs::create_dir(store.dir.join("linked-file")).unwrap();
    symlink(
        &external,
        store.dir.join("linked-file").join(SKILL_FILE_NAME),
    )
    .unwrap();
    assert_eq!(store.get("linked-file"), Err(SkillError::Unsafe));
    assert_eq!(
        store.delete("linked-file", "forged"),
        Err(SkillError::Unsafe)
    );
    assert_eq!(fs::read_to_string(&external).unwrap(), markdown("external"));
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("skills");
    symlink(outside.path(), &root).unwrap();
    assert!(matches!(SkillStore::open(root), Err(SkillError::Unsafe)));
}

#[test]
fn duplicate_names_and_catalogue_bounds_do_not_overwrite_files() {
    let store = SkillStore::temporary();
    let original = store.create(markdown("original")).unwrap();
    assert_eq!(
        store.create(markdown("original")),
        Err(SkillError::Conflict)
    );
    assert_eq!(store.get(&original.directory).unwrap(), original);
    for index in 1..MAXIMUM_SKILLS {
        fs::create_dir(store.dir.join(format!("folder-{index}"))).unwrap();
    }
    assert_eq!(store.create(markdown("overflow")), Err(SkillError::Full));
    fs::create_dir(store.dir.join("external-overflow")).unwrap();
    assert_eq!(store.list(), Err(SkillError::Full));
}
