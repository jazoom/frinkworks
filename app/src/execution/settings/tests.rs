use crate::{
    agents::ToolId,
    providers::{ModelSelection, ProviderKind},
};

use super::ExecutionSettings;

fn model() -> ModelSelection {
    ModelSelection::new(ProviderKind::Xai, "test-model".to_owned(), None).unwrap()
}

#[test]
fn settings_reject_duplicate_tools_and_control_characters() {
    assert!(
        ExecutionSettings::new(
            model(),
            String::new(),
            vec![ToolId::Read, ToolId::Read],
            crate::tests::test_environment_id(),
        )
        .is_none()
    );
    assert!(
        ExecutionSettings::new(
            model(),
            "bad\0text".to_owned(),
            Vec::new(),
            crate::tests::test_environment_id(),
        )
        .is_none()
    );
}

#[test]
fn project_basename_does_not_use_the_reserved_workflow_alias() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("project");
    std::fs::create_dir(&path).unwrap();
    let grant = super::DirectoryGrant::from_selected(&path, &[]).unwrap();
    assert!(
        crate::workflows::definition::AgentAuthority::new(
            vec![ToolId::Read],
            vec![crate::workflows::definition::GuestDirectoryAccess {
                alias: grant.alias,
                access: crate::agents::AccessMode::ReadOnly,
            }],
        )
        .is_ok()
    );
}

#[test]
fn multiple_directories_can_use_write() {
    let root = tempfile::tempdir().unwrap();
    let first_path = root.path().join("first");
    let second_path = root.path().join("second");
    std::fs::create_dir(&first_path).unwrap();
    std::fs::create_dir(&second_path).unwrap();
    let mut first = super::DirectoryGrant::from_selected(&first_path, &[]).unwrap();
    first.access = super::DirectoryAccess::Write;
    let mut second =
        super::DirectoryGrant::from_selected(&second_path, std::slice::from_ref(&first)).unwrap();
    second.access = super::DirectoryAccess::Write;

    assert_eq!(super::validate_directories(&[first, second]), Ok(()));
}

#[test]
fn duplicate_directory_path_is_invalid_with_distinct_grant_ids() {
    let parent = tempfile::tempdir().unwrap();
    let first = super::DirectoryGrant::from_selected(parent.path(), &[]).unwrap();
    let mut duplicate = first.clone();
    duplicate.id = super::DirectoryGrantId::generate().unwrap();
    duplicate.alias = "other".to_owned();
    assert!(
        ExecutionSettings::new(
            model(),
            String::new(),
            Vec::new(),
            crate::tests::test_environment_id(),
        )
        .unwrap()
        .with_directories(vec![first, duplicate])
        .is_none()
    );
}

#[test]
fn combined_ceiling_uses_write_without_changing_read_settings() {
    let root = tempfile::tempdir().unwrap();
    let mut grant = super::DirectoryGrant::from_selected(root.path(), &[]).unwrap();
    grant.access = super::DirectoryAccess::Write;
    let direct = ExecutionSettings::new(
        model(),
        String::new(),
        vec![ToolId::Write],
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_directories(vec![grant])
    .unwrap();
    let mut reviewed = direct.clone();
    reviewed.directories[0].access = super::DirectoryAccess::Read;
    assert_eq!(
        ExecutionSettings::combined([&reviewed, &direct]).unwrap(),
        direct
    );
}

#[test]
fn sandbox_and_host_locations_cannot_combine() {
    let sandbox = ExecutionSettings::new(
        model(),
        String::new(),
        vec![ToolId::Run],
        crate::tests::test_environment_id(),
    )
    .unwrap();
    let host = sandbox.clone().with_location(super::ToolLocation::Host);
    assert!(ExecutionSettings::combined([&sandbox, &host]).is_none());
    assert_eq!(
        ExecutionSettings::from_file(host.to_file())
            .unwrap()
            .location,
        super::ToolLocation::Host
    );
    assert_eq!(host.host_approval, super::HostApprovalPolicy::AskEachTime);
    let automatic = host
        .clone()
        .with_host_approval(super::HostApprovalPolicy::Automatic);
    assert!(ExecutionSettings::combined([&host, &automatic]).is_none());
    assert_eq!(
        ExecutionSettings::from_file(automatic.to_file())
            .unwrap()
            .host_approval,
        super::HostApprovalPolicy::Automatic
    );
}

#[test]
fn settings_reject_overlapping_directory_roots() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("root");
    let child = root.join("child");
    std::fs::create_dir_all(&child).unwrap();
    let first = crate::execution::DirectoryGrant::from_selected(&root, &[]).unwrap();

    assert_eq!(
        crate::execution::DirectoryGrant::from_selected(&child, &[first]),
        Err(crate::execution::DirectoryGrantError::Overlap)
    );
}

