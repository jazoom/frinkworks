use super::*;

impl WorkflowDefinition {
    pub(crate) fn from_file_bytes(bytes: &[u8]) -> Result<Self, DefinitionError> {
        Self::from_file(serde_json::from_slice(bytes).map_err(|_| DefinitionError::Format)?)
    }
}

pub(crate) fn test_environment_id() -> EnvironmentId {
    EnvironmentId::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap()
}

pub(crate) fn test_named_definition(name: &str) -> WorkflowDefinition {
    WorkflowDefinition::from_parts(
        name.to_owned(),
        test_environment_id(),
        vec![
            RoleDefinition::new(
                RoleKey::parse("agent").unwrap(),
                "Agent".to_owned(),
                String::new(),
                String::new(),
            )
            .unwrap(),
        ],
        vec![StepDefinition {
            key: StepKey::parse("work").unwrap(),
            name: "Work on task".to_owned(),
            inputs: Vec::new(),
            review: None,
            action: StepAction::Agent(AgentStep {
                role: RoleKey::parse("agent").unwrap(),
                environment: StepEnvironment::WorkflowDefault,
                directory_access: StepAccess::Write,
                authority: AgentAuthority::new(vec![ToolId::List], Vec::new()).unwrap(),
                settings: ModelStepSettings::SameAsRunDefaults,
                required_outputs: vec![RequiredOutput {
                    key: OutputKey::parse(ASSISTANT_REPLY).unwrap(),
                    kind: OutputKind::AssistantReply,
                }],
            }),
        }],
    )
    .unwrap()
}

#[test]
fn forward_feedback_requires_the_declared_review_loop() {
    let definition =
        crate::workflows::seeds::implement_and_review_definition(test_environment_id());
    for change in 0..3 {
        let mut steps = definition.steps().to_vec();
        match change {
            0 => steps[1].review = None,
            1 => steps[0].inputs[0].kind = ArtefactKind::TestReport,
            _ => {
                steps[0].inputs[0].source = ArtefactSource::StepOutput {
                    step: steps[1].key.clone(),
                    output: OutputKey::parse("undeclared").unwrap(),
                }
            }
        }
        assert!(
            WorkflowDefinition::from_parts(
                "Invalid feedback".into(),
                test_environment_id(),
                definition.roles().to_vec(),
                steps,
            )
            .is_err()
        );
    }
}

#[test]
fn legacy_permissions_and_snapshot_contracts_fail_closed() {
    let original = serde_json::to_value(test_named_definition("One step").to_file()).unwrap();
    for value in [
        "read-only",
        "write-candidate",
        "review-before-apply",
        "direct-write",
    ] {
        let mut file = original.clone();
        file["steps"][0]["action"]["directory-access"] = value.into();
        assert!(WorkflowDefinition::from_file_bytes(&serde_json::to_vec(&file).unwrap()).is_err());
    }
    for kind in ["candidate-revision", "human-decision"] {
        assert!(OutputKind::parse(kind).is_none());
        assert!(ArtefactKind::parse(kind).is_none());
    }
    let mut file = original.clone();
    file["commit-policy"] = "automatic-after-review".into();
    assert!(WorkflowDefinition::from_file_bytes(&serde_json::to_vec(&file).unwrap()).is_err());
    let mut file = original;
    file["steps"][0]["inputs"] = serde_json::json!([{"key":"source", "kind":"plan", "source":{"source":"run-initial-candidate"}}]);
    assert!(WorkflowDefinition::from_file_bytes(&serde_json::to_vec(&file).unwrap()).is_err());
}

#[test]
fn definition_bounds_and_unique_keys_are_enforced() {
    let original = test_named_definition("One step");
    for count in [0, MAXIMUM_STEPS + 1] {
        assert!(
            WorkflowDefinition::from_parts(
                "Bound".into(),
                test_environment_id(),
                original.roles().to_vec(),
                vec![original.steps()[0].clone(); count]
            )
            .is_err()
        );
    }
    assert!(
        WorkflowDefinition::from_parts(
            " ".into(),
            test_environment_id(),
            original.roles().to_vec(),
            original.steps().to_vec()
        )
        .is_err()
    );
    assert!(
        WorkflowDefinition::from_parts(
            "Duplicate".into(),
            test_environment_id(),
            original.roles().to_vec(),
            vec![original.steps()[0].clone(); 2]
        )
        .is_err()
    );
    for key in ["", "../escape", "9start", "a b"] {
        assert!(StepKey::parse(key).is_err());
    }
}

#[test]
fn plan_gate_requires_a_plan_and_a_bounded_earlier_revision_target() {
    let definition = crate::workflows::seeds::plan_then_implement_definition(test_environment_id());
    let mut steps = definition.steps().to_vec();
    steps[1].inputs.clear();
    assert!(
        WorkflowDefinition::from_parts(
            "Invalid".into(),
            test_environment_id(),
            definition.roles().to_vec(),
            steps
        )
        .is_err()
    );
    for limit in [0, MAXIMUM_REVIEW_ATTEMPTS + 1] {
        let mut steps = definition.steps().to_vec();
        let StepAction::HumanGate(gate) = &mut steps[1].action else {
            unreachable!()
        };
        gate.revision.as_mut().unwrap().attempt_limit = limit;
        assert!(
            WorkflowDefinition::from_parts(
                "Invalid".into(),
                test_environment_id(),
                definition.roles().to_vec(),
                steps
            )
            .is_err()
        );
    }
}

#[test]
fn read_phase_rejects_host_execution() {
    let definition = crate::workflows::seeds::plan_a_change_definition(test_environment_id());
    let settings =
        crate::workflows::tests::settings().with_location(crate::execution::ToolLocation::Host);
    assert!(definition.with_conversation_settings(&settings).is_err());
}
