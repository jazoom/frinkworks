use super::*;
use crate::workflows::{
    artefacts::*,
    tests::{complete_plan, run, start},
};

fn reload(run: &WorkflowRun) -> Result<WorkflowRun, RunRecordError> {
    let bytes = serde_json::to_vec(&run.to_file()).unwrap();
    WorkflowRun::from_file(serde_json::from_slice(&bytes).unwrap())
}

#[test]
fn system_only_workflow_keeps_pinned_settings_without_model_phases() {
    let mut file =
        serde_json::to_value(crate::tests::test_named_definition("Status").to_file()).unwrap();
    file["roles"] = serde_json::json!([]);
    file["steps"][0]["action"] = serde_json::json!({
        "type": "system-command", "command": "repository-status",
        "environment": {"source": "workflow-default"}, "required-outputs": []
    });
    let definition = WorkflowDefinition::from_file(serde_json::from_value(file).unwrap()).unwrap();
    let run = run(definition);
    assert!(run.phase_models.is_empty());
    assert_eq!(
        reload(&run).unwrap().directory_settings(),
        Some(run.settings.clone())
    );
}

#[test]
fn live_write_and_read_attempts_survive_valid_record_round_trips() {
    for definition in [
        crate::workflows::seeds::implement_a_change_definition(crate::tests::test_environment_id()),
        crate::workflows::seeds::plan_a_change_definition(crate::tests::test_environment_id()),
    ] {
        let mut run = run(definition);
        assert_eq!(reload(&run).unwrap(), run);
        let attempt = start(&mut run, Vec::new(), 2);
        assert_eq!(reload(&run).unwrap(), run);
        run.record_cleanup(attempt, AttemptCleanupRecord::Complete)
            .unwrap();
        run.fail_attempt(attempt, FailureCategory::Tool, 3).unwrap();
        assert_eq!(reload(&run).unwrap(), run);
    }
}

#[test]
fn changed_capabilities_and_output_provenance_are_rejected_on_reload() {
    let mut run = run(crate::workflows::seeds::plan_a_change_definition(
        crate::tests::test_environment_id(),
    ));
    let store = WorkflowArtefactRepository::in_memory();
    complete_plan(&mut run, &store, "Plan", 2);
    assert!(reload(&run).is_ok());
    let mut changed = run.clone();
    changed.attempts[0].capabilities.tools.clear();
    assert!(reload(&changed).is_err());
    let mut changed = run.clone();
    changed.artefacts[0].provenance.run_id = RunId::generate().unwrap();
    assert!(reload(&changed).is_err());
    let mut changed = run.clone();
    changed.attempts[0].outputs.clear();
    assert!(reload(&changed).is_err());
    let mut changed = run;
    changed.attempts[0].cleanup = AttemptCleanupRecord::Pending;
    assert!(reload(&changed).is_err());
}

#[test]
fn missing_required_inputs_and_changed_provenance_fail_restart() {
    let mut run = run(crate::workflows::seeds::plan_then_implement_definition(
        crate::tests::test_environment_id(),
    ));
    let store = WorkflowArtefactRepository::in_memory();
    let plan = complete_plan(&mut run, &store, "Plan", 2);
    let gate = run
        .open_plan_gate(GateId::generate().unwrap(), plan, 4)
        .unwrap();
    let kind = crate::workflows::gates::PlanDecisionKind::Accepted;
    let output = decision(&run, &gate, kind, None, 5);
    run.decide_plan_gate(gate.id, gate.revision, output, kind, None, None, 5)
        .unwrap();
    let step = run
        .pinned
        .definition
        .step(run.ready_step().unwrap())
        .unwrap();
    let inputs = run
        .resolve_inputs_before(step, run.attempts.len(), 6)
        .unwrap();
    start(&mut run, inputs, 6);
    assert!(reload(&run).is_ok());
    let mut missing = run.clone();
    missing.attempts.last_mut().unwrap().inputs.clear();
    assert!(reload(&missing).is_err());
    run.artefacts[0]
        .provenance
        .inputs
        .push(gate.candidate.clone());
    assert!(reload(&run).is_err());
}

fn decision(
    run: &WorkflowRun,
    gate: &HumanGateRecord,
    kind: crate::workflows::gates::PlanDecisionKind,
    note: Option<&str>,
    at: u64,
) -> ArtefactRecord {
    let (bytes, object_hash, artefact_hash) =
        encode_plan_decision(gate.candidate.artefact_hash, kind, note, at, None).unwrap();
    ArtefactRecord {
        id: crate::workflows::ArtefactId::generate().unwrap(),
        kind: crate::workflows::definition::ArtefactKind::PlanDecision,
        object_hash,
        artefact_hash,
        payload_bytes: bytes.len() as u64,
        created_at_ms: at,
        provenance: ArtefactProvenance {
            run_id: run.id,
            producer: ArtefactProducer::HumanGate {
                gate_id: gate.id,
                step: gate.step.clone(),
                output: gate.output.clone(),
            },
            inputs: vec![gate.candidate.clone()],
        },
        summary: ArtefactSummary::PlanDecision {
            plan: gate.candidate.artefact_hash,
            decision: kind,
        },
    }
}

