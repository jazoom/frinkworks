use super::*;
use std::sync::Arc;

fn fixture() -> (
    crate::state::AppState,
    tempfile::TempDir,
    ConversationRecord,
    ConversationRecord,
    WorkflowRun,
    tempfile::TempDir,
) {
    let root = tempfile::tempdir().unwrap();
    let mut state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    state.conversations =
        Arc::new(ConversationStore::open(root.path().join("conversations")).unwrap());
    state.workflow_runs = Arc::new(WorkflowRunStore::open(root.path().join("runs")).unwrap());
    let source = state
        .conversations
        .create_saved(
            ConversationId::generate().unwrap(),
            None,
            Some(ConversationModelConfiguration::direct(
                crate::providers::ModelSelection::new(
                    crate::providers::ProviderKind::Xai,
                    "model".to_owned(),
                    None,
                )
                .unwrap(),
                crate::tests::test_environment_id(),
            )),
            Vec::new(),
        )
        .unwrap();
    let source = state
        .conversations
        .begin_message_with_model(
            &source.id,
            source.revision,
            None,
            JobId::generate().unwrap(),
            "Prepare changes".to_owned(),
        )
        .unwrap();
    let mut phase = crate::providers::AssistantReply::default();
    phase.push_response("Completed preparation phase");
    state
        .conversations
        .settle_tool_batch(&source.id, source.active_job.unwrap(), &phase)
        .unwrap();
    let source = state.conversations.get(&source.id).unwrap();
    let destination = state
        .conversations
        .create_saved(
            ConversationId::generate().unwrap(),
            None,
            source.model.clone(),
            Vec::new(),
        )
        .unwrap();
    let (run, files) = crate::workflows::handoff::tests::prepared_run(&state, &source);
    state.workflow_runs.create(run.clone()).unwrap();
    (state, root, source, destination, run, files)
}

#[test]
fn committed_transfer_recovers_after_catalogue_failure_without_duplicate_ownership_or_file_writes()
{
    let (state, root, source, destination, run, files) = fixture();
    let connection =
        rusqlite::Connection::open(root.path().join("conversations/conversations.sqlite3"))
            .unwrap();
    // Reject the second projection after SQLite accepts the first one.
    connection
        .execute_batch(&format!(
            "CREATE TRIGGER reject_destination BEFORE UPDATE ON conversations
         WHEN NEW.id = '{}' BEGIN SELECT RAISE(ABORT, 'projection failure'); END;",
            destination.id.as_hex(),
        ))
        .unwrap();
    let job = JobId::generate().unwrap();
    let moved = state
        .conversations
        .transfer_prepared(
            &state.workflow_runs,
            &run,
            source.revision,
            &destination,
            job,
            "Exact continuation",
        )
        .unwrap();
    let committed = state.workflow_runs.get(&run.id).unwrap();
    assert_eq!(committed.conversation_id, Some(destination.id));
    assert!(committed.pending_handoff.is_some());
    assert!(
        state
            .workflow_runs
            .mutate(&run.id, |current| current.cancel_gate(
                run.gates[0].id,
                run.gates[0].revision,
                10
            ))
            .is_err()
    );
    assert_eq!(moved.messages[0].text, "Exact continuation");
    assert_eq!(state.conversations.get(&source.id), Some(source.clone()));
    assert_eq!(
        state.conversations.get(&destination.id),
        Some(destination.clone())
    );
    connection
        .execute_batch("DROP TRIGGER reject_destination")
        .unwrap();
    let conversations = ConversationStore::open(root.path().join("conversations")).unwrap();
    let runs = WorkflowRunStore::open(root.path().join("runs")).unwrap();
    conversations.recover_handoffs(&runs).unwrap();
    conversations.interrupt_requests().unwrap();
    let restored = conversations.get(&destination.id).unwrap();
    assert_eq!(restored.messages.len(), 2);
    assert_eq!(restored.messages[0].text, "Exact continuation");
    assert!(restored.active_job.is_none());
    assert_eq!(restored.messages[1].status, MessageStatus::Interrupted);
    assert!(conversations.get(&source.id).unwrap().active_job.is_none());
    let source = conversations.get(&source.id).unwrap();
    assert_eq!(source.messages[1].text, "Completed preparation phase");
    let last = source.messages.last().unwrap();
    assert_eq!(last.status, MessageStatus::Complete);
    assert!(last.final_phase);
    assert!(runs.for_conversation(&source.id).is_empty());
    assert_eq!(runs.for_conversation(&destination.id).len(), 1);
    assert_eq!(runs.get(&run.id).unwrap().gates, run.gates);
    assert!(runs.get(&run.id).unwrap().pending_handoff.is_none());
    conversations.recover_handoffs(&runs).unwrap();
    assert_eq!(conversations.get(&destination.id).unwrap(), restored);
    assert_eq!(
        std::fs::read_to_string(files.path().join("file.txt")).unwrap(),
        "original\n"
    );
}

