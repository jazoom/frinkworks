use crate::{
    agents::{NetworkAccess, ToolId},
    providers::{ModelSelection, ProviderKind},
};

use super::ProjectFreeAuthority;

fn settings() -> crate::execution::ExecutionSettings {
    crate::execution::ExecutionSettings::new(
        ModelSelection::new(ProviderKind::Xai, "test-model".to_owned(), None).unwrap(),
        String::new(),
        vec![ToolId::List, ToolId::Write],
        crate::tests::test_environment_id(),
    )
    .and_then(|settings| settings.with_network(NetworkAccess::Public))
    .unwrap()
}

#[test]
fn empty_host_grants_resolve_only_the_private_workspace() {
    let authority = ProjectFreeAuthority::from_settings(7, &settings()).unwrap();

    assert!(authority.policy.grants().is_empty());
    assert_eq!(authority.policy.primary_guest(), "/workspace");
    assert!(authority.policy.resolve("notes.txt").is_ok());
    assert!(authority.policy.resolve("/project/secret").is_err());
    assert_eq!(authority.network, NetworkAccess::Public);
}

#[test]
fn equal_basenames_receive_stable_distinct_aliases() {
    let parent = tempfile::tempdir().unwrap();
    let first = parent.path().join("one").join("src");
    let second = parent.path().join("two").join("src");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    let first = crate::execution::DirectoryGrant::from_selected(&first, &[]).unwrap();
    let second =
        crate::execution::DirectoryGrant::from_selected(&second, std::slice::from_ref(&first))
            .unwrap();

    assert_eq!(first.alias, "src");
    assert_eq!(second.alias, "src-2");
    assert_eq!(first.guest_path(), "/access/src");
    assert_eq!(second.guest_path(), "/access/src-2");
}

#[cfg(unix)]
#[test]
fn symbolic_aliases_cannot_duplicate_a_canonical_root() {
    let parent = tempfile::tempdir().unwrap();
    let target = parent.path().join("target");
    let link = parent.path().join("shortcut");
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let target_grant = crate::execution::DirectoryGrant::from_selected(&target, &[]).unwrap();

    assert_eq!(
        crate::execution::DirectoryGrant::from_selected(&link, &[target_grant]),
        Err(crate::execution::DirectoryGrantError::Duplicate)
    );
}

#[test]
fn power_plant_data_classification_covers_root_descendants_and_ancestors() {
    let home = tempfile::tempdir().unwrap();
    let data = home.path().join(".local").join("power-plant");
    let child = data.join("providers");
    std::fs::create_dir_all(&child).unwrap();

    assert_eq!(
        super::classify_sensitive_directory(&data, &data),
        Some(super::SensitiveDirectory::PowerPlantData)
    );
    assert_eq!(
        super::classify_sensitive_directory(&child, &data),
        Some(super::SensitiveDirectory::PowerPlantData)
    );
    assert_eq!(
        super::classify_sensitive_directory(home.path(), &data),
        Some(super::SensitiveDirectory::PowerPlantData)
    );
    let unrelated = tempfile::tempdir().unwrap();
    assert_eq!(
        super::classify_sensitive_directory(unrelated.path(), &data),
        None
    );
}

#[test]
fn replacement_at_the_same_path_invalidates_authority() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("root");
    std::fs::create_dir(&root).unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(&root, &[]).unwrap();
    let configured = settings().with_directories(vec![grant]).unwrap();
    std::fs::rename(&root, parent.path().join("original")).unwrap();
    std::fs::create_dir(&root).unwrap();

    assert_eq!(
        ProjectFreeAuthority::from_settings(2, &configured),
        Err(crate::execution::DirectoryGrantError::Unavailable)
    );
}

#[test]
fn directory_order_does_not_select_a_git_destination() {
    let parent = tempfile::tempdir().unwrap();
    let first_path = parent.path().join("first");
    let second_path = parent.path().join("second");
    std::fs::create_dir(&first_path).unwrap();
    std::fs::create_dir(&second_path).unwrap();
    let first = crate::execution::DirectoryGrant::from_selected(&first_path, &[]).unwrap();
    let second =
        crate::execution::DirectoryGrant::from_selected(&second_path, std::slice::from_ref(&first))
            .unwrap();
    let settings = settings()
        .with_directories(vec![first.clone(), second.clone()])
        .unwrap();
    let authority = ProjectFreeAuthority::from_settings(1, &settings).unwrap();
    assert_eq!(authority.policy.primary_alias(), "");
    let selected = settings.with_git_destination(Some(second.id)).unwrap();
    let authority = ProjectFreeAuthority::from_settings(1, &selected).unwrap();
    assert_eq!(authority.policy.primary_alias(), second.alias);
}
