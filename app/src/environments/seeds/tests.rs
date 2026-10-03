use super::{ALPINE_GIT_V1, EnvironmentSeed, SeedKey, alpine_git_draft};
use crate::environments::catalogue::EnvironmentCatalogue;
use crate::environments::recipe::EnvironmentDraft;

fn temp_catalogue(seeds: &[EnvironmentSeed]) -> (tempfile::TempDir, EnvironmentCatalogue) {
    let dir = tempfile::tempdir().expect("dir");
    let catalogue = EnvironmentCatalogue::open_with_seeds(
        dir.path().join("environments.json"),
        dir.path().join("environment-preparation-logs"),
        seeds,
    )
    .expect("open");
    (dir, catalogue)
}

#[test]
fn first_open_seeds_alpine_git_once() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("environments.json");
    let logs = dir.path().join("logs");
    let first = EnvironmentCatalogue::open(path.clone(), logs.clone()).expect("open");
    let records = first.list();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].name, "Alpine Git");
    assert_eq!(records[0].recipe.oci_image.as_str(), "alpine/git");
    assert_eq!(
        records[0].recipe.setup_script,
        "apk add --no-cache ripgrep\n"
    );
    assert_eq!(first.applied_seed_count(), 1);
    assert_eq!(first.preparation_count(), 1);
    let second = EnvironmentCatalogue::open(path, logs).expect("reopen");
    assert_eq!(second.list().len(), 1);
    assert_eq!(second.list()[0].id, records[0].id);
    assert_eq!(second.preparation_count(), 1);
}

#[test]
fn restart_preserves_an_edited_seeded_environment() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("environments.json");
    let logs = dir.path().join("logs");
    let catalogue = EnvironmentCatalogue::open(path.clone(), logs.clone()).expect("open");
    let seeded = catalogue.list().into_iter().next().expect("seed");
    catalogue
        .update(
            &seeded.id,
            seeded.revision,
            EnvironmentDraft {
                name: "Edited alpine".to_owned(),
                oci_image: seeded.recipe.oci_image.as_str().to_owned(),
                setup_script: seeded.recipe.setup_script.clone(),
            },
        )
        .expect("edit");
    let reopened = EnvironmentCatalogue::open(path, logs).expect("reopen");
    let loaded = reopened.list();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, seeded.id);
    assert_eq!(loaded[0].name, "Edited alpine");
    assert_eq!(loaded[0].recipe_version, seeded.recipe_version);
    assert_eq!(reopened.preparation_count(), 1);
}

#[test]
fn restart_does_not_restore_a_deleted_seeded_environment() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("environments.json");
    let logs = dir.path().join("logs");
    let catalogue = EnvironmentCatalogue::open(path.clone(), logs.clone()).expect("open");
    let seeded = catalogue.list().into_iter().next().expect("seed");
    catalogue
        .delete(&seeded.id, seeded.revision)
        .expect("delete");
    assert!(catalogue.list().is_empty());
    assert!(catalogue.retired_ids().contains(&seeded.id));
    assert_eq!(catalogue.applied_seed_count(), 1);
    let reopened = EnvironmentCatalogue::open(path, logs).expect("reopen");
    assert!(reopened.list().is_empty());
    assert!(reopened.retired_ids().contains(&seeded.id));
    assert_eq!(reopened.applied_seed_count(), 1);
}

#[test]
fn a_present_seed_key_is_not_reapplied_from_code() {
    let (_dir, catalogue) = temp_catalogue(&[EnvironmentSeed {
        key: SeedKey::parse(ALPINE_GIT_V1).expect("key"),
        draft: EnvironmentDraft {
            name: "Custom alpine".to_owned(),
            oci_image: "alpine/git".to_owned(),
            setup_script: String::new(),
        },
    }]);
    assert_eq!(catalogue.list()[0].name, "Custom alpine");
}

#[test]
fn production_inputs_contain_no_seed_provenance() {
    let draft = alpine_git_draft();
    let text = format!("{} {} {}", draft.name, draft.oci_image, draft.setup_script);
    assert!(!text.contains(ALPINE_GIT_V1));
    assert!(!text.contains("built_in"));
    assert!(!text.contains("built-in"));
}
