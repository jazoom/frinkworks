use super::*;
use crate::agents::AgentId;
use crate::conversations::{ConversationId, DocumentId};
use crate::projects::ProjectId;
use crate::tests::test_environment_id;
use crate::workflows::definition::PinnedWorkflowDefinition;
use crate::workflows::run::{RunKind, RunSource};
use crate::workflows::seeds::task_loop_definition;
use crate::workflows::{ArtefactId, RunId};

impl TaskLoopStore {
    pub(crate) fn in_memory() -> Self {
        Self {
            dir: None,
            inner: Mutex::new(BTreeMap::new()),
        }
    }
}

fn snapshot() -> TaskListSnapshot {
    let markdown = "# Tasks\n\n- [ ] First task\n- [ ] Second task\n".to_owned();
    TaskListSnapshot {
        document_id: DocumentId::generate().expect("document"),
        revision: 1,
        content_hash: crate::workflows::artefacts::ObjectHash::of(markdown.as_bytes()).as_str(),
        markdown,
    }
}

pub(crate) fn loop_record() -> TaskLoop {
    let definition = task_loop_definition(test_environment_id());
    let list = snapshot();
    let parsed = crate::workflows::task_list::parse(&list.markdown).expect("tasks");
    TaskLoop::create(
        TaskLoopId::generate().expect("loop"),
        1,
        ConversationId::generate().expect("conversation"),
        Some(ProjectId::parse(&"b".repeat(32)).expect("project")),
        Some(AgentId::generate().expect("agent")),
        "Implement each remaining task.".to_owned(),
        PinnedWorkflowDefinition::pin(None, definition),
        Vec::new(),
        crate::tests::test_environment_set(&task_loop_definition(test_environment_id())),
        list,
        parsed
            .eligible_tasks()
            .map(|task| TaskLoopItem {
                index: task.index,
                markdown: task.markdown.clone(),
                child_id: None,
                previous_child_ids: Vec::new(),
                outcome: TaskOutcome::Pending,
            })
            .collect(),
    )
    .expect("loop")
}

pub(crate) fn completed_child(
    parent: &TaskLoop,
    child_id: RunId,
    source: ArtefactReference,
) -> WorkflowRun {
    let task = parent
        .tasks
        .iter()
        .find(|task| task.child_id == Some(child_id))
        .expect("task");
    let mut run = parent
        .child_run(child_id, 2, task.index, task.markdown.clone())
        .expect("child");
    run.source = RunSource::Captured {
        source: crate::workflows::run::RunSourceState {
            initial: source.clone(),
            accepted: source.clone(),
            observed: crate::workflows::run::ObservedCandidate::Exact { artefact: source },
        },
    };
    run.state = RunState::Completed;
    run.attempts.push(crate::workflows::run::AttemptRecord {
        id: crate::workflows::AttemptId::generate().expect("attempt"),
        step: crate::workflows::definition::StepKey::parse("commit").expect("commit step"),
        ordinal: 1,
        action_kind: crate::workflows::run::ActionKind::SystemCommand,
        started_at_ms: 2,
        finished_at_ms: Some(3),
        state: crate::workflows::run::AttemptState::Completed,
        result: Some(crate::workflows::run::AttemptResult::Completed {
            outputs: Vec::new(),
        }),
        review_route: None,
        inputs: Vec::new(),
        outputs: Vec::new(),
        capabilities: crate::tests::test_agent_capabilities(),
        sandbox: crate::workflows::run::AttemptSandboxRecord {
            kind: crate::workflows::run::AttemptSandboxKind::IsolatedAttempt,
            snapshot_digest: parent.environments.steps[0].snapshot_digest.clone(),
        },
        initial_context: None,
        cleanup: crate::workflows::run::AttemptCleanupRecord::Complete,
        apply_transaction: None,
        commit_transaction: None,
        commit_result: Some(super::super::commit::CommitResult {
            commit: "a".repeat(40),
        }),
    });
    run
}

