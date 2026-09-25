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
fn duplicate_directory_identity_is_invalid_at_distinct_paths() {
    let parent = tempfile::tempdir().unwrap();
    let first = super::DirectoryGrant::from_selected(parent.path(), &[]).unwrap();
    let mut duplicate = first.clone();
    duplicate.id = super::DirectoryGrantId::generate().unwrap();
    duplicate.alias = "other".to_owned();
    duplicate.host_path = parent.path().with_extension("alias");
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