#[test]
fn plan_gate_identity_revision_and_attempt_limit_survive_restart() {
    use crate::workflows::gates::PlanDecisionKind;
    let mut run = run(crate::workflows::seeds::plan_then_implement_definition(
        crate::tests::test_environment_id(),
    ));
    let store = WorkflowArtefactRepository::in_memory();
    for index in 0..3 {
        let at = 2 + index * 4;
        let plan = complete_plan(&mut run, &store, &format!("Plan {index}"), at);
        let gate = run
            .open_plan_gate(GateId::generate().unwrap(), plan, at + 2)
            .unwrap();
        run = reload(&run).unwrap();
        let output = decision(
            &run,
            &gate,
            PlanDecisionKind::RevisionRequested,
            Some("Change the plan"),
            at + 3,
        );
        assert!(
            run.decide_plan_gate(
                gate.id,
                GateRevision::new(99).unwrap(),
                output.clone(),
                PlanDecisionKind::RevisionRequested,
                Some("Change the plan".into()),
                Some(AttemptId::generate().unwrap()),
                at + 3
            )
            .is_err()
        );
        run.decide_plan_gate(
            gate.id,
            gate.revision,
            output,
            PlanDecisionKind::RevisionRequested,
            Some("Change the plan".into()),
            Some(AttemptId::generate().unwrap()),
            at + 3,
        )
        .unwrap();
        run = reload(&run).unwrap();
    }
    assert!(matches!(
        run.state,
        RunState::Escalated {
            reason: EscalationReason::AttemptLimit,
            ..
        }
    ));
    assert!(run.revision_reservation.is_none());
    let mut revived = run.clone();
    revived.state = RunState::Ready {
        step: StepKey::parse("planner").unwrap(),
    };
    assert!(reload(&revived).is_err());
}

#[test]
fn accepted_plan_advances_without_file_application() {
    let mut run = run(crate::workflows::seeds::plan_then_implement_definition(
        crate::tests::test_environment_id(),
    ));
    let store = WorkflowArtefactRepository::in_memory();
    let plan = complete_plan(&mut run, &store, "Plan", 2);
    let gate = run
        .open_plan_gate(GateId::generate().unwrap(), plan, 4)
        .unwrap();
    let kind = crate::workflows::gates::PlanDecisionKind::Accepted;
    let output = decision(&run, &gate, kind, None, 5);
    run.decide_plan_gate(gate.id, gate.revision, output, kind, None, None, 5)
        .unwrap();
    assert_eq!(run.ready_step().unwrap().as_str(), "implementer");
    assert!(reload(&run).is_ok());
}

#[test]
fn plan_decisions_reject_unrelated_plans_and_forged_timestamps() {
    let mut run = run(crate::workflows::seeds::plan_then_implement_definition(
        crate::tests::test_environment_id(),
    ));
    let store = WorkflowArtefactRepository::in_memory();
    let original = complete_plan(&mut run, &store, "Original plan", 2);
    let gate = run
        .open_plan_gate(GateId::generate().unwrap(), original.clone(), 4)
        .unwrap();
    let kind = crate::workflows::gates::PlanDecisionKind::RevisionRequested;
    let output = decision(&run, &gate, kind, Some("Revise"), 5);
    run.decide_plan_gate(
        gate.id,
        gate.revision,
        output,
        kind,
        Some("Revise".into()),
        Some(AttemptId::generate().unwrap()),
        5,
    )
    .unwrap();
    let revised = complete_plan(&mut run, &store, "Revised plan", 6);
    assert!(
        run.open_plan_gate(GateId::generate().unwrap(), original, 8)
            .is_err()
    );
    let gate = run
        .open_plan_gate(GateId::generate().unwrap(), revised, 8)
        .unwrap();
    let kind = crate::workflows::gates::PlanDecisionKind::Accepted;
    let output = decision(&run, &gate, kind, None, 9);
    let mut forged = output.clone();
    forged.created_at_ms = 7;
    assert!(
        run.decide_plan_gate(gate.id, gate.revision, forged, kind, None, None, 9)
            .is_err()
    );
    run.decide_plan_gate(gate.id, gate.revision, output, kind, None, None, 9)
        .unwrap();
    assert!(reload(&run).is_ok());
    run.artefacts.last_mut().unwrap().created_at_ms = 7;
    assert!(reload(&run).is_err());
}

#[test]
fn interrupted_and_cancelled_attempts_cannot_restart() {
    for cancel in [false, true] {
        let mut run = run(crate::tests::test_named_definition("Task"));
        let attempt = start(&mut run, Vec::new(), 2);
        run.record_cleanup(attempt, AttemptCleanupRecord::Complete)
            .unwrap();
        if cancel {
            run.cancel(3).unwrap();
        } else {
            run.interrupt(3).unwrap();
        }
        assert!(run.is_terminal());
        assert!(run.ready_step().is_none());
        assert!(reload(&run).is_ok());
    }
}
