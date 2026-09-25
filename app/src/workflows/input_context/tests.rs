use super::*;

#[test]
fn review_loop_imports_declared_feedback_only_after_the_first_attempt() {
    use crate::workflows::{
        artefacts::*,
        run::*,
        tests::{run, start},
    };
    let mut run = run(crate::workflows::seeds::implement_and_review_definition(
        crate::tests::test_environment_id(),
    ));
    let store = crate::workflows::WorkflowArtefactRepository::in_memory();
    let implementer = run.pinned.definition.steps()[0].clone();
    assert!(
        run.resolve_inputs_before(&implementer, 0, 2)
            .unwrap()
            .is_empty()
    );
    assert!(verify_inputs(&run, &implementer, &[], &store).is_ok());
    let attempt = start(&mut run, vec![], 2);
    run.record_cleanup(attempt, AttemptCleanupRecord::Complete)
        .unwrap();
    run.complete_attempt(attempt, 3).unwrap();
    assert!(verify_inputs(&run, &implementer, &[], &store).is_err());
    let reviewer = run.pinned.definition.steps()[1].clone();
    let attempt = start(&mut run, vec![], 4);
    let verdict = ReviewVerdict::RevisionRequired;
    let (bytes, object_hash, artefact_hash) =
        payload::encode_review(verdict, "Fix the unchecked directory authority.", None).unwrap();
    assert_eq!(store.publish(&bytes).unwrap(), object_hash);
    let output = reviewer
        .required_outputs()
        .iter()
        .find(|output| output.kind == crate::workflows::definition::OutputKind::ReviewReport)
        .unwrap();
    let record = ArtefactRecord {
        id: crate::workflows::ArtefactId::generate().unwrap(),
        kind: ArtefactKind::ReviewReport,
        artefact_hash,
        object_hash,
        payload_bytes: bytes.len() as u64,
        created_at_ms: 5,
        provenance: ArtefactProvenance {
            run_id: run.id,
            producer: ArtefactProducer::StepAttempt {
                attempt_id: attempt,
                step: reviewer.key.clone(),
                output: Some(output.key.clone()),
                disposition: ProductionDisposition::RequiredOutput,
            },
            inputs: vec![],
        },
        summary: ArtefactSummary::Review { verdict },
    };
    let reference = ArtefactReference {
        id: record.id,
        kind: record.kind,
        artefact_hash,
    };
    run.record_attempt_outputs(
        attempt,
        vec![record],
        vec![AttemptArtefactOutput {
            key: output.key.clone(),
            artefact: reference.clone(),
        }],
    )
    .unwrap();
    run.record_cleanup(attempt, AttemptCleanupRecord::Complete)
        .unwrap();
    run.complete_attempt(attempt, 5).unwrap();
    assert_eq!(run.ready_step(), Some(&implementer.key));
    let inputs = run
        .resolve_inputs_before(&implementer, run.attempts.len(), 6)
        .unwrap();
    assert_eq!(inputs[0].artefact, reference);
    let verified = verify_inputs(&run, &implementer, &inputs, &store).unwrap();
    assert!(
        format_agent_context(&verified, true).contains("Fix the unchecked directory authority.")
    );
    assert_eq!(
        verify_inputs(&run, &implementer, &[], &store),
        Err(InputContextError::Missing)
    );
    start(&mut run, inputs.clone(), 6);
    assert!(WorkflowRun::from_file(run.to_file()).is_ok());
    run.artefacts[0].provenance.run_id = crate::workflows::RunId::generate().unwrap();
    assert_eq!(
        verify_inputs(&run, &implementer, &inputs, &store),
        Err(InputContextError::Provenance)
    );
}

#[test]
fn plan_input_requires_exact_provenance_object_and_hash() {
    let mut run = crate::workflows::tests::run(
        crate::workflows::seeds::plan_then_implement_definition(crate::tests::test_environment_id()),
    );
    let store = crate::workflows::WorkflowArtefactRepository::in_memory();
    let plan = crate::workflows::tests::complete_plan(&mut run, &store, "Exact plan", 2);
    let gate = run.pinned.definition.steps()[1].clone();
    let input = AttemptArtefactInput {
        key: gate.inputs[0].key.clone(),
        artefact: plan,
    };
    assert!(verify_inputs(&run, &gate, std::slice::from_ref(&input), &store).is_ok());
    let mut changed = input.clone();
    changed.artefact.artefact_hash = ArtefactHash::of(b"other", b"plan");
    assert_eq!(
        verify_inputs(&run, &gate, &[changed], &store),
        Err(InputContextError::Changed)
    );
    run.artefacts[0].provenance.run_id = crate::workflows::RunId::generate().unwrap();
    assert_eq!(
        verify_inputs(&run, &gate, &[input], &store),
        Err(InputContextError::Provenance)
    );
}

#[test]
fn instruction_and_brief_bounds_reject_untrusted_content() {
    assert_eq!(
        validate_instruction_text("secret", Some("secret")),
        Err(InstructionError::Credential)
    );
    assert_eq!(
        validate_instruction_text("bad\0text", None),
        Err(InstructionError::Invalid)
    );
    assert_eq!(
        validate_instruction_text(&"x".repeat(MAXIMUM_PROJECT_INSTRUCTION_BYTES + 1), None),
        Err(InstructionError::Bound)
    );
    assert!(validate_launch_brief(&"x".repeat(MAXIMUM_LAUNCH_BRIEF_BYTES + 1)).is_err());
    assert!(validate_launch_brief(" ").is_err());
    assert_eq!(
        classify_instruction_exit(Some(4), true),
        Err(InstructionError::Path)
    );
    assert_eq!(
        classify_instruction_exit(Some(3), true),
        Ok(Some(ProjectInstructions::Absent))
    );
}

#[test]
fn packet_integrity_rejects_modified_instruction_text() {
    let mut run = crate::workflows::tests::run(crate::workflows::seeds::plan_a_change_definition(
        crate::tests::test_environment_id(),
    ));
    run.launch_brief = "Inspect the project".into();
    let store = crate::workflows::WorkflowArtefactRepository::in_memory();
    let mut packet = build_attempt_packet_for_request(
        &run,
        &run.pinned.definition.steps()[0],
        &[],
        &store,
        ProjectInstructions::Present(vec![InstructionSource::new(
            "root",
            "/access/root/AGENTS.md",
            "Use Rust".into(),
        )]),
        &[],
        "",
        &[],
        None,
        None,
    )
    .unwrap();
    assert!(packet.validate());
    if let ProjectInstructionState::Present { text, .. } = &mut packet.project_instructions.state {
        *text = "tampered".into();
    }
    assert!(!packet.validate());
}