pub(crate) fn source(label: u8) -> ArtefactReference {
    ArtefactReference {
        id: ArtefactId::generate().expect("artefact"),
        kind: crate::workflows::definition::ArtefactKind::CandidateRevision,
        artefact_hash: crate::workflows::artefacts::ArtefactHash::parse(&format!(
            "sha256:{}",
            crate::hex::encode(&[label; 32])
        ))
        .expect("hash"),
    }
}

#[test]
fn parent_child_identities_stay_distinct() {
    let store = TaskLoopStore::in_memory();
    let record = store.create(loop_record()).expect("create");
    let (parent, first_id, first_task) = store.reserve_next_child(&record.id, 0).expect("reserve");
    assert_eq!(first_task.index, 0);
    assert_eq!(parent.current_child(), Some(first_id));
    let child = parent
        .child_run(first_id, 2, first_task.index, first_task.markdown)
        .expect("child");
    assert_eq!(child.parent_loop, Some(parent.id));
    assert_eq!(child.conversation_id, Some(parent.conversation_id));
    assert_eq!(child.kind, RunKind::Configured);
    assert_eq!(
        child.task_selection.as_ref().map(|task| task.index),
        Some(0)
    );
    assert_ne!(child.id.as_hex(), parent.id.as_hex());
    store
        .mark_dispatched(&parent.id, first_id)
        .expect("dispatch");
    let runs = crate::workflows::WorkflowRunStore::in_memory();
    runs.create(child).expect("store child");
    assert!(runs.summaries().is_empty());
    assert!(runs.for_conversation(&parent.conversation_id).is_empty());
}

#[test]
fn parent_source_stays_distinct_from_later_task_bases() {
    let store = TaskLoopStore::in_memory();
    let record = store.create(loop_record()).expect("create");
    let (parent, first_id, _) = store.reserve_next_child(&record.id, 0).expect("first");
    store
        .mark_dispatched(&parent.id, first_id)
        .expect("dispatch");
    let original = source(1);
    store
        .record_original_source(&parent.id, original.clone())
        .expect("original");
    let first = completed_child(
        &store.get(&parent.id).expect("loop"),
        first_id,
        original.clone(),
    );
    let (parent, advance) = store.complete_child(&parent.id, &first).expect("complete");
    assert_eq!(advance, LoopAdvance::Next);
    assert_eq!(parent.original_source.as_ref(), Some(&original));
    let (parent, second_id, _) = store.reserve_next_child(&parent.id, 1).expect("second");
    let second = parent
        .child_run(second_id, 4, 1, parent.tasks[1].markdown.clone())
        .expect("second child");
    assert_ne!(second.id, first_id);
    assert_eq!(parent.original_source.as_ref(), Some(&original));
    assert!(matches!(second.source, RunSource::Pending));
    assert!(second.attempts.is_empty());
    let parent = store
        .record_original_source(&parent.id, source(2))
        .expect("later source capture");
    assert_eq!(parent.original_source, Some(original));
}

#[test]
fn duplicate_dispatch_is_rejected_without_a_second_child() {
    let store = TaskLoopStore::in_memory();
    let record = store.create(loop_record()).expect("create");
    let (parent, first_id, _) = store.reserve_next_child(&record.id, 0).expect("first");
    store
        .mark_dispatched(&parent.id, first_id)
        .expect("dispatch");
    assert_eq!(
        store.mark_dispatched(&parent.id, first_id).err(),
        Some(TaskLoopError::DuplicateDispatch)
    );
    assert_eq!(
        store.reserve_next_child(&parent.id, 0).err(),
        Some(TaskLoopError::DuplicateDispatch)
    );
    assert_eq!(store.get(&parent.id).expect("loop").tasks[1].child_id, None);
    let first = completed_child(&store.get(&parent.id).expect("loop"), first_id, source(1));
    store.complete_child(&parent.id, &first).expect("complete");
    assert_eq!(
        store.complete_child(&parent.id, &first).err(),
        Some(TaskLoopError::DuplicateDispatch)
    );
}

