use super::*;

#[test]
fn direct_snapshots_retain_the_original_files_and_reject_a_replaced_directory() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("file.txt"), "before\n").unwrap();
    let mut grant = crate::execution::DirectoryGrant::from_selected(&path, &[]).unwrap();
    grant.access = DirectoryAccess::DirectWrite;
    let capture = || {
        CandidateCapture::capture_set(
            std::slice::from_ref(&grant),
            state.local_data.root(),
            &state.workflow_artefacts,
        )
    };
    let before = capture().unwrap().manifest_bytes().unwrap();
    let before = state.workflow_artefacts.publish(&before).unwrap().as_str();
    std::fs::write(path.join("file.txt"), "after\n").unwrap();
    let after = capture().unwrap().manifest_bytes().unwrap();
    let after = state.workflow_artefacts.publish(&after).unwrap().as_str();
    let changes = DirectChanges {
        before,
        after: Some(after),
    };
    std::fs::write(path.join("file.txt"), "later host edit\n").unwrap();
    let diff = changes.diff(&state).unwrap();
    let change = diff.change(0, &state.workflow_artefacts).unwrap();
    let text: String = change
        .text
        .unwrap()
        .into_iter()
        .map(|part| part.text)
        .collect();
    assert!(text.contains("-before"));
    assert!(text.contains("+after"));
    assert!(!text.contains("later host edit"));
    std::fs::rename(&path, root.path().join("original")).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(capture().is_err());
    assert!(changes.diff(&state).is_some());
}