#[test]
fn stale_source_revision_and_concurrent_transfers_cannot_move_the_same_run_twice() {
    let (state, _root, source, destination, run, _files) = fixture();
    let changed = state
        .conversations
        .rename(&source.id, source.revision, "New context".to_owned())
        .unwrap();
    assert!(
        state
            .conversations
            .transfer_prepared(
                &state.workflow_runs,
                &run,
                source.revision,
                &destination,
                JobId::generate().unwrap(),
                "Continue"
            )
            .is_err()
    );
    assert_eq!(state.workflow_runs.get(&run.id), Some(run.clone()));
    let other = state
        .conversations
        .create_saved(
            ConversationId::generate().unwrap(),
            None,
            source.model.clone(),
            Vec::new(),
        )
        .unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            barrier.wait();
            state.conversations.transfer_prepared(
                &state.workflow_runs,
                &run,
                changed.revision,
                &destination,
                JobId::generate().unwrap(),
                "First",
            )
        });
        let second = scope.spawn(|| {
            barrier.wait();
            state.conversations.transfer_prepared(
                &state.workflow_runs,
                &run,
                changed.revision,
                &other,
                JobId::generate().unwrap(),
                "Second",
            )
        });
        [first.join().unwrap(), second.join().unwrap()]
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let moved = state.workflow_runs.get(&run.id).unwrap();
    assert_eq!(moved.ownership_history, vec![source.id]);
    assert_eq!(moved.gates, run.gates);
    assert_eq!(
        state.conversations.get(&source.id).unwrap().messages.len(),
        3
    );
}

#[test]
fn recovery_after_catalogue_commit_is_idempotent_and_keeps_original_gate_provenance() {
    let (state, root, source, destination, run, _files) = fixture();
    let job = JobId::generate().unwrap();
    state
        .conversations
        .transfer_prepared(
            &state.workflow_runs,
            &run,
            source.revision,
            &destination,
            job,
            "Continue",
        )
        .unwrap();
    state
        .workflow_runs
        .mutate(&run.id, |current| {
            current.pending_handoff = Some(PendingHandoff {
                source: source.id.as_hex(),
                source_job: source.active_job.map(|id| id.as_hex()),
                destination_revision: destination.revision,
                destination_job: job.as_hex(),
                prompt: "Continue".to_owned(),
            });
            Ok(())
        })
        .unwrap();
    let conversations = ConversationStore::open(root.path().join("conversations")).unwrap();
    let expected_source = conversations.get(&source.id).unwrap();
    let expected_destination = conversations.get(&destination.id).unwrap();
    let runs = WorkflowRunStore::open(root.path().join("runs")).unwrap();
    conversations.recover_handoffs(&runs).unwrap();
    assert_eq!(conversations.get(&source.id).unwrap(), expected_source);
    assert_eq!(
        conversations.get(&destination.id).unwrap(),
        expected_destination
    );
    let recovered = runs.get(&run.id).unwrap();
    assert_eq!(recovered.gates, run.gates);
    assert_eq!(recovered.artefacts, run.artefacts);
    assert_eq!(recovered.state, run.state);
    assert_ne!(
        recovered.decision_revision(&run.gates[0]),
        run.decision_revision(&run.gates[0])
    );
}
