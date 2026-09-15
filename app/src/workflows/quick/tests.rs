use super::*;

#[test]
fn ordinary_work_only_requests_application_for_reviewed_directories() {
    for reviewed in [false, true] {
        let pinned = pin_project_free_quick_task_with_directories(
            &[ToolId::Read, ToolId::Write],
            "Change the file.",
            crate::tests::test_environment_id(),
            vec![GuestDirectoryAccess {
                alias: "source".to_owned(),
                access: AccessMode::ReadOnly,
            }],
            reviewed,
        )
        .unwrap();
        assert_eq!(pinned.workflow_id, None);
        assert_eq!(
            pinned.definition.steps().len(),
            if reviewed { 3 } else { 1 }
        );
        assert!(!pinned.definition.steps().iter().any(|step| matches!(&step.action, StepAction::SystemCommand(action) if action.command == SystemCommandId::CommitCandidate)));
        if reviewed {
            assert_eq!(
                pinned.definition.steps()[0].inputs[0].source,
                ArtefactSource::RunCurrentCandidate
            );
            let StepAction::HumanGate(gate) = &pinned.definition.steps()[1].action else {
                panic!("gate")
            };
            assert_eq!(
                gate.revision.as_ref().unwrap().revision_target.as_str(),
                AGENT_STEP_KEY
            );
            assert!(
                matches!(&pinned.definition.steps()[2].action, StepAction::SystemCommand(action) if action.command == SystemCommandId::ApplyChanges)
            );
        }
    }
}

pub(crate) fn pin_quick_task(
    access: AccessMode,
    tools: &[ToolId],
    instructions: &str,
    environment: EnvironmentId,
) -> Result<PinnedWorkflowDefinition, DefinitionError> {
    let pinned = pin_project_free_quick_task_with_directories(
        tools,
        instructions,
        environment,
        Vec::new(),
        access.is_writable(),
    )?;
    let mut steps = pinned.definition.steps().to_vec();
    if !access.is_writable() {
        steps[0].inputs = vec![super::super::definition::initial_candidate_input()];
    }
    let definition = WorkflowDefinition::from_parts(
        pinned.definition.name().to_owned(),
        environment,
        pinned.definition.roles().to_vec(),
        steps,
    )?;
    Ok(PinnedWorkflowDefinition::pin(None, definition))
}
