use super::*;

impl super::WorkflowRunStore {
    pub(crate) fn in_memory() -> Self {
        Self {
            dir: None,
            inner: Mutex::new(BTreeMap::new()),
            fail_next_mutation: Mutex::new(false),
        }
    }
    pub(crate) fn fail_next_mutation(&self) {
        *self
            .fail_next_mutation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
    }
}

use crate::tests::test_named_definition;
use std::fs;

use super::{BROWSER_SUMMARY_LIMIT, StoreError, WorkflowRunStore};
use crate::workflows::definition::{PinnedWorkflowDefinition, WorkflowDefinition};
use crate::workflows::id::{AttemptId, RunId};
use crate::workflows::run::{RunState, WorkflowRun};

fn definition(name: &str) -> WorkflowDefinition {
    test_named_definition(name)
}

fn start_test_attempt(
    run: &mut WorkflowRun,
    attempt: crate::workflows::id::AttemptId,
    at_ms: u64,
) -> Result<(), crate::workflows::run::TransitionError> {
    let caps = crate::tests::test_agent_capabilities();
    let sandbox = crate::workflows::run::AttemptSandboxRecord {
        kind: crate::workflows::run::AttemptSandboxKind::IsolatedAttempt,
        snapshot_digest: run.environments.steps[0].snapshot_digest.clone(),
    };
    run.start_attempt(attempt, Vec::new(), caps, sandbox, at_ms)
}

fn run_named(name: &str, created_at_ms: u64) -> WorkflowRun {
    run_with_id(name, RunId::generate().expect("run"), created_at_ms)
}

fn run_with_id(name: &str, id: RunId, created_at_ms: u64) -> WorkflowRun {
    let definition = definition(name);
    let environments = crate::tests::test_environment_set(&definition);
    WorkflowRun::configured(
        id,
        created_at_ms,
        crate::agents::AgentId::generate().expect("agent"),
        PinnedWorkflowDefinition::pin(None, definition),
        environments,
    )
}

#[test]
fn a_pinned_version_one_definition_survives_a_source_edit() {
    let dir = tempfile::tempdir().expect("dir");
    let store = WorkflowRunStore::open(dir.path().to_path_buf()).expect("open");
    let first = definition("Original");
    let version = first.version();
    let environments = crate::tests::test_environment_set(&first);
    let run = store
        .create(WorkflowRun::configured(
            RunId::generate().expect("run"),
            1,
            crate::agents::AgentId::generate().expect("agent"),
            PinnedWorkflowDefinition::pin(None, first),
            environments,
        ))
        .expect("create");
    let path = dir.path().join(format!("{}.json", run.id.as_hex()));
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(path).expect("read")).expect("json");
    assert!(value["definition"].get("first-step").is_none());
    assert_eq!(value["version"], version.as_hex());

    let later = definition("Edited");
    assert_ne!(later.version(), version);
    let loaded = WorkflowRunStore::open(dir.path().to_path_buf())
        .expect("reopen")
        .get(&run.id)
        .expect("run");
    assert_eq!(loaded.pinned.version, version);
    assert_eq!(loaded.pinned.definition.name(), "Original");
}