#[test]
fn a_bound_blocks_dispatch_durably() {
    let dir = tempfile::tempdir().expect("directory");
    let store = TaskLoopStore::open(dir.path().join("loops")).expect("store");
    let parent = store.create(loop_record()).expect("create");
    assert_eq!(
        store
            .reserve_next_child(&parent.id, MAXIMUM_LOOP_ATTEMPTS)
            .err(),
        Some(TaskLoopError::AttemptLimit)
    );
    let reopened = TaskLoopStore::open(dir.path().join("loops")).expect("reopen");
    let parent = reopened.get(&parent.id).expect("parent");
    assert_eq!(parent.state, TaskLoopState::Blocked);
    assert!(parent.tasks.iter().all(|task| task.child_id.is_none()));
}

#[test]
fn checked_tasks_keep_the_remaining_tasks_original_indices() {
    let mut parent = loop_record();
    parent.task_list.markdown = parent.task_list.markdown.replacen("[ ]", "[x]", 1);
    parent.task_list.content_hash =
        crate::workflows::artefacts::ObjectHash::of(parent.task_list.markdown.as_bytes()).as_str();
    parent.tasks.remove(0);
    let store = TaskLoopStore::in_memory();
    let parent = store
        .create(parent)
        .expect("checked tasks are context only");
    let (parent, child, task) = store.reserve_next_child(&parent.id, 0).expect("reserve");
    assert_eq!(task.index, 1);
    assert!(
        parent
            .child_run(child, 2, 0, task.markdown.clone())
            .is_err()
    );
    assert!(
        parent
            .child_run(child, 2, task.index, task.markdown)
            .is_ok()
    );
    let mut file = parent.to_file().expect("file");
    file.task_list.markdown.push_str("altered context");
    assert_eq!(
        TaskLoop::from_file(file).err(),
        Some(TaskLoopError::Corrupt)
    );
}

#[test]
fn a_substituted_child_cannot_advance_the_parent() {
    let store = TaskLoopStore::in_memory();
    let parent = store.create(loop_record()).expect("create");
    let (parent, id, _) = store.reserve_next_child(&parent.id, 0).expect("reserve");
    store.mark_dispatched(&parent.id, id).expect("dispatch");
    let mut child = completed_child(&parent, id, source(1));
    child.attempts[0].commit_result = None;
    assert_eq!(
        store.complete_child(&parent.id, &child).err(),
        Some(TaskLoopError::Conflict)
    );
    child = completed_child(&parent, id, source(1));
    child.task_selection.as_mut().expect("selection").index = 1;
    assert_eq!(
        store.complete_child(&parent.id, &child).err(),
        Some(TaskLoopError::Conflict)
    );
    child = completed_child(&parent, id, source(1));
    child.environments.steps[0].snapshot_digest =
        crate::environments::SnapshotDigest::parse(&format!("sha256:{}", "e".repeat(64)))
            .expect("replacement snapshot");
    assert_eq!(
        store.complete_child(&parent.id, &child).err(),
        Some(TaskLoopError::Conflict)
    );
    child = completed_child(&parent, id, source(1));
    child.agent_id = Some(AgentId::generate().expect("other agent"));
    assert_eq!(
        store.complete_child(&parent.id, &child).err(),
        Some(TaskLoopError::Conflict)
    );
    assert_eq!(store.get(&parent.id).expect("parent").completed_count(), 0);
}

