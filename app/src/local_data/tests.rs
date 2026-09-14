use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::{
    CatalogueResetConflict, Inner, OWNERSHIP_CONTENTS, OWNERSHIP_MARKER_NAME, RESET_CONTENTS,
    RESET_MARKER_NAME, ResetRequest,
};
use crate::agents::{AccessMode, AgentId, AgentRecord, AgentStore, DirectoryGrant};
use crate::config::{RuntimeConfig, StartupConfig};
use crate::conversations::ConversationStore;
use crate::preferences::{Preferences, Theme};
use crate::projects::{ProjectId, ProjectRecord, ProjectStore};
use crate::providers::{ModelSelection, ProviderConnection, ProviderKind};
use crate::vault::ProviderVault;
use crate::workflows::{WorkflowExecution, WorkflowRunStore};

impl super::LocalDataReset {
    pub(crate) fn detached() -> Self {
        Self::for_test(PathBuf::from("/powerplant-test-local-data"))
    }

    pub(crate) fn for_test(root: PathBuf) -> Self {
        Self {
            root,
            inner: Arc::new(Mutex::new(Inner {
                pending: false,
                execution: None,
            })),
            mutation: Arc::new(tokio::sync::Mutex::new(())),
        }
    }
}

fn test_config(data_dir: PathBuf, protected_user_roots: Vec<PathBuf>) -> StartupConfig {
    StartupConfig {
        bind_address: "localhost:4000".to_owned(),
        runtime: RuntimeConfig::development(),
        static_dir: PathBuf::from("/tmp/powerplant-static"),
        data_dir,
        protected_user_roots,
    }
}

fn prepare(data_dir: PathBuf) -> (StartupConfig, super::LocalDataReset) {
    super::prepare(test_config(data_dir, Vec::new())).expect("prepare")
}

fn listed_names(root: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(root)
        .expect("list")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

fn git_worktree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("worktree");
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .expect("git")
            .success()
    );
    dir
}

fn marker_path(root: &Path, name: &str) -> PathBuf {
    root.join(name)
}

fn assert_secret_safe(error: &str, paths: &[&Path]) {
    for path in paths {
        let displayed = path.to_string_lossy();
        if displayed.is_empty() {
            continue;
        }
        assert!(
            !error.contains(displayed.as_ref()),
            "error included path {displayed}: {error}"
        );
    }
}

#[test]
fn claims_an_absent_root_and_stores_the_canonical_path() {
    let dir = tempfile::tempdir().expect("dir");
    let configured = dir.path().join("data");
    let (config, local_data) = prepare(configured.clone());
    let canonical = configured.canonicalize().expect("canonical");
    assert_eq!(config.data_dir, canonical);
    assert_eq!(local_data.root(), canonical.as_path());
    assert_eq!(listed_names(&canonical), vec![OWNERSHIP_MARKER_NAME]);
    assert_eq!(
        fs::read(marker_path(&canonical, OWNERSHIP_MARKER_NAME)).expect("read"),
        OWNERSHIP_CONTENTS
    );
    assert!(!local_data.is_pending());
}

#[test]
fn claims_an_empty_existing_root() {
    let dir = tempfile::tempdir().expect("dir");
    let root = dir.path().join("data");
    fs::create_dir(&root).expect("create");
    let (_, local_data) = prepare(root.clone());
    assert_eq!(listed_names(local_data.root()), vec![OWNERSHIP_MARKER_NAME]);
}

#[test]
fn rejects_an_unowned_directory_with_a_foreign_entry() {
    let dir = tempfile::tempdir().expect("dir");
    let root = dir.path().join("data");
    fs::create_dir(&root).expect("create");
    fs::write(root.join("README.md"), b"nope").expect("foreign");
    let error = super::prepare(test_config(root.clone(), Vec::new()))
        .err()
        .expect("rejected");
    assert_eq!(
        error,
        "The Power Plant data directory is not a private owned root."
    );
    assert_secret_safe(&error, &[&root]);
    assert!(!marker_path(&root, OWNERSHIP_MARKER_NAME).exists());
}