#[test]
fn a_non_git_directory_remains_valid_for_file_work() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("notes");
    std::fs::create_dir(&path).unwrap();
    let grant = super::DirectoryGrant::from_selected(&path, &[]).unwrap();
    let settings = ExecutionSettings::new(
        model(),
        String::new(),
        vec![ToolId::Read],
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_directories(vec![grant.clone()])
    .unwrap();
    let authority = crate::execution::ProjectFreeAuthority::from_settings(1, &settings).unwrap();
    assert_eq!(authority.policy.grants().len(), 1);
    assert_eq!(authority.policy.grants()[0].host_path, grant.host_path);
    assert_eq!(grant.access, super::DirectoryAccess::Read);
}

#[test]
fn persisted_grants_follow_the_path_when_a_directory_returns() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("source");
    std::fs::create_dir(&path).unwrap();
    let mut grant = super::DirectoryGrant::from_selected(&path, &[]).unwrap();
    grant.access = super::DirectoryAccess::Write;
    let settings = ExecutionSettings::new(
        model(),
        String::new(),
        vec![ToolId::Write],
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_directories(vec![grant.clone()])
    .unwrap();
    let file = serde_json::to_string(&settings.to_file()).unwrap();
    let form = grant.form_value();
    std::fs::rename(&path, parent.path().join("original")).unwrap();
    assert_eq!(
        grant.revalidate(),
        Err(super::DirectoryGrantError::Unavailable)
    );
    std::fs::write(&path, "not a directory").unwrap();
    assert_eq!(
        grant.revalidate(),
        Err(super::DirectoryGrantError::Unavailable)
    );
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();

    let restored = ExecutionSettings::from_file(serde_json::from_str(&file).unwrap()).unwrap();
    assert_eq!(restored, settings);
    assert_eq!(restored.directories[0].revalidate(), Ok(()));
    assert_eq!(super::DirectoryGrant::parse_form(&form), Some(grant));
    let value: serde_json::Value = serde_json::from_str(&file).unwrap();
    for field in ["identity", "device", "inode"] {
        assert!(value["directories"][0].get(field).is_none());
    }
}

#[cfg(unix)]
#[test]
fn directory_open_rejects_transient_root_and_ancestor_redirects() {
    let parent = tempfile::tempdir().unwrap();
    let parent = parent.path().canonicalize().unwrap();
    let source = parent.join("source");
    let original = parent.join("original");
    let external = parent.join("external");
    std::fs::create_dir_all(source.join("nested")).unwrap();
    std::fs::create_dir_all(external.join("nested")).unwrap();

    for path in [&source, &source.join("nested")] {
        let grant = super::DirectoryGrant::from_selected(path, &[]).unwrap();
        grant.revalidate().unwrap();
        std::fs::rename(&source, &original).unwrap();
        std::os::unix::fs::symlink(&external, &source).unwrap();

        // Exercise the actual open between successful path checks, without scheduler timing.
        let opened = super::open_directory_without_links(&grant.host_path);
        std::fs::remove_file(&source).unwrap();
        std::fs::rename(&original, &source).unwrap();
        grant.revalidate().unwrap();
        assert!(opened.is_err(), "a transient link must not supply a handle");
        assert!(grant.open_directory().is_ok());
    }
}

#[cfg(unix)]
#[test]
fn open_directory_rejects_redirects_and_retains_the_open_root() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("source");
    let old = parent.path().join("original");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("file"), "original").unwrap();
    let grant = super::DirectoryGrant::from_selected(&path, &[]).unwrap();
    let opened = grant.open_directory().unwrap();
    std::fs::rename(&path, &old).unwrap();
    assert!(grant.open_directory().is_err());
    std::os::unix::fs::symlink(&old, &path).unwrap();
    assert!(grant.open_directory().is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("file"), "replacement").unwrap();
    assert_eq!(opened.read_to_string("file").unwrap(), "original");
    assert_eq!(
        grant
            .open_directory()
            .unwrap()
            .read_to_string("file")
            .unwrap(),
        "replacement"
    );
    let selected = super::DirectoryGrant::from_selected(&path, &[]).unwrap();
    assert_eq!(
        super::DirectoryGrant::from_selected(&path, &[selected]),
        Err(super::DirectoryGrantError::Duplicate)
    );
    std::fs::remove_dir_all(&path).unwrap();
    std::os::unix::fs::symlink(&old, &path).unwrap();
    let original = super::DirectoryGrant::from_selected(&old, &[]).unwrap();
    assert_eq!(
        super::DirectoryGrant::from_selected(&path, &[original]),
        Err(super::DirectoryGrantError::Duplicate)
    );
}