#[test]
fn pause_after_a_completed_task_does_not_dispatch_the_next_child() {
    let store = TaskLoopStore::in_memory();
    let parent = store.create(loop_record()).expect("create");
    let (parent, first_id, _) = store.reserve_next_child(&parent.id, 0).expect("first");
    store
        .mark_dispatched(&parent.id, first_id)
        .expect("dispatch");
    let token = parent.command_token();
    let parent = store.request_pause(&parent.id, &token).expect("pause");
    assert!(matches!(
        parent.state,
        TaskLoopState::PauseRequested { child, .. } if child == first_id
    ));
    assert_eq!(
        store.request_pause(&parent.id, &token).err(),
        Some(TaskLoopError::Stale)
    );
    let first = completed_child(&parent, first_id, source(1));
    let (parent, advance) = store.complete_child(&parent.id, &first).expect("complete");
    assert_eq!(advance, LoopAdvance::Pause);
    assert_eq!(parent.state, TaskLoopState::Paused);
    assert_eq!(parent.tasks[0].outcome, TaskOutcome::CompletedCommit);
    assert_eq!(parent.tasks[1].child_id, None);
    assert_eq!(parent.tasks[1].outcome, TaskOutcome::Pending);
    let stale = parent.command_token().replace("paused", "active");
    assert_eq!(
        store.stop_if_token(&parent.id, &stale).err(),
        Some(TaskLoopError::Stale)
    );
    assert_eq!(
        store.get(&parent.id).expect("paused").state,
        TaskLoopState::Paused
    );
    let (parent, second_id, second) = store
        .reserve_next_child(&parent.id, 1)
        .expect("continue next");
    assert_eq!(second.index, 1);
    assert_ne!(second_id, first_id);
    assert_eq!(parent.tasks[0].outcome, TaskOutcome::CompletedCommit);
    assert_eq!(parent.current_child(), Some(second_id));
}

#[test]
fn pause_at_an_awaiting_child_is_not_approval() {
    let store = TaskLoopStore::in_memory();
    let parent = store.create(loop_record()).expect("create");
    let (parent, first_id, _) = store.reserve_next_child(&parent.id, 0).expect("first");
    store
        .mark_dispatched(&parent.id, first_id)
        .expect("dispatch");
    store.mark_awaiting(&parent.id, first_id).expect("await");
    let parent = store.get(&parent.id).expect("awaiting");
    let parent = store
        .request_pause(&parent.id, &parent.command_token())
        .expect("pause");
    assert!(matches!(
        parent.state,
        TaskLoopState::PauseRequested { child, .. } if child == first_id
    ));
    assert_eq!(parent.tasks[0].outcome, TaskOutcome::Dispatched);
    assert_eq!(parent.completed_count(), 0);
    store
        .mark_awaiting(&parent.id, first_id)
        .expect("keep pause");
    assert!(matches!(
        store.get(&parent.id).expect("still pause").state,
        TaskLoopState::PauseRequested { .. }
    ));
}

#[test]
fn a_stale_stop_cannot_cancel_the_current_worker() {
    let store = TaskLoopStore::in_memory();
    let parent = store.create(loop_record()).expect("parent");
    let (parent, child, _) = store.reserve_next_child(&parent.id, 0).expect("child");
    let job = crate::sessions::Job::new(crate::sessions::JobId::generate().expect("job"), child, 0);
    let token = parent.command_token();
    let paused = store.request_pause(&parent.id, &token).expect("pause");
    assert_eq!(
        store.request_stop(&parent.id, &token, &job),
        Err(TaskLoopError::Stale)
    );
    assert!(!job.cancel_requested());
    store
        .request_stop(&parent.id, &paused.command_token(), &job)
        .expect("stop");
    assert!(job.cancel_requested());
    assert_eq!(store.get(&parent.id).expect("parent"), paused);
}