#[test]
fn summaries_keep_the_newest_fifty_runs() {
    let store = WorkflowRunStore::in_memory();
    let definition = definition("Limit");
    let environments = crate::tests::test_environment_set(&definition);
    let mut created = Vec::new();
    for created_at_ms in 1..=BROWSER_SUMMARY_LIMIT as u64 + 1 {
        let run = store
            .create(WorkflowRun::configured(
                RunId::generate().expect("run"),
                created_at_ms,
                crate::agents::AgentId::generate().expect("agent"),
                PinnedWorkflowDefinition::pin(None, definition.clone()),
                environments.clone(),
            ))
            .expect("create");
        created.push(run);
    }
    let summaries = store.summaries();
    let expected: Vec<_> = created.iter().skip(1).rev().map(|run| run.id).collect();
    assert_eq!(
        summaries
            .iter()
            .map(|summary| summary.id)
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn summaries_break_equal_timestamps_by_greatest_run_id() {
    let store = WorkflowRunStore::in_memory();
    let lesser = RunId::parse(&"0".repeat(32)).expect("lesser");
    let greater = RunId::parse(&"f".repeat(32)).expect("greater");
    store
        .create(run_with_id("Lesser", lesser, 10))
        .expect("lesser");
    store
        .create(run_with_id("Greater", greater, 10))
        .expect("greater");
    let summaries = store.summaries();
    assert_eq!(summaries[0].id, greater);
    assert_eq!(summaries[1].id, lesser);
}

#[test]
fn recovery_marks_active_work_as_interrupted() {
    let dir = tempfile::tempdir().expect("dir");
    let store = WorkflowRunStore::open(dir.path().to_path_buf()).expect("open");
    let run = store.create(run_named("Active", 1)).expect("create");
    let attempt = AttemptId::generate().expect("attempt");
    store
        .mutate(&run.id, |run| start_test_attempt(run, attempt, 2))
        .expect("start");
    let reopened = WorkflowRunStore::open(dir.path().to_path_buf()).expect("reopen");
    assert!(reopened.get(&run.id).expect("active run").is_active());
    reopened.interrupt_active().expect("interrupt");
    let loaded = reopened.get(&run.id).expect("run");
    assert_eq!(loaded.state, RunState::Interrupted);
    assert_eq!(
        loaded.attempts[0].state,
        crate::workflows::run::AttemptState::Interrupted
    );
}

#[test]
fn recovery_accepts_cleanup_recorded_before_attempt_finalisation() {
    let dir = tempfile::tempdir().expect("dir");
    let store = WorkflowRunStore::open(dir.path().to_path_buf()).expect("open");
    let run = store.create(run_named("Active", 1)).expect("create");
    let attempt = AttemptId::generate().expect("attempt");
    store
        .mutate(&run.id, |run| start_test_attempt(run, attempt, 2))
        .expect("start");
    store
        .mutate(&run.id, |run| {
            run.record_cleanup(
                attempt,
                crate::workflows::run::AttemptCleanupRecord::Complete,
            )
        })
        .expect("cleanup");

    let reopened = WorkflowRunStore::open(dir.path().to_path_buf()).expect("reopen");
    assert!(reopened.get(&run.id).expect("run").is_active());
    reopened.interrupt_active().expect("interrupt");
    let recovered = reopened.get(&run.id).expect("run");
    assert_eq!(recovered.state, RunState::Interrupted);
    assert_eq!(
        recovered.attempts[0].cleanup,
        crate::workflows::run::AttemptCleanupRecord::Complete
    );
}

#[test]
fn malformed_json_fails_startup() {
    let dir = tempfile::tempdir().expect("dir");
    fs::write(
        dir.path().join(format!("{}.json", "a".repeat(32))),
        b"{not json",
    )
    .expect("write");
    assert_eq!(
        WorkflowRunStore::open(dir.path().to_path_buf()).err(),
        Some(StoreError::Corrupt)
    );
}

#[test]
fn filename_disagreement_fails_startup() {
    let dir = tempfile::tempdir().expect("dir");
    let store = WorkflowRunStore::open(dir.path().to_path_buf()).expect("open");
    let run = store.create(run_named("Named", 1)).expect("create");
    let original = dir.path().join(format!("{}.json", run.id.as_hex()));
    let renamed = dir.path().join(format!("{}.json", "b".repeat(32)));
    fs::rename(original, renamed).expect("rename");
    assert_eq!(
        WorkflowRunStore::open(dir.path().to_path_buf()).err(),
        Some(StoreError::Corrupt)
    );
}

#[test]
fn digest_disagreement_fails_startup() {
    let dir = tempfile::tempdir().expect("dir");
    let store = WorkflowRunStore::open(dir.path().to_path_buf()).expect("open");
    let run = store.create(run_named("Named", 1)).expect("create");
    let path = dir.path().join(format!("{}.json", run.id.as_hex()));
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
    value["version"] = serde_json::Value::String("0".repeat(64));
    fs::write(&path, serde_json::to_vec(&value).expect("bytes")).expect("write");
    assert_eq!(
        WorkflowRunStore::open(dir.path().to_path_buf()).err(),
        Some(StoreError::Corrupt)
    );
}

#[test]
fn invalid_states_fail_startup() {
    let dir = tempfile::tempdir().expect("dir");
    let store = WorkflowRunStore::open(dir.path().to_path_buf()).expect("open");
    let run = store.create(run_named("Named", 1)).expect("create");
    let path = dir.path().join(format!("{}.json", run.id.as_hex()));
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
    value["state"] = serde_json::json!({"type": "completed"});
    fs::write(&path, serde_json::to_vec(&value).expect("bytes")).expect("write");
    assert_eq!(
        WorkflowRunStore::open(dir.path().to_path_buf()).err(),
        Some(StoreError::Corrupt)
    );
}

#[test]
fn non_chronological_attempts_fail_startup() {
    let dir = tempfile::tempdir().expect("dir");
    let store = WorkflowRunStore::open(dir.path().to_path_buf()).expect("open");
    let run = store.create(run_named("Named", 1)).expect("create");
    let attempt = AttemptId::generate().expect("attempt");
    store
        .mutate(&run.id, |run| start_test_attempt(run, attempt, 2))
        .expect("start");
    let path = dir.path().join(format!("{}.json", run.id.as_hex()));
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
    value["attempts"][0]["started-at-ms"] = serde_json::json!(0);
    fs::write(&path, serde_json::to_vec(&value).expect("bytes")).expect("write");
    assert_eq!(
        WorkflowRunStore::open(dir.path().to_path_buf()).err(),
        Some(StoreError::Corrupt)
    );
}

#[test]
fn a_terminal_attempt_without_a_result_fails_startup() {
    let dir = tempfile::tempdir().expect("dir");
    let store = WorkflowRunStore::open(dir.path().to_path_buf()).expect("open");
    let run = store.create(run_named("Named", 1)).expect("create");
    let attempt = AttemptId::generate().expect("attempt");
    store
        .mutate(&run.id, |run| start_test_attempt(run, attempt, 2))
        .expect("start");
    store
        .mutate(&run.id, |run| {
            run.record_cleanup(
                attempt,
                crate::workflows::run::AttemptCleanupRecord::Complete,
            )?;
            run.complete_attempt(attempt, 3)
        })
        .expect("complete");
    let path = dir.path().join(format!("{}.json", run.id.as_hex()));
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
    value["attempts"][0]["result"] = serde_json::Value::Null;
    fs::write(&path, serde_json::to_vec(&value).expect("bytes")).expect("write");
    assert_eq!(
        WorkflowRunStore::open(dir.path().to_path_buf()).err(),
        Some(StoreError::Corrupt)
    );
}

#[test]
fn a_completed_run_marked_failed_fails_startup() {
    let dir = tempfile::tempdir().expect("dir");
    let store = WorkflowRunStore::open(dir.path().to_path_buf()).expect("open");
    let run = store.create(run_named("Named", 1)).expect("create");
    let attempt = AttemptId::generate().expect("attempt");
    store
        .mutate(&run.id, |run| start_test_attempt(run, attempt, 2))
        .expect("start");
    store
        .mutate(&run.id, |run| {
            run.record_cleanup(
                attempt,
                crate::workflows::run::AttemptCleanupRecord::Complete,
            )?;
            run.complete_attempt(attempt, 3)
        })
        .expect("complete");
    let path = dir.path().join(format!("{}.json", run.id.as_hex()));
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
    value["state"] = serde_json::json!({"type": "failed"});
    fs::write(&path, serde_json::to_vec(&value).expect("bytes")).expect("write");
    assert_eq!(
        WorkflowRunStore::open(dir.path().to_path_buf()).err(),
        Some(StoreError::Corrupt)
    );
}

#[test]
fn a_completed_attempt_without_complete_cleanup_fails_startup() {
    let dir = tempfile::tempdir().expect("dir");
    let store = WorkflowRunStore::open(dir.path().to_path_buf()).expect("open");
    let run = store.create(run_named("Named", 1)).expect("create");
    let attempt = AttemptId::generate().expect("attempt");
    store
        .mutate(&run.id, |run| start_test_attempt(run, attempt, 2))
        .expect("start");
    store
        .mutate(&run.id, |run| {
            run.record_cleanup(
                attempt,
                crate::workflows::run::AttemptCleanupRecord::Complete,
            )?;
            run.complete_attempt(attempt, 3)
        })
        .expect("complete");
    let path = dir.path().join(format!("{}.json", run.id.as_hex()));
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
    value["attempts"][0]["cleanup"] = serde_json::json!({"state": "pending"});
    fs::write(&path, serde_json::to_vec(&value).expect("bytes")).expect("write");
    assert_eq!(
        WorkflowRunStore::open(dir.path().to_path_buf()).err(),
        Some(StoreError::Corrupt)
    );
}

#[test]
fn a_host_path_in_capabilities_fails_startup() {
    let dir = tempfile::tempdir().expect("dir");
    let store = WorkflowRunStore::open(dir.path().to_path_buf()).expect("open");
    let run = store.create(run_named("Named", 1)).expect("create");
    let attempt = AttemptId::generate().expect("attempt");
    store
        .mutate(&run.id, |run| start_test_attempt(run, attempt, 2))
        .expect("start");
    let path = dir.path().join(format!("{}.json", run.id.as_hex()));
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
    value["attempts"][0]["capabilities"]["directories"] = serde_json::json!([{
        "alias": "project", "guest-path": "/home/user/project", "access": "read-only", "role": "primary-source"
    }]);
    fs::write(&path, serde_json::to_vec(&value).expect("bytes")).expect("write");
    assert_eq!(
        WorkflowRunStore::open(dir.path().to_path_buf()).err(),
        Some(StoreError::Corrupt)
    );
}

#[test]
fn owner_only_permissions_cover_the_run_directory() {
    let dir = tempfile::tempdir().expect("dir");
    let runs = dir.path().join("workflow-runs");
    let store = WorkflowRunStore::open(runs.clone()).expect("open");
    store.create(run_named("Named", 1)).expect("create");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&runs).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }
}

