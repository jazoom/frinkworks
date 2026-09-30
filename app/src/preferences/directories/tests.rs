use super::*;

#[test]
fn history_is_bounded_deduplicated_and_persistent_without_consent_eviction() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("preferences.json");
    let preferences = Preferences::open(path.clone());
    let mut grants = Vec::new();
    for index in 0..12 {
        let directory = root.path().join(format!("project-{index}"));
        std::fs::create_dir(&directory).unwrap();
        let grant = DirectoryGrant::from_selected(&directory, &[]).unwrap();
        preferences.remember_directory(&grant, index == 0).unwrap();
        grants.push(grant);
    }
    let order = preferences
        .recent_directories()
        .into_iter()
        .map(|entry| entry.id)
        .collect::<Vec<_>>();
    let mut earlier = DirectoryGrant::from_selected(&grants[6].host_path, &[]).unwrap();
    earlier.access = DirectoryAccess::Write;
    preferences.remember_directory(&earlier, false).unwrap();
    let mut latest = grants[11].clone();
    latest.access = DirectoryAccess::Write;
    preferences.remember_directory(&latest, false).unwrap();
    let reader = Preferences::open(path.clone());
    let entries = reader.recent_directories();
    assert_eq!(entries.len(), 10);
    assert_eq!(entries[0].host_path, latest.host_path);
    assert_eq!(entries[0].access, DirectoryAccess::Write);
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>(),
        order
    );
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.host_path == earlier.host_path)
            .unwrap()
            .access,
        DirectoryAccess::Write
    );
    assert!(reader.directory_approved(&grants[0]));
    assert!(!reader.directory_approved(&grants[1]));
    reader.forget_directory(&entries[0].id).unwrap();
    let reader = Preferences::open(path);
    assert_eq!(reader.recent_directories().len(), 9);
    assert!(
        !reader
            .recent_directories()
            .iter()
            .any(|entry| entry.host_path == latest.host_path)
    );
    assert!(reader.directory_approved(&grants[0]));
}

#[test]
fn access_updates_keep_shortcuts_forgotten_and_preserve_exact_path_consent() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("preferences.json");
    let preferences = Preferences::open(path.clone());
    let mut grant = DirectoryGrant::from_selected(root.path(), &[]).unwrap();
    preferences.remember_directory(&grant, true).unwrap();
    grant.access = DirectoryAccess::Write;
    preferences.update_directory_access(&grant, false).unwrap();
    assert_eq!(
        preferences.recent_directories()[0].access,
        DirectoryAccess::Write
    );
    preferences.forget_directory(&grant.id.as_hex()).unwrap();
    grant.access = DirectoryAccess::Read;
    preferences.update_directory_access(&grant, true).unwrap();
    let reader = Preferences::open(path);
    assert!(reader.recent_directories().is_empty());
    assert!(reader.directory_approved(&grant));
}

#[test]
fn malformed_or_oversized_directory_history_cannot_restore_consent() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("preferences.json");
    let grant = DirectoryGrant::from_selected(root.path(), &[]).unwrap();
    let preferences = Preferences::open(path.clone());
    preferences.remember_directory(&grant, true).unwrap();
    let valid: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let mut cases = Vec::new();
    for field in ["id", "host_path", "access"] {
        let mut file = valid.clone();
        file["recent_directories"][0][field] = "invalid".into();
        cases.push(file);
    }
    let mut file = valid.clone();
    file["recent_directories"] = serde_json::json!(vec![file["recent_directories"][0].clone(); 11]);
    cases.push(file);
    let mut file = valid.clone();
    file["approved_directories"] = serde_json::json!(["relative/path"]);
    cases.push(file);
    let mut file = valid.clone();
    file["approved_directories"] = serde_json::json!(vec![grant.host_path.clone(); 1_025]);
    cases.push(file);
    for file in cases {
        let bytes = serde_json::to_vec(&file).unwrap();
        crate::storage::write_private(&path, &bytes).unwrap();
        let reader = Preferences::open(path.clone());
        assert!(reader.recent_directories().is_empty());
        assert!(!reader.directory_approved(&grant));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}