#[test]
fn stop_preserves_completed_tasks() {
    let store = TaskLoopStore::in_memory();
    let parent = store.create(loop_record()).expect("create");
    let (parent, first_id, _) = store.reserve_next_child(&parent.id, 0).expect("first");
    store
        .mark_dispatched(&parent.id, first_id)
        .expect("dispatch");
    let first = completed_child(&parent, first_id, source(1));
    let (parent, _) = store.complete_child(&parent.id, &first).expect("complete");
    let (parent, second_id, _) = store.reserve_next_child(&parent.id, 1).expect("second");
    store
        .mark_dispatched(&parent.id, second_id)
        .expect("dispatch second");
    let parent = store
        .stop_if_token(&parent.id, &parent.command_token())
        .expect("stop");
    assert_eq!(parent.state, TaskLoopState::Stopped);
    assert_eq!(parent.tasks[0].outcome, TaskOutcome::CompletedCommit);
    assert_eq!(parent.tasks[1].outcome, TaskOutcome::Cancelled);
}

fn interrupted_child(parent: &TaskLoop, child_id: RunId) -> WorkflowRun {
    let task = parent
        .tasks
        .iter()
        .find(|task| task.child_id == Some(child_id))
        .expect("task");
    let mut run = parent
        .child_run(child_id, 2, task.index, task.markdown.clone())
        .expect("child");
    run.state = RunState::Interrupted;
    run
}

#[test]
fn recovery_uses_child_evidence_not_the_parent_counter() {
    let store = TaskLoopStore::in_memory();
    let parent = store.create(loop_record()).expect("create");
    let (parent, first_id, _) = store.reserve_next_child(&parent.id, 0).expect("reserve");
    store
        .mark_dispatched(&parent.id, first_id)
        .expect("dispatch");
    let dispatched = store.get(&parent.id).expect("dispatched");
    let child = interrupted_child(&dispatched, first_id);
    store
        .mutate(&parent.id, |record| {
            record.tasks[0].outcome = TaskOutcome::CompletedCommit;
            record.state = TaskLoopState::Paused;
            Ok(())
        })
        .expect("counter only");
    let runs = crate::workflows::WorkflowRunStore::in_memory();
    runs.create(child).expect("store child");
    store.reconcile(&runs).expect("reconcile");
    let parent = store.get(&parent.id).expect("parent");
    assert_eq!(parent.state, TaskLoopState::Blocked);
    store.reconcile(&runs).expect("second recovery");
    let parent = store.get(&parent.id).expect("parent");
    assert_eq!(parent.state, TaskLoopState::Blocked);
    assert!(!parent.allows_retry());
}

#[test]
fn a_reserved_child_without_a_created_run_is_not_complete() {
    let store = TaskLoopStore::in_memory();
    let parent = store.create(loop_record()).expect("create");
    let (parent, first_id, _) = store.reserve_next_child(&parent.id, 0).expect("reserve");
    let runs = crate::workflows::WorkflowRunStore::in_memory();
    store.reconcile(&runs).expect("reconcile");
    let parent = store.get(&parent.id).expect("parent");
    assert_eq!(parent.state, TaskLoopState::Interrupted);
    assert_eq!(parent.tasks[0].child_id, Some(first_id));
    assert_eq!(parent.tasks[0].outcome, TaskOutcome::Reserved);
    assert_eq!(parent.tasks[1].child_id, None);
    assert!(parent.allows_retry());
    assert!(!parent.allows_continue());
}

#[test]
fn recovered_completion_pauses_before_the_next_task() {
    let store = TaskLoopStore::in_memory();
    let parent = store.create(loop_record()).expect("create");
    let (parent, first_id, _) = store.reserve_next_child(&parent.id, 0).expect("reserve");
    store
        .mark_dispatched(&parent.id, first_id)
        .expect("dispatch");
    let runs = crate::workflows::WorkflowRunStore::in_memory();
    let child = completed_child(&parent, first_id, source(1));
    runs.create(child).expect("store child");
    store.reconcile(&runs).expect("reconcile");
    let parent = store.get(&parent.id).expect("parent");
    assert_eq!(parent.state, TaskLoopState::Paused);
    assert_eq!(parent.tasks[0].outcome, TaskOutcome::CompletedCommit);
    assert_eq!(parent.tasks[1].child_id, None);
    assert_eq!(parent.tasks[1].outcome, TaskOutcome::Pending);
    assert!(parent.allows_continue());
    let (next, next_id, task) = store.reserve_next_child(&parent.id, 1).expect("next");
    assert_ne!(next_id, first_id);
    assert_eq!(task.index, parent.tasks[1].index);
    assert_eq!(next.tasks[0].child_id, Some(first_id));
}