#[cfg(unix)]
#[test]
fn rejects_a_symbolic_entry_before_claiming_the_root() {
    let dir = tempfile::tempdir().expect("dir");
    let root = dir.path().join("data");
    let outside = dir.path().join("outside-agents");
    fs::create_dir(&root).expect("root");
    fs::create_dir(&outside).expect("outside");
    std::os::unix::fs::symlink(&outside, root.join("agents")).expect("link");

    let error = super::prepare(test_config(root.clone(), Vec::new()))
        .err()
        .expect("rejected");

    assert_eq!(
        error,
        "The Power Plant data directory is not a private owned root."
    );
    assert!(!marker_path(&root, OWNERSHIP_MARKER_NAME).exists());
    assert!(outside.exists());
}

#[test]
fn rejects_a_filesystem_root_and_a_path_without_a_final_component() {
    for path in ["/", "/tmp/..", ""] {
        let error = super::prepare(test_config(PathBuf::from(path), Vec::new()))
            .err()
            .expect("rejected");
        assert_eq!(
            error,
            "POWERPLANT_DATA_DIR must name a directory with a final path component."
        );
        assert_secret_safe(&error, &[Path::new(path)]);
    }
}

#[test]
fn rejects_protected_user_roots_and_allows_a_child_data_root() {
    let home = tempfile::tempdir().expect("home");
    let xdg = tempfile::tempdir().expect("xdg");
    let profile = tempfile::tempdir().expect("profile");
    let local_app = tempfile::tempdir().expect("localapp");
    let protected = vec![
        home.path().to_path_buf(),
        xdg.path().to_path_buf(),
        profile.path().to_path_buf(),
        local_app.path().to_path_buf(),
    ];

    for root in &protected {
        let error = super::prepare(test_config(root.clone(), protected.clone()))
            .err()
            .expect("equals rejected");
        assert_eq!(
            error,
            "The Power Plant data directory is not a private owned root."
        );
        assert_secret_safe(&error, &[root.as_path()]);
        assert!(!marker_path(root, OWNERSHIP_MARKER_NAME).exists());
    }

    let ancestor = home.path().parent().expect("parent").to_path_buf();
    if ancestor.file_name().is_some() {
        let error = super::prepare(test_config(ancestor.clone(), protected.clone()))
            .err()
            .expect("ancestor rejected");
        assert_eq!(
            error,
            "The Power Plant data directory is not a private owned root."
        );
        assert_secret_safe(&error, &[&ancestor, home.path()]);
    }

    let child = home.path().join("share").join("powerplant");
    let (_, local_data) = super::prepare(test_config(child, protected)).expect("child allowed");
    assert_eq!(listed_names(local_data.root()), vec![OWNERSHIP_MARKER_NAME]);
}

#[test]
fn ignores_relative_protected_user_roots() {
    let dir = tempfile::tempdir().expect("dir");
    let root = dir.path().join("data");
    let (_, local_data) = super::prepare(test_config(
        root,
        vec![PathBuf::from("relative-home"), PathBuf::from("AppData")],
    ))
    .expect("ignored");
    assert_eq!(listed_names(local_data.root()), vec![OWNERSHIP_MARKER_NAME]);
}

#[cfg(unix)]
#[test]
fn rejects_a_symbolic_link_or_non_directory_root() {
    let dir = tempfile::tempdir().expect("dir");
    let target = dir.path().join("target");
    fs::create_dir(&target).expect("target");
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&target, &link).expect("link");
    let error = super::prepare(test_config(link.clone(), Vec::new()))
        .err()
        .expect("link rejected");
    assert_eq!(
        error,
        "The Power Plant data directory is not a usable directory."
    );
    assert_secret_safe(&error, &[&link, &target]);
    assert!(!marker_path(&target, OWNERSHIP_MARKER_NAME).exists());

    let file = dir.path().join("file");
    fs::write(&file, b"nope").expect("file");
    let error = super::prepare(test_config(file.clone(), Vec::new()))
        .err()
        .expect("file rejected");
    assert_eq!(
        error,
        "The Power Plant data directory is not a usable directory."
    );
    assert_secret_safe(&error, &[&file]);
}