#[test]
fn an_unsupported_run_schema_fails_startup() {
    let dir = tempfile::tempdir().expect("dir");
    let store = WorkflowRunStore::open(dir.path().to_path_buf()).expect("open");
    let run = store.create(run_named("Named", 1)).expect("create");
    let path = dir.path().join(format!("{}.json", run.id.as_hex()));
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
    value["record-version"] = serde_json::json!(2);
    fs::write(&path, serde_json::to_vec(&value).expect("bytes")).expect("write");
    assert_eq!(
        WorkflowRunStore::open(dir.path().to_path_buf()).err(),
        Some(StoreError::Corrupt)
    );
}

#[test]
fn a_capability_record_without_agent_revision_fails_startup() {
    let dir = tempfile::tempdir().expect("dir");
    let store = WorkflowRunStore::open(dir.path().to_path_buf()).expect("open");
    let run = store.create(run_named("Named", 1)).expect("create");
    let attempt = AttemptId::generate().expect("attempt");
    store
        .mutate(&run.id, |run| start_test_attempt(run, attempt, 2))
        .expect("start");
    let path = dir.path().join(format!("{}.json", run.id.as_hex()));
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
    value["attempts"][0]["capabilities"]
        .as_object_mut()
        .expect("capabilities")
        .remove("agent-revision");
    fs::write(&path, serde_json::to_vec(&value).expect("bytes")).expect("write");
    assert_eq!(
        WorkflowRunStore::open(dir.path().to_path_buf()).err(),
        Some(StoreError::Corrupt)
    );
}