#[test]
fn retry_keeps_the_previous_child_for_inspection() {
    let store = TaskLoopStore::in_memory();
    let parent = store.create(loop_record()).expect("create");
    let (parent, first_id, _) = store.reserve_next_child(&parent.id, 0).expect("reserve");
    store
        .mark_dispatched(&parent.id, first_id)
        .expect("dispatch");
    let runs = crate::workflows::WorkflowRunStore::in_memory();
    runs.create(interrupted_child(&parent, first_id))
        .expect("store child");
    store.reconcile(&runs).expect("reconcile");
    let parent = store.get(&parent.id).expect("failed");
    assert_eq!(parent.state, TaskLoopState::Failed);
    let (parent, retry_id, task) = store.retry_current(&parent.id, 1, false).expect("retry");
    assert_ne!(retry_id, first_id);
    assert_eq!(task.previous_child_ids, vec![first_id]);
    assert_eq!(parent.tasks[0].child_id, Some(retry_id));
    assert_eq!(parent.tasks[0].outcome, TaskOutcome::Reserved);
    assert!(matches!(parent.state, TaskLoopState::Active { child, .. } if child == retry_id));
    let retry = parent
        .child_run(retry_id, 4, task.index, task.markdown)
        .expect("fresh child");
    assert!(retry.attempts.is_empty());
    assert!(matches!(retry.source, RunSource::Pending));
}

#[test]
fn an_uncertain_commit_blocks_a_replacement_child() {
    let store = TaskLoopStore::in_memory();
    let parent = store.create(loop_record()).expect("create");
    let (parent, first_id, _) = store.reserve_next_child(&parent.id, 0).expect("reserve");
    store
        .mark_dispatched(&parent.id, first_id)
        .expect("dispatch");
    let mut child = completed_child(&parent, first_id, source(1));
    child.state = RunState::Active {
        step: crate::workflows::definition::StepKey::parse("commit").expect("commit"),
        attempt: child.attempts[0].id,
    };
    child.attempts[0].state = crate::workflows::run::AttemptState::Active;
    child.attempts[0].finished_at_ms = None;
    child.attempts[0].result = None;
    child.attempts[0].commit_result = None;
    child.attempts[0].commit_transaction = Some(crate::workflows::commit::CommitTransaction {
        state: crate::workflows::commit::CommitTransactionState::WorktreeApplied,
        candidate: source(2),
        reviews: Vec::new(),
        approval: None,
        expected_reference: "refs/heads/main".to_owned(),
        old_object: Some("a".repeat(40)),
        target_tree: Some("b".repeat(40)),
        expected_commit: Some("c".repeat(40)),
        timestamp: "1 +0000".to_owned(),
    });
    let runs = crate::workflows::WorkflowRunStore::in_memory();
    runs.create(child).expect("store child");
    store.reconcile(&runs).expect("reconcile");
    let parent = store.get(&parent.id).expect("parent");
    assert_eq!(parent.state, TaskLoopState::Blocked);
    assert_eq!(parent.tasks[0].child_id, Some(first_id));
    assert_eq!(parent.tasks[1].child_id, None);
    assert_eq!(
        store.retry_current(&parent.id, 1, false).err(),
        Some(TaskLoopError::Conflict)
    );
    assert_eq!(
        store.reserve_next_child(&parent.id, 1).err(),
        Some(TaskLoopError::DuplicateDispatch)
    );
}