#[cfg(unix)]
#[test]
fn rejects_malformed_oversized_and_symbolic_markers() {
    let dir = tempfile::tempdir().expect("dir");
    let root = dir.path().join("data");
    let (_, local_data) = prepare(root);
    let root = local_data.root().to_path_buf();
    let outside = dir.path().join("outside");
    fs::write(&outside, OWNERSHIP_CONTENTS).expect("outside");

    fs::remove_file(marker_path(&root, OWNERSHIP_MARKER_NAME)).expect("remove");
    std::os::unix::fs::symlink(&outside, marker_path(&root, OWNERSHIP_MARKER_NAME)).expect("link");
    let error = super::prepare(test_config(root.clone(), Vec::new()))
        .err()
        .expect("ownership symlink");
    assert_eq!(
        error,
        "The Power Plant data directory is not a private owned root."
    );
    assert_eq!(fs::read(&outside).expect("kept"), OWNERSHIP_CONTENTS);

    fs::remove_file(marker_path(&root, OWNERSHIP_MARKER_NAME)).expect("remove link");
    fs::write(marker_path(&root, OWNERSHIP_MARKER_NAME), b"nope").expect("malformed");
    let error = super::prepare(test_config(root.clone(), Vec::new()))
        .err()
        .expect("malformed ownership");
    assert_eq!(
        error,
        "The Power Plant data directory is not a private owned root."
    );

    fs::write(
        marker_path(&root, OWNERSHIP_MARKER_NAME),
        [OWNERSHIP_CONTENTS, b"extra"].concat(),
    )
    .expect("oversized");
    let error = super::prepare(test_config(root.clone(), Vec::new()))
        .err()
        .expect("oversized ownership");
    assert_eq!(
        error,
        "The Power Plant data directory is not a private owned root."
    );

    fs::write(
        marker_path(&root, OWNERSHIP_MARKER_NAME),
        OWNERSHIP_CONTENTS,
    )
    .expect("restore");
    fs::create_dir(marker_path(&root, RESET_MARKER_NAME)).expect("reset dir");
    let error = super::prepare(test_config(root.clone(), Vec::new()))
        .err()
        .expect("reset directory");
    assert_eq!(
        error,
        "The Power Plant data directory is not a private owned root."
    );
    assert!(marker_path(&root, OWNERSHIP_MARKER_NAME).exists());

    fs::remove_dir(marker_path(&root, RESET_MARKER_NAME)).expect("remove dir");
    std::os::unix::fs::symlink(&outside, marker_path(&root, RESET_MARKER_NAME))
        .expect("reset link");
    let error = super::prepare(test_config(root.clone(), Vec::new()))
        .err()
        .expect("reset symlink");
    assert_eq!(
        error,
        "The Power Plant data directory is not a private owned root."
    );
    assert_eq!(fs::read(&outside).expect("kept"), OWNERSHIP_CONTENTS);
    assert!(marker_path(&root, OWNERSHIP_MARKER_NAME).exists());

    fs::remove_file(marker_path(&root, RESET_MARKER_NAME)).expect("remove reset link");
    fs::write(
        marker_path(&root, RESET_MARKER_NAME),
        b"powerplant-reset-v1",
    )
    .expect("malformed");
    let error = super::prepare(test_config(root.clone(), Vec::new()))
        .err()
        .expect("malformed reset");
    assert_eq!(
        error,
        "The Power Plant data directory is not a private owned root."
    );
    assert!(marker_path(&root, OWNERSHIP_MARKER_NAME).exists());
    assert!(root.exists());
}

#[cfg(unix)]
#[test]
fn ownership_and_reset_markers_use_private_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("dir");
    let (_, local_data) = prepare(dir.path().join("data"));
    let root = local_data.root();
    let dir_mode = fs::metadata(root).expect("dir meta").permissions().mode() & 0o777;
    let ownership_mode = fs::metadata(marker_path(root, OWNERSHIP_MARKER_NAME))
        .expect("ownership meta")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700);
    assert_eq!(ownership_mode, 0o600);

    assert_eq!(
        local_data.record_reset_for_test().expect("record"),
        ResetRequest::Recorded
    );
    let reset_mode = fs::metadata(marker_path(root, RESET_MARKER_NAME))
        .expect("reset meta")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(reset_mode, 0o600);
}

