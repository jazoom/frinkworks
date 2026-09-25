use super::*;

pub(crate) fn prepared_run(
    state: &crate::state::AppState,
    conversation: &crate::conversations::ConversationRecord,
) -> (WorkflowRun, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("file.txt"), "original\n").unwrap();
    let mut grant = crate::execution::DirectoryGrant::from_selected(directory.path(), &[]).unwrap();
    grant.access = crate::execution::DirectoryAccess::Write;
    let mut settings = conversation.model.as_ref().unwrap().settings.clone();
    settings.directories = vec![grant];
    let definition = crate::workflows::seeds::plan_then_implement_definition(settings.environment)
        .with_conversation_settings(&settings)
        .unwrap();
    let mut run = crate::workflows::tests::run(definition);
    run.conversation_id = Some(conversation.id);
    run.launch_brief = "Implement the accepted plan.".to_owned();
    for environment in &run.environments.environments {
        state.environment_snapshots.mark(
            environment.snapshot.artifact_key.clone(),
            crate::environments::SnapshotAvailability::Available,
        );
    }
    for phase in &mut run.phase_models {
        phase.selection = settings.model.clone();
        phase.instructions = settings.instructions.clone();
        phase.settings = Some(settings.clone());
    }
    let plan = crate::workflows::tests::complete_plan(
        &mut run,
        &state.workflow_artefacts,
        "Implement the task",
        2,
    );
    run.open_plan_gate(crate::workflows::GateId::generate().unwrap(), plan, 4)
        .unwrap();
    (run, directory)
}

#[test]
fn ownership_transfer_invalidates_old_decision_forms_without_touching_files() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let conversation = state
        .conversations
        .create_saved(
            ConversationId::generate().unwrap(),
            Some("Source".into()),
            Some(crate::conversations::ConversationModelConfiguration {
                settings: crate::workflows::tests::settings(),
                preset: None,
            }),
            vec![],
        )
        .unwrap();
    let (mut run, files) = prepared_run(&state, &conversation);
    let revision = run.decision_revision(&run.gates[0]);
    let fingerprint = run.handoff_fingerprint();
    let destination = ConversationId::generate().unwrap();
    assert!(
        run.transfer(&"0".repeat(64), conversation.id, destination)
            .is_err()
    );
    run.transfer(&fingerprint, conversation.id, destination)
        .unwrap();
    assert_ne!(revision, run.decision_revision(&run.gates[0]));
    assert_eq!(run.conversation_id, Some(destination));
    assert_eq!(
        std::fs::read_to_string(files.path().join("file.txt")).unwrap(),
        "original\n"
    );
    assert!(
        run.transfer(&fingerprint, conversation.id, destination)
            .is_err()
    );
}