#[test]
fn recovery_rejects_a_child_from_another_parent() {
    let store = TaskLoopStore::in_memory();
    let parent = store.create(loop_record()).expect("parent");
    let (parent, child_id, _) = store.reserve_next_child(&parent.id, 0).expect("reserve");
    let mut child = interrupted_child(&parent, child_id);
    child.parent_loop = Some(TaskLoopId::generate().expect("other parent"));
    let runs = crate::workflows::WorkflowRunStore::in_memory();
    runs.create(child).expect("child");
    assert_eq!(store.reconcile(&runs), Err(TaskLoopError::Corrupt));
    assert_eq!(store.get(&parent.id).expect("unchanged"), parent);
}

fn applied_child() -> (TaskLoopStore, TaskLoop, WorkflowRun) {
    use crate::workflows::apply::{
        ApplyRoot, ApplyRootOutcome, ApplyTransaction, ApplyTransactionState,
    };
    let definition =
        crate::workflows::seeds::implement_and_review_definition(test_environment_id());
    let definition = crate::workflows::definition::WorkflowDefinition::from_parts_with_mode(
        definition.name().to_owned(),
        definition.default_environment(),
        definition.roles().to_vec(),
        definition.steps().to_vec(),
        crate::workflows::definition::ExecutionMode::TaskList,
    )
    .expect("task loop definition");
    let mut parent = loop_record();
    parent.environments = crate::tests::test_environment_set(&definition);
    parent.pinned = PinnedWorkflowDefinition::pin(None, definition);
    let store = TaskLoopStore::in_memory();
    let parent = store.create(parent).expect("parent");
    let (parent, child_id, _) = store.reserve_next_child(&parent.id, 0).expect("reserve");
    let parent = store
        .mark_dispatched(&parent.id, child_id)
        .expect("dispatch");
    let mut child = completed_child(&parent, child_id, source(1));
    let attempt = &mut child.attempts[0];
    attempt.step = crate::workflows::definition::StepKey::parse("apply").expect("step");
    attempt.commit_result = None;
    attempt.apply_transaction = Some(ApplyTransaction {
        state: ApplyTransactionState::Verified,
        roots: vec![ApplyRoot {
            grant_id: crate::execution::DirectoryGrantId::generate().expect("grant"),
            alias: "files".to_owned(),
            host_path: PathBuf::from("/files"),
            identity: crate::execution::CanonicalDirectoryIdentity {
                device: 1,
                inode: 2,
            },
            baseline_candidate: crate::workflows::artefacts::CandidateHash::parse(&format!(
                "sha256:{}",
                "a".repeat(64)
            ))
            .expect("hash"),
            candidate_hash: crate::workflows::artefacts::CandidateHash::parse(&format!(
                "sha256:{}",
                "b".repeat(64)
            ))
            .expect("hash"),
            exclusions: Vec::new(),
            outcome: ApplyRootOutcome::Applied,
        }],
        baseline: source(1),
        candidate: source(2),
        approval: source(3),
    });
    (store, parent, child)
}

#[test]
fn generic_application_requires_all_roots_and_cleanup_to_settle() {
    use crate::workflows::apply::ApplyRootOutcome;
    let (store, parent, mut child) = applied_child();
    for outcome in [
        ApplyRootOutcome::Pending,
        ApplyRootOutcome::Conflicted,
        ApplyRootOutcome::Uncertain,
    ] {
        child.attempts[0]
            .apply_transaction
            .as_mut()
            .expect("transaction")
            .roots[0]
            .outcome = outcome;
        assert_eq!(
            store.complete_child(&parent.id, &child).err(),
            Some(TaskLoopError::Conflict)
        );
        assert_eq!(store.get(&parent.id).expect("parent"), parent);
    }
    child.attempts[0]
        .apply_transaction
        .as_mut()
        .expect("transaction")
        .roots[0]
        .outcome = ApplyRootOutcome::Applied;
    let mut earlier = child.attempts[0].clone();
    earlier.apply_transaction = None;
    earlier.cleanup = crate::workflows::run::AttemptCleanupRecord::Orphaned {
        sandbox: true,
        workspace: false,
        journal: false,
    };
    child.attempts.insert(0, earlier);
    assert_eq!(
        store.complete_child(&parent.id, &child).err(),
        Some(TaskLoopError::Conflict)
    );
    child.attempts.remove(0);
    let (completed, advance) = store
        .complete_child(&parent.id, &child)
        .expect("settled application");
    assert_eq!(advance, LoopAdvance::Next);
    assert_eq!(
        completed.tasks[0].outcome,
        TaskOutcome::CompletedApplication
    );
    assert_eq!(
        store.complete_child(&parent.id, &child).err(),
        Some(TaskLoopError::DuplicateDispatch)
    );
    assert!(completed.tasks[1].child_id.is_none());
}