#[test]
fn repeated_reset_requests_return_pending_without_another_write() {
    let dir = tempfile::tempdir().expect("dir");
    let (_, local_data) = prepare(dir.path().join("data"));
    assert_eq!(
        local_data.record_reset_for_test().expect("first"),
        ResetRequest::Recorded
    );
    assert!(local_data.is_pending());
    fs::write(
        marker_path(local_data.root(), RESET_MARKER_NAME),
        b"tampered",
    )
    .expect("tamper");
    assert_eq!(
        local_data.record_reset_for_test().expect("second"),
        ResetRequest::Pending
    );
    assert_eq!(
        fs::read(marker_path(local_data.root(), RESET_MARKER_NAME)).expect("read"),
        b"tampered"
    );
}

#[test]
fn reset_request_rejects_an_invalid_existing_marker() {
    let dir = tempfile::tempdir().expect("dir");
    let (_, local_data) = prepare(dir.path().join("data"));
    let marker = marker_path(local_data.root(), RESET_MARKER_NAME);
    fs::write(&marker, [RESET_CONTENTS, b"extra"].concat()).expect("invalid marker");

    assert!(local_data.record_reset_for_test().is_err());
    assert!(!local_data.is_pending());
    assert_eq!(
        fs::read(marker).expect("unchanged marker"),
        [RESET_CONTENTS, b"extra"].concat()
    );
}

#[cfg(unix)]
#[test]
fn reset_deletes_only_the_owned_tree() {
    let dir = tempfile::tempdir().expect("dir");
    let outside = tempfile::tempdir().expect("outside");
    let sentinel = outside.path().join("keep.txt");
    fs::write(&sentinel, b"keep").expect("sentinel");
    let (_, local_data) = prepare(dir.path().join("data"));
    let root = local_data.root().to_path_buf();
    fs::write(root.join("providers.json"), b"secret").expect("owned file");
    std::os::unix::fs::symlink(outside.path(), root.join("link")).expect("link");
    assert_eq!(
        local_data.record_reset_for_test().expect("record"),
        ResetRequest::Recorded
    );

    let (_, local_data) = super::prepare(test_config(root.clone(), Vec::new())).expect("reset");
    assert_eq!(fs::read(&sentinel).expect("outside kept"), b"keep");
    assert_eq!(listed_names(local_data.root()), vec![OWNERSHIP_MARKER_NAME]);
    assert!(!local_data.is_pending());
}

#[test]
fn startup_reset_leaves_no_provider_project_agent_run_or_saved_theme() {
    let dir = tempfile::tempdir().expect("dir");
    let (_, local_data) = prepare(dir.path().join("data"));
    let root = local_data.root().to_path_buf();
    let worktree = git_worktree();

    ProviderVault::open(root.join("providers.json"))
        .expect("vault")
        .insert_api_key(ProviderConnection::with_key(
            ProviderKind::Xai,
            "sk-test-reset-key",
            ProviderKind::Xai.default_model(),
        ))
        .expect("provider");
    ProjectStore::open(root.join("projects.json"))
        .expect("projects")
        .create("Desk".to_owned(), worktree.path().to_path_buf())
        .expect("project");
    fs::create_dir(root.join("agents")).expect("agents");
    fs::write(root.join("agents").join("agent.json"), b"{\"agent\":true}").expect("agent");
    fs::create_dir(root.join("workflow-runs")).expect("runs");
    fs::write(
        root.join("workflow-runs").join("run.json"),
        b"{\"run\":true}",
    )
    .expect("run");
    Preferences::open(root.join("preferences.json"))
        .set_theme(Theme::Sector7G)
        .expect("theme");

    assert_eq!(
        local_data.record_reset_for_test().expect("record"),
        ResetRequest::Recorded
    );
    let (_, local_data) = super::prepare(test_config(root.clone(), Vec::new())).expect("reset");
    let root = local_data.root();

    assert_eq!(listed_names(root), vec![OWNERSHIP_MARKER_NAME]);
    assert!(
        !ProviderVault::open(root.join("providers.json"))
            .expect("vault")
            .has_providers()
    );
    assert!(
        ProjectStore::open(root.join("projects.json"))
            .expect("projects")
            .list()
            .is_empty()
    );
    assert!(
        AgentStore::open(root.join("agents"))
            .expect("agents")
            .list()
            .is_empty()
    );
    assert!(
        WorkflowRunStore::open(root.join("workflow-runs"))
            .expect("runs")
            .summaries()
            .is_empty()
    );
    assert_eq!(
        Preferences::open(root.join("preferences.json")).theme(),
        Theme::System
    );
}