#[test]
fn direct_completion_requires_successful_outputs_and_managed_cleanup() {
    let (_, _, mut child) = applied_child();
    let root = tempfile::tempdir().unwrap();
    let mut grant = crate::execution::DirectoryGrant::from_selected(root.path(), &[]).unwrap();
    grant.access = crate::execution::DirectoryAccess::DirectWrite;
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(
            crate::providers::ProviderKind::Xai,
            "grok-4.6".to_owned(),
            None,
        )
        .unwrap(),
        String::new(),
        crate::agents::ToolId::ALL.to_vec(),
        test_environment_id(),
    )
    .unwrap()
    .with_directories(vec![grant])
    .unwrap();
    let step = child.pinned.definition.first_step().clone();
    child.phase_models = vec![crate::workflows::run::PhaseModelSelection {
        step: step.clone(),
        selection: settings.model.clone(),
        instructions: String::new(),
        preset: None,
        settings: Some(settings),
    }];
    child.attempts[0].step = step;
    child.attempts[0].action_kind = crate::workflows::run::ActionKind::Agent;
    child.attempts[0].apply_transaction = None;
    assert_eq!(child_outcome(&child), Some(TaskOutcome::CompletedDirect));
    child.attempts[0].result = None;
    assert_ne!(child_outcome(&child), Some(TaskOutcome::CompletedDirect));
    child.attempts[0].result = Some(crate::workflows::run::AttemptResult::Completed {
        outputs: Vec::new(),
    });
    child.attempts[0].cleanup = crate::workflows::run::AttemptCleanupRecord::Orphaned {
        sandbox: true,
        workspace: false,
        journal: false,
    };
    assert_eq!(child_outcome(&child), None);
}

#[test]
fn uncertain_application_never_counts_as_completion() {
    use crate::workflows::apply::ApplyTransactionState;
    let (store, parent, mut child) = applied_child();
    for state in [
        ApplyTransactionState::Prepared,
        ApplyTransactionState::Applying {
            completed: 0,
            path: "file".to_owned(),
        },
        ApplyTransactionState::Applied { completed: 1 },
        ApplyTransactionState::RecoveryUncertain,
    ] {
        child.attempts[0]
            .apply_transaction
            .as_mut()
            .expect("transaction")
            .state = state;
        assert!(child_settlement_uncertain(&child));
        assert_eq!(
            store.complete_child(&parent.id, &child).err(),
            Some(TaskLoopError::Conflict)
        );
        assert_eq!(store.get(&parent.id).expect("parent"), parent);
    }
}

#[test]
fn recovery_does_not_reopen_a_deliberate_between_task_stop() {
    let store = TaskLoopStore::in_memory();
    let parent = store.create(loop_record()).expect("parent");
    let stopped = store.stop(&parent.id).expect("stop before dispatch");
    let runs = crate::workflows::WorkflowRunStore::in_memory();
    store.reconcile(&runs).expect("recovery");
    assert_eq!(store.get(&parent.id).expect("parent"), stopped);
}