#[test]
fn catalogue_conflict_uses_path_components_at_the_owned_root_boundary() {
    let dir = tempfile::tempdir().expect("dir");
    let (_, local_data) = prepare(dir.path().join("data"));
    let root = local_data.root().to_path_buf();
    let prefix_sibling = root.with_file_name("data-copy");

    assert_eq!(
        local_data.catalogue_conflict(&[project_record(root.clone())], &[], &[], &[], &[]),
        Some(CatalogueResetConflict::Project)
    );
    assert_eq!(
        local_data.catalogue_conflict(&[], &[agent_record(root.join("grant"))], &[], &[], &[]),
        Some(CatalogueResetConflict::AgentGrant)
    );
    assert_eq!(
        local_data.catalogue_conflict(
            &[project_record(prefix_sibling.clone())],
            &[agent_record(prefix_sibling.join("grant")),],
            &[],
            &[],
            &[],
        ),
        None
    );
}

#[test]
fn catalogue_conflict_includes_conversation_directory_grants() {
    let dir = tempfile::tempdir().expect("dir");
    let (_, local_data) = prepare(dir.path().join("data"));
    let granted = local_data.root().join("conversation-source");
    std::fs::create_dir(&granted).unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(&granted, &[]).unwrap();
    let settings = crate::execution::ExecutionSettings::new(
        ModelSelection::new(ProviderKind::Xai, "model".to_owned(), None).unwrap(),
        String::new(),
        Vec::new(),
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_directories(vec![grant])
    .unwrap();
    let store = ConversationStore::in_memory();
    let record = store
        .create_saved(
            crate::conversations::ConversationId::generate().unwrap(),
            None,
            Some("Granted".to_owned()),
            Some(crate::conversations::ConversationModelConfiguration {
                settings,
                preset: None,
            }),
            Vec::new(),
        )
        .unwrap();

    assert_eq!(
        local_data.catalogue_conflict(&[], &[], &[record], &[], &[]),
        Some(CatalogueResetConflict::ConversationGrant)
    );

    let broad_grant = crate::execution::DirectoryGrant::from_selected(dir.path(), &[]).unwrap();
    let broad_settings = crate::execution::ExecutionSettings::new(
        ModelSelection::new(ProviderKind::Xai, "model".to_owned(), None).unwrap(),
        String::new(),
        Vec::new(),
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_directories(vec![broad_grant])
    .unwrap();
    let broad_record = store
        .create_saved(
            crate::conversations::ConversationId::generate().unwrap(),
            None,
            Some("Broad grant".to_owned()),
            Some(crate::conversations::ConversationModelConfiguration {
                settings: broad_settings,
                preset: None,
            }),
            Vec::new(),
        )
        .unwrap();
    assert_eq!(
        local_data.catalogue_conflict(&[], &[], &[broad_record], &[], &[]),
        Some(CatalogueResetConflict::ConversationGrant)
    );
}

#[test]
fn catalogue_conflict_includes_preset_directory_grants() {
    let dir = tempfile::tempdir().expect("dir");
    let (_, local_data) = prepare(dir.path().join("data"));
    let granted = local_data.root().join("preset-source");
    std::fs::create_dir(&granted).unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(&granted, &[]).unwrap();
    let settings = crate::execution::ExecutionSettings::new(
        ModelSelection::new(ProviderKind::Xai, "model".to_owned(), None).unwrap(),
        String::new(),
        Vec::new(),
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_directories(vec![grant])
    .unwrap();
    let store = crate::presets::PresetStore::in_memory();
    let preset = store
        .create(
            "Sensitive",
            settings,
            crate::presets::PresetProvenance::Draft,
        )
        .unwrap();

    assert_eq!(
        local_data.catalogue_conflict(&[], &[], &[], &[preset], &[]),
        Some(CatalogueResetConflict::PresetGrant)
    );
}

#[test]
fn reset_detects_directory_references_in_saved_workflow_overrides() {
    let root = tempfile::tempdir().unwrap();
    let (_, local_data) = prepare(root.path().join("data"));
    let path = local_data.root().join("user-files");
    fs::create_dir(&path).unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(&path, &[]).unwrap();
    let mut definition =
        crate::workflows::seeds::plan_a_change_definition(crate::tests::test_environment_id());
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(
            crate::providers::ProviderKind::Xai,
            "grok-4.6".to_owned(),
            None,
        )
        .unwrap(),
        String::new(),
        Vec::new(),
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_directories(vec![grant])
    .unwrap();
    definition = definition.with_conversation_settings(&settings).unwrap();
    let definition = crate::workflows::definition::WorkflowDefinition::from_file_bytes(
        &serde_json::to_vec(&definition.to_file()).unwrap(),
    )
    .unwrap();
    let record = crate::workflows::WorkflowCatalogue::in_memory()
        .create(definition)
        .unwrap();
    assert_eq!(
        local_data.catalogue_conflict(&[], &[], &[], &[], &[record]),
        Some(CatalogueResetConflict::WorkflowGrant)
    );
}

#[tokio::test]
async fn reset_request_retains_execution_until_process_exit() {
    let dir = tempfile::tempdir().expect("dir");
    let (_, local_data) = prepare(dir.path().join("data"));
    let execution = Arc::new(WorkflowExecution::new());
    let projects = ProjectStore::in_memory();
    let agents = AgentStore::in_memory();
    let conversations = ConversationStore::in_memory();
    let presets = crate::presets::PresetStore::in_memory();

    assert_eq!(
        local_data
            .request_reset(
                &execution,
                &projects,
                &agents,
                &conversations,
                &presets,
                &crate::workflows::WorkflowCatalogue::in_memory(),
            )
            .await
            .expect("record"),
        ResetRequest::Recorded
    );
    assert!(execution.acquire().is_err());
    assert_eq!(
        local_data
            .request_reset(
                &execution,
                &projects,
                &agents,
                &conversations,
                &presets,
                &crate::workflows::WorkflowCatalogue::in_memory(),
            )
            .await
            .expect("repeat"),
        ResetRequest::Pending
    );
    assert!(execution.acquire().is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn failed_marker_write_releases_both_process_permits() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("dir");
    let (_, local_data) = prepare(dir.path().join("data"));
    let root = local_data.root();
    let mut permissions = fs::metadata(root).expect("meta").permissions();
    permissions.set_mode(0o555);
    fs::set_permissions(root, permissions).expect("lock");
    let execution = Arc::new(WorkflowExecution::new());
    let projects = ProjectStore::in_memory();
    let agents = AgentStore::in_memory();
    let conversations = ConversationStore::in_memory();
    let presets = crate::presets::PresetStore::in_memory();

    let failed = local_data
        .request_reset(
            &execution,
            &projects,
            &agents,
            &conversations,
            &presets,
            &crate::workflows::WorkflowCatalogue::in_memory(),
        )
        .await;
    let mut restore = fs::metadata(root).expect("meta").permissions();
    restore.set_mode(0o700);
    fs::set_permissions(root, restore).expect("unlock");

    assert!(failed.is_err());
    assert!(!local_data.is_pending());
    let _execution = execution.acquire().expect("execution released");
    let _host_paths = local_data
        .begin_host_path_mutation()
        .await
        .expect("host paths released");
}

fn project_record(host_path: PathBuf) -> ProjectRecord {
    ProjectRecord {
        id: ProjectId::generate().expect("project"),
        revision: 1,
        name: "Desk".to_owned(),
        host_path,
        created_at_ms: 0,
    }
}

fn agent_record(host_path: PathBuf) -> AgentRecord {
    AgentRecord {
        id: AgentId::generate().expect("agent"),
        revision: 1,
        name: "Worker".to_owned(),
        instructions: String::new(),
        selection: None,
        tools: Vec::new(),
        network: crate::agents::NetworkAccess::None,
        directories: vec![DirectoryGrant {
            alias: "project".to_owned(),
            host_path,
            access: AccessMode::ReadWrite,
        }],
        primary_directory: "project".to_owned(),
    }
}
