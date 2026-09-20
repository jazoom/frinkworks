use super::{
    CommitError, NON_APPROVED_MESSAGE, commit_tree_command, hash_object_command,
    index_info_command, parse_object_id, require_commit_approval, require_unchanged_project,
    utc_timestamp, write_tree_command,
};
use crate::tests::test_environment_id;
use crate::workflows::artefacts::candidate::GitObjectFormat;
use crate::workflows::artefacts::{
    ArtefactProducer, ArtefactProvenance, ArtefactRecord, ArtefactReference, ArtefactSummary,
    CandidateHash, ProductionDisposition, ReviewVerdict, WorkflowArtefactRepository,
    artefact_hash_for, payload,
};
use crate::workflows::definition::{
    ArtefactKind, ArtefactSource, InputKey, OutputKey, PinnedWorkflowDefinition, RequiredInput,
    StepDefinition, StepKey,
};
use crate::workflows::gates::{GateRevision, HumanDecisionKind, HumanGateRecord, HumanGateState};
use crate::workflows::id::{ArtefactId, AttemptId, GateId, RunId};
use crate::workflows::run::{
    AttemptArtefactInput, ObservedCandidate, RunSource, RunSourceState, WorkflowRun,
};
use crate::workflows::seeds::correctness_security_definition;

fn store() -> WorkflowArtefactRepository {
    WorkflowArtefactRepository::in_memory()
}

#[test]
fn journal_paths_are_derived_from_identifiers() {
    let dir = tempfile::tempdir().expect("dir");
    let journals =
        super::journal::CommitJournals::open(dir.path().join("workflow-commit-journals"))
            .expect("open");
    let run = RunId::generate().expect("run");
    let attempt = AttemptId::generate().expect("attempt");
    let journal = journals.create(run, attempt).expect("create");
    journal
        .write_index_backup("original.index", b"idx")
        .expect("write");
    journal.flush().expect("flush");
    assert_eq!(
        journal.read_index_backup("original.index").expect("read"),
        b"idx"
    );
    assert!(
        journals
            .path(run, attempt)
            .expect("derived")
            .ends_with(attempt.as_hex())
    );
    assert!(journals.load(run, attempt).is_ok());
    journals.remove(run, attempt).expect("remove");
    assert!(journals.load(run, attempt).is_err());
}

#[test]
fn command_contract_rejects_non_approved_and_stale_reviews() {
    let store = store();
    let definition = correctness_security_definition(test_environment_id());
    let environments = crate::tests::test_environment_set(&definition);
    let mut run = WorkflowRun::configured(
        RunId::generate().expect("run"),
        1,
        crate::agents::AgentId::generate().expect("agent"),
        PinnedWorkflowDefinition::pin(None, definition),
        environments,
    );
    let dir = tempfile::tempdir().expect("git");
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .expect("git")
            .success()
    );
    let captured = crate::workflows::artefacts::CandidateCapture::capture_host(dir.path(), &store)
        .expect("capture");
    let bytes = captured.manifest_bytes().expect("manifest");
    let object = store.publish(&bytes).expect("object");
    let candidate = ArtefactRecord {
        id: ArtefactId::generate().expect("id"),
        kind: ArtefactKind::CandidateRevision,
        artefact_hash: artefact_hash_for(
            ArtefactKind::CandidateRevision,
            crate::workflows::artefacts::CANDIDATE_SCHEMA,
            &bytes,
        ),
        object_hash: object,
        payload_bytes: bytes.len() as u64,
        created_at_ms: 1,
        provenance: ArtefactProvenance {
            run_id: run.id,
            producer: ArtefactProducer::StepAttempt {
                attempt_id: AttemptId::generate().expect("attempt"),
                step: StepKey::parse("implementer").expect("step"),
                output: Some(OutputKey::parse("candidate").expect("output")),
                disposition: ProductionDisposition::RequiredOutput,
            },
            inputs: Vec::new(),
        },
        summary: ArtefactSummary::Candidate {
            candidate: captured.candidate_hash,
            entries: 0,
            bytes: 0,
            disposition: ProductionDisposition::RequiredOutput,
        },
    };
    let reference = ArtefactReference {
        id: candidate.id,
        kind: candidate.kind,
        artefact_hash: candidate.artefact_hash,
    };
    run.artefacts.push(candidate.clone());
    run.source = RunSource::Captured {
        source: RunSourceState {
            initial: reference.clone(),
            accepted: reference.clone(),
            observed: ObservedCandidate::Exact {
                artefact: reference.clone(),
            },
        },
    };

    let (bytes, object, hash) = payload::encode_review(
        captured.candidate_hash,
        ReviewVerdict::RevisionRequired,
        "needs work",
        None,
    )
    .expect("review");
    store.publish(&bytes).expect("store");
    let review = ArtefactRecord {
        id: ArtefactId::generate().expect("id"),
        kind: ArtefactKind::ReviewReport,
        artefact_hash: hash,
        object_hash: object,
        payload_bytes: bytes.len() as u64,
        created_at_ms: 1,
        provenance: ArtefactProvenance {
            run_id: run.id,
            producer: ArtefactProducer::StepAttempt {
                attempt_id: AttemptId::generate().expect("attempt"),
                step: StepKey::parse("correctness-review").expect("step"),
                output: Some(OutputKey::parse("review").expect("output")),
                disposition: ProductionDisposition::RequiredOutput,
            },
            inputs: vec![reference.clone()],
        },
        summary: ArtefactSummary::Review {
            candidate: captured.candidate_hash,
            verdict: ReviewVerdict::RevisionRequired,
        },
    };
    let mut security_review = review.clone();
    security_review.id = ArtefactId::generate().expect("id");
    security_review.provenance.producer = ArtefactProducer::StepAttempt {
        attempt_id: AttemptId::generate().expect("attempt"),
        step: StepKey::parse("security-review").expect("step"),
        output: Some(OutputKey::parse("review").expect("output")),
        disposition: ProductionDisposition::RequiredOutput,
    };
    run.artefacts
        .extend([review.clone(), security_review.clone()]);
    let mut inputs = vec![
        AttemptArtefactInput {
            key: crate::workflows::definition::InputKey::parse("candidate").expect("key"),
            artefact: reference,
        },
        AttemptArtefactInput {
            key: crate::workflows::definition::InputKey::parse("correctness-review").expect("key"),
            artefact: ArtefactReference {
                id: review.id,
                kind: review.kind,
                artefact_hash: review.artefact_hash,
            },
        },
        AttemptArtefactInput {
            key: crate::workflows::definition::InputKey::parse("security-review").expect("key"),
            artefact: ArtefactReference {
                id: security_review.id,
                kind: security_review.kind,
                artefact_hash: security_review.artefact_hash,
            },
        },
    ];
    let commit_step = run.pinned.definition.steps().iter().find(|step| matches!(&step.action, crate::workflows::definition::StepAction::SystemCommand(action) if action.command == crate::workflows::commands::SystemCommandId::CommitCandidate)).expect("commit step");
    assert_eq!(
        require_commit_approval(&run, commit_step, &inputs, &store).err(),
        Some(CommitError::Assurance)
    );
    assert_eq!(CommitError::Assurance.message(), NON_APPROVED_MESSAGE);

    for (bound, accepted) in [
        (captured.candidate_hash, true),
        (
            crate::workflows::artefacts::CandidateHash::of(b"stale"),
            false,
        ),
    ] {
        let (bytes, object, hash) =
            payload::encode_review(bound, ReviewVerdict::Approved, "approved", None)
                .expect("approved review");
        store.publish(&bytes).expect("store approved review");
        for stored_review in run
            .artefacts
            .iter_mut()
            .filter(|record| record.kind == ArtefactKind::ReviewReport)
        {
            stored_review.object_hash = object;
            stored_review.artefact_hash = hash;
            stored_review.summary = ArtefactSummary::Review {
                candidate: bound,
                verdict: ReviewVerdict::Approved,
            };
        }
        for input in &mut inputs[1..] {
            input.artefact.artefact_hash = hash;
        }
        assert_eq!(
            require_commit_approval(&run, commit_step, &inputs, &store).is_ok(),
            accepted
        );
    }
}

fn captured_candidate(
    store: &WorkflowArtefactRepository,
) -> (
    WorkflowRun,
    ArtefactRecord,
    ArtefactReference,
    crate::workflows::artefacts::candidate::CandidateRevisionArtefact,
    tempfile::TempDir,
) {
    let definition = correctness_security_definition(test_environment_id());
    let environments = crate::tests::test_environment_set(&definition);
    let mut run = WorkflowRun::configured(
        RunId::generate().expect("run"),
        1,
        crate::agents::AgentId::generate().expect("agent"),
        PinnedWorkflowDefinition::pin(None, definition),
        environments,
    );
    let dir = tempfile::tempdir().expect("git");
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .expect("git")
            .success()
    );
    let captured = crate::workflows::artefacts::CandidateCapture::capture_host(dir.path(), store)
        .expect("capture");
    let bytes = captured.manifest_bytes().expect("manifest");
    let object = store.publish(&bytes).expect("object");
    let candidate = ArtefactRecord {
        id: ArtefactId::generate().expect("id"),
        kind: ArtefactKind::CandidateRevision,
        artefact_hash: artefact_hash_for(
            ArtefactKind::CandidateRevision,
            crate::workflows::artefacts::CANDIDATE_SCHEMA,
            &bytes,
        ),
        object_hash: object,
        payload_bytes: bytes.len() as u64,
        created_at_ms: 1,
        provenance: ArtefactProvenance {
            run_id: run.id,
            producer: ArtefactProducer::StepAttempt {
                attempt_id: AttemptId::generate().expect("attempt"),
                step: StepKey::parse("implementer").expect("step"),
                output: Some(OutputKey::parse("candidate").expect("output")),
                disposition: ProductionDisposition::RequiredOutput,
            },
            inputs: Vec::new(),
        },
        summary: ArtefactSummary::Candidate {
            candidate: captured.candidate_hash,
            entries: 0,
            bytes: 0,
            disposition: ProductionDisposition::RequiredOutput,
        },
    };
    let reference = ArtefactReference {
        id: candidate.id,
        kind: candidate.kind,
        artefact_hash: candidate.artefact_hash,
    };
    run.artefacts.push(candidate.clone());
    run.source = RunSource::Captured {
        source: RunSourceState {
            initial: reference.clone(),
            accepted: reference.clone(),
            observed: ObservedCandidate::Exact {
                artefact: reference.clone(),
            },
        },
    };
    (run, candidate, reference, captured, dir)
}

fn commit_step(run: &WorkflowRun) -> StepDefinition {
    run.pinned
        .definition
        .steps()
        .iter()
        .find(|step| {
            matches!(
                &step.action,
                crate::workflows::definition::StepAction::SystemCommand(action)
                    if action.command == crate::workflows::commands::SystemCommandId::CommitCandidate
            )
        })
        .expect("commit step")
        .clone()
}

fn decision_input() -> RequiredInput {
    RequiredInput {
        key: InputKey::parse("decision").expect("key"),
        kind: ArtefactKind::HumanDecision,
        source: ArtefactSource::StepOutput {
            step: StepKey::parse("approve").expect("step"),
            output: OutputKey::parse("decision").expect("output"),
        },
    }
}

fn with_decision_input(mut step: StepDefinition) -> StepDefinition {
    step.inputs.push(decision_input());
    step
}

fn use_human_policy(run: &mut WorkflowRun) {
    use crate::workflows::definition::{
        HumanGateStep, OutputKind, RequiredOutput, StepAction, WorkflowDefinition,
    };
    let definition = &run.pinned.definition;
    let mut steps = definition.steps().to_vec();
    let index = steps
        .iter()
        .position(|step| step.key == commit_step(run).key)
        .expect("commit");
    let candidate = steps[index]
        .inputs
        .iter()
        .find(|input| input.kind == ArtefactKind::CandidateRevision)
        .expect("candidate")
        .clone();
    steps[index].inputs.push(decision_input());
    steps.insert(
        index,
        StepDefinition {
            key: StepKey::parse("approve").expect("gate"),
            name: "Approve candidate".to_owned(),
            inputs: vec![candidate],
            action: StepAction::HumanGate(HumanGateStep {
                required_output: RequiredOutput {
                    key: OutputKey::parse("decision").expect("decision"),
                    kind: OutputKind::HumanDecision,
                },
                revision: None,
            }),
            review: None,
        },
    );
    run.pinned = crate::workflows::definition::PinnedWorkflowDefinition::pin(
        None,
        WorkflowDefinition::from_parts(
            definition.name().to_owned(),
            definition.default_environment(),
            definition.roles().to_vec(),
            steps,
        )
        .expect("human policy"),
    );
}

fn decision_only_step(mut step: StepDefinition) -> StepDefinition {
    step.inputs
        .retain(|input| input.kind == ArtefactKind::CandidateRevision);
    with_decision_input(step)
}

fn approved_review(
    run: &WorkflowRun,
    candidate: &ArtefactReference,
    hash: CandidateHash,
    step: &str,
    store: &WorkflowArtefactRepository,
) -> ArtefactRecord {
    let (bytes, object, artefact_hash) =
        payload::encode_review(hash, ReviewVerdict::Approved, "approved", None).expect("review");
    store.publish(&bytes).expect("store");
    ArtefactRecord {
        id: ArtefactId::generate().expect("id"),
        kind: ArtefactKind::ReviewReport,
        artefact_hash,
        object_hash: object,
        payload_bytes: bytes.len() as u64,
        created_at_ms: 1,
        provenance: ArtefactProvenance {
            run_id: run.id,
            producer: ArtefactProducer::StepAttempt {
                attempt_id: AttemptId::generate().expect("attempt"),
                step: StepKey::parse(step).expect("step"),
                output: Some(OutputKey::parse("review").expect("output")),
                disposition: ProductionDisposition::RequiredOutput,
            },
            inputs: vec![candidate.clone()],
        },
        summary: ArtefactSummary::Review {
            candidate: hash,
            verdict: ReviewVerdict::Approved,
        },
    }
}

fn approved_decision(
    run: &WorkflowRun,
    candidate: &ArtefactReference,
    candidate_hash: CandidateHash,
    diff_base: CandidateHash,
    store: &WorkflowArtefactRepository,
) -> (ArtefactRecord, HumanGateRecord, ArtefactReference) {
    let gate_id = GateId::generate().expect("gate");
    let (bytes, object, artefact_hash) = payload::encode_human_decision(
        candidate_hash,
        diff_base,
        HumanDecisionKind::Approved,
        None,
        2,
        None,
    )
    .expect("decision");
    store.publish(&bytes).expect("store");
    let reference = ArtefactReference {
        id: ArtefactId::generate().expect("id"),
        kind: ArtefactKind::HumanDecision,
        artefact_hash,
    };
    let record = ArtefactRecord {
        id: reference.id,
        kind: ArtefactKind::HumanDecision,
        artefact_hash,
        object_hash: object,
        payload_bytes: bytes.len() as u64,
        created_at_ms: 2,
        provenance: ArtefactProvenance {
            run_id: run.id,
            producer: ArtefactProducer::HumanGate {
                gate_id,
                step: StepKey::parse("approve").expect("step"),
                output: OutputKey::parse("decision").expect("output"),
            },
            inputs: vec![candidate.clone()],
        },
        summary: ArtefactSummary::HumanDecision {
            candidate: candidate_hash,
            diff_base,
            decision: HumanDecisionKind::Approved,
        },
    };
    let gate = HumanGateRecord {
        id: gate_id,
        step: StepKey::parse("approve").expect("step"),
        sequence: 1,
        revision: GateRevision::new(1).expect("revision"),
        opened_at_ms: 1,
        closed_at_ms: Some(2),
        candidate: candidate.clone(),
        diff_base: candidate.clone(),
        state: HumanGateState::Approved,
        decision: Some(reference.clone()),
        output: OutputKey::parse("decision").expect("output"),
    };
    (record, gate, reference)
}

fn candidate_input(reference: &ArtefactReference) -> AttemptArtefactInput {
    AttemptArtefactInput {
        key: InputKey::parse("candidate").expect("key"),
        artefact: reference.clone(),
    }
}

fn review_input(key: &str, record: &ArtefactRecord) -> AttemptArtefactInput {
    AttemptArtefactInput {
        key: InputKey::parse(key).expect("key"),
        artefact: ArtefactReference {
            id: record.id,
            kind: record.kind,
            artefact_hash: record.artefact_hash,
        },
    }
}

fn decision_attempt_input(reference: &ArtefactReference) -> AttemptArtefactInput {
    AttemptArtefactInput {
        key: InputKey::parse("decision").expect("key"),
        artefact: reference.clone(),
    }
}

#[test]
fn commit_approval_accepts_review_and_decision_shapes() {
    let store = store();
    let (mut run, _, reference, captured, _dir) = captured_candidate(&store);
    let correctness = approved_review(
        &run,
        &reference,
        captured.candidate_hash,
        "correctness-review",
        &store,
    );
    let security = approved_review(
        &run,
        &reference,
        captured.candidate_hash,
        "security-review",
        &store,
    );
    let (decision, gate, decision_reference) = approved_decision(
        &run,
        &reference,
        captured.candidate_hash,
        captured.candidate_hash,
        &store,
    );
    run.artefacts
        .extend([correctness.clone(), security.clone(), decision.clone()]);
    run.gates.push(gate);

    let reviews = vec![
        candidate_input(&reference),
        review_input("correctness-review", &correctness),
        review_input("security-review", &security),
    ];
    assert!(require_commit_approval(&run, &commit_step(&run), &reviews, &store).is_ok());

    let decision_only = vec![
        candidate_input(&reference),
        decision_attempt_input(&decision_reference),
    ];
    assert!(
        require_commit_approval(
            &run,
            &decision_only_step(commit_step(&run)),
            &decision_only,
            &store
        )
        .is_err()
    );
    use_human_policy(&mut run);
    assert!(require_commit_approval(&run, &commit_step(&run), &reviews, &store).is_err());
    assert!(
        require_commit_approval(
            &run,
            &decision_only_step(commit_step(&run)),
            &decision_only,
            &store
        )
        .is_ok()
    );

    let mut both = reviews;
    both.push(decision_attempt_input(&decision_reference));
    assert!(require_commit_approval(&run, &commit_step(&run), &both, &store).is_ok());
}

#[test]
fn commit_approval_rejects_missing_and_unrelated_authority() {
    let store = store();
    let (mut run, _, reference, captured, _dir) = captured_candidate(&store);
    let (decision, gate, decision_reference) = approved_decision(
        &run,
        &reference,
        captured.candidate_hash,
        captured.candidate_hash,
        &store,
    );
    run.artefacts.push(decision);
    run.gates.push(gate);
    use_human_policy(&mut run);
    let step = decision_only_step(commit_step(&run));
    let decision_input = decision_attempt_input(&decision_reference);
    let candidate = candidate_input(&reference);

    assert_eq!(
        require_commit_approval(&run, &step, std::slice::from_ref(&candidate), &store).err(),
        Some(CommitError::Assurance)
    );
    assert_eq!(
        require_commit_approval(&run, &step, &[candidate.clone(), candidate.clone()], &store).err(),
        Some(CommitError::Assurance)
    );
    assert_eq!(
        require_commit_approval(
            &run,
            &step,
            &[
                candidate.clone(),
                decision_input.clone(),
                decision_input.clone(),
            ],
            &store
        )
        .err(),
        Some(CommitError::Assurance)
    );
    let plan = AttemptArtefactInput {
        key: InputKey::parse("plan").expect("key"),
        artefact: ArtefactReference {
            id: ArtefactId::generate().expect("id"),
            kind: ArtefactKind::Plan,
            artefact_hash: reference.artefact_hash,
        },
    };
    assert_eq!(
        require_commit_approval(&run, &step, &[candidate.clone(), plan.clone()], &store).err(),
        Some(CommitError::Assurance)
    );
    let test = AttemptArtefactInput {
        key: InputKey::parse("test").expect("key"),
        artefact: ArtefactReference {
            id: ArtefactId::generate().expect("id"),
            kind: ArtefactKind::TestReport,
            artefact_hash: reference.artefact_hash,
        },
    };
    assert_eq!(
        require_commit_approval(&run, &step, &[candidate, decision_input, test], &store).err(),
        Some(CommitError::Assurance)
    );
}

#[test]
fn commit_approval_rejects_decision_provenance_mismatches() {
    let store = store();
    let (mut run, _, reference, captured, _dir) = captured_candidate(&store);
    use_human_policy(&mut run);
    let (decision, gate, decision_reference) = approved_decision(
        &run,
        &reference,
        captured.candidate_hash,
        captured.candidate_hash,
        &store,
    );
    let step = decision_only_step(commit_step(&run));
    let inputs = vec![
        candidate_input(&reference),
        decision_attempt_input(&decision_reference),
    ];

    let mut other_run = run.clone();
    let mut foreign = decision.clone();
    foreign.provenance.run_id = RunId::generate().expect("other run");
    other_run.artefacts.push(foreign);
    other_run.gates.push(gate.clone());
    assert_eq!(
        require_commit_approval(&other_run, &step, &inputs, &store).err(),
        Some(CommitError::Assurance)
    );

    let mut other_gate = run.clone();
    let mut misplaced = decision.clone();
    if let ArtefactProducer::HumanGate { gate_id, .. } = &mut misplaced.provenance.producer {
        *gate_id = GateId::generate().expect("other gate");
    }
    other_gate.artefacts.push(misplaced);
    other_gate.gates.push(gate.clone());
    assert_eq!(
        require_commit_approval(&other_gate, &step, &inputs, &store).err(),
        Some(CommitError::Assurance)
    );

    let mut other_output = run.clone();
    let mut renamed = decision.clone();
    if let ArtefactProducer::HumanGate { output, .. } = &mut renamed.provenance.producer {
        *output = OutputKey::parse("other").expect("output");
    }
    other_output.artefacts.push(renamed);
    other_output.gates.push(gate.clone());
    assert_eq!(
        require_commit_approval(&other_output, &step, &inputs, &store).err(),
        Some(CommitError::Assurance)
    );

    let mut undeclared = step.clone();
    undeclared.inputs.last_mut().expect("decision").source = ArtefactSource::RunCurrentCandidate;
    let mut named = run.clone();
    named.artefacts.push(decision.clone());
    named.gates.push(gate.clone());
    assert_eq!(
        require_commit_approval(&named, &undeclared, &inputs, &store).err(),
        Some(CommitError::Assurance)
    );

    let mut unclosed = run.clone();
    let mut open = gate.clone();
    open.closed_at_ms = None;
    unclosed.artefacts.push(decision.clone());
    unclosed.gates.push(open);
    assert_eq!(
        require_commit_approval(&unclosed, &step, &inputs, &store).err(),
        Some(CommitError::Assurance)
    );

    let wrong_hash = artefact_hash_for(ArtefactKind::HumanDecision, 1, b"wrong object identity");
    let mut wrong_identity = run.clone();
    let mut mismatched = decision.clone();
    mismatched.artefact_hash = wrong_hash;
    let mut mismatched_gate = gate.clone();
    mismatched_gate.decision = Some(ArtefactReference {
        id: mismatched.id,
        kind: mismatched.kind,
        artefact_hash: wrong_hash,
    });
    let mismatched_inputs = vec![
        candidate_input(&reference),
        AttemptArtefactInput {
            key: InputKey::parse("decision").expect("key"),
            artefact: ArtefactReference {
                id: mismatched.id,
                kind: mismatched.kind,
                artefact_hash: wrong_hash,
            },
        },
    ];
    wrong_identity.artefacts.push(mismatched);
    wrong_identity.gates.push(mismatched_gate);
    assert_eq!(
        require_commit_approval(&wrong_identity, &step, &mismatched_inputs, &store).err(),
        Some(CommitError::Assurance)
    );

    let mut revision = run.clone();
    let (bytes, object, hash) = payload::encode_human_decision(
        captured.candidate_hash,
        captured.candidate_hash,
        HumanDecisionKind::RevisionRequested,
        Some("change this"),
        2,
        None,
    )
    .expect("revision");
    store.publish(&bytes).expect("store revision");
    let mut requested = decision.clone();
    requested.object_hash = object;
    requested.artefact_hash = hash;
    requested.summary = ArtefactSummary::HumanDecision {
        candidate: captured.candidate_hash,
        diff_base: captured.candidate_hash,
        decision: HumanDecisionKind::RevisionRequested,
    };
    let mut requested_gate = gate.clone();
    requested_gate.state = HumanGateState::RevisionRequested;
    requested_gate.decision = Some(ArtefactReference {
        id: requested.id,
        kind: requested.kind,
        artefact_hash: hash,
    });
    let requested_inputs = vec![
        candidate_input(&reference),
        AttemptArtefactInput {
            key: InputKey::parse("decision").expect("key"),
            artefact: ArtefactReference {
                id: requested.id,
                kind: requested.kind,
                artefact_hash: hash,
            },
        },
    ];
    revision.artefacts.push(requested);
    revision.gates.push(requested_gate);
    assert_eq!(
        require_commit_approval(&revision, &step, &requested_inputs, &store).err(),
        Some(CommitError::Assurance)
    );

    let mut other_candidate = run.clone();
    let stale = CandidateHash::of(b"stale");
    let (bytes, object, hash) = payload::encode_human_decision(
        stale,
        captured.candidate_hash,
        HumanDecisionKind::Approved,
        None,
        2,
        None,
    )
    .expect("stale candidate");
    store.publish(&bytes).expect("store stale");
    let mut bound = decision.clone();
    bound.object_hash = object;
    bound.artefact_hash = hash;
    bound.summary = ArtefactSummary::HumanDecision {
        candidate: stale,
        diff_base: captured.candidate_hash,
        decision: HumanDecisionKind::Approved,
    };
    let mut bound_gate = gate.clone();
    bound_gate.decision = Some(ArtefactReference {
        id: bound.id,
        kind: bound.kind,
        artefact_hash: hash,
    });
    let bound_inputs = vec![
        candidate_input(&reference),
        AttemptArtefactInput {
            key: InputKey::parse("decision").expect("key"),
            artefact: ArtefactReference {
                id: bound.id,
                kind: bound.kind,
                artefact_hash: hash,
            },
        },
    ];
    other_candidate.artefacts.push(bound);
    other_candidate.gates.push(bound_gate);
    assert_eq!(
        require_commit_approval(&other_candidate, &step, &bound_inputs, &store).err(),
        Some(CommitError::Assurance)
    );

    let mut other_base = run.clone();
    let (bytes, object, hash) = payload::encode_human_decision(
        captured.candidate_hash,
        stale,
        HumanDecisionKind::Approved,
        None,
        2,
        None,
    )
    .expect("stale base");
    store.publish(&bytes).expect("store base");
    let mut base = decision;
    base.object_hash = object;
    base.artefact_hash = hash;
    base.summary = ArtefactSummary::HumanDecision {
        candidate: captured.candidate_hash,
        diff_base: stale,
        decision: HumanDecisionKind::Approved,
    };
    let mut base_gate = gate;
    base_gate.decision = Some(ArtefactReference {
        id: base.id,
        kind: base.kind,
        artefact_hash: hash,
    });
    let base_inputs = vec![
        candidate_input(&reference),
        AttemptArtefactInput {
            key: InputKey::parse("decision").expect("key"),
            artefact: ArtefactReference {
                id: base.id,
                kind: base.kind,
                artefact_hash: hash,
            },
        },
    ];
    other_base.artefacts.push(base);
    other_base.gates.push(base_gate);
    assert_eq!(
        require_commit_approval(&other_base, &step, &base_inputs, &store).err(),
        Some(CommitError::Assurance)
    );
}

#[test]
fn commit_rejects_plan_acceptance_as_code_assurance() {
    let store = store();
    let (run, _, reference, _, _dir) = captured_candidate(&store);
    let commit = commit_step(&run);
    let plan_decision = AttemptArtefactInput {
        key: InputKey::parse("decision").expect("key"),
        artefact: ArtefactReference {
            id: ArtefactId::generate().expect("id"),
            kind: ArtefactKind::PlanDecision,
            artefact_hash: artefact_hash_for(ArtefactKind::PlanDecision, 1, b"plan decision"),
        },
    };
    assert_eq!(
        require_commit_approval(
            &run,
            &commit,
            &[candidate_input(&reference), plan_decision],
            &store,
        )
        .err(),
        Some(CommitError::Assurance)
    );
}

#[test]
fn project_drift_stops_a_commit_before_host_application() {
    let store = store();
    let (_, _, _, initial, dir) = captured_candidate(&store);
    std::fs::write(dir.path().join("file.txt"), b"target").expect("target");
    let target = crate::workflows::artefacts::CandidateCapture::capture_host(dir.path(), &store)
        .expect("target");
    std::fs::write(dir.path().join("file.txt"), b"drifted").expect("drift");
    let before = std::fs::read(dir.path().join("file.txt")).expect("before");

    assert_eq!(
        require_unchanged_project(dir.path(), &initial, &target, &store),
        Err(CommitError::Preflight)
    );
    assert_eq!(
        std::fs::read(dir.path().join("file.txt")).expect("after"),
        before
    );
}

fn run_git(project: &std::path::Path, exec: crate::sandbox::GuestExec) -> String {
    use std::io::Write;
    use std::process::Stdio;

    assert_eq!(exec.program, "git");
    let mut command = std::process::Command::new(&exec.program);
    command.current_dir(project).args(&exec.args).envs(exec.env);
    if exec.stdin.is_some() {
        command.stdin(Stdio::piped());
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn git");
    if let Some(stdin) = exec.stdin {
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(&stdin)
            .expect("write stdin");
    }
    let output = child.wait_with_output().expect("git output");
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf8")
        .trim()
        .to_owned()
}

#[test]
fn fixed_plumbing_creates_the_exact_tree_without_touching_the_live_index() {
    let dir = tempfile::tempdir().expect("project");
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .expect("init")
            .success()
    );
    for name in ["old.txt", "keep.txt", "removed.txt"] {
        std::fs::write(dir.path().join(name), b"old").expect("old");
    }
    assert!(
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .status()
            .expect("add")
            .success()
    );
    let timestamp = utc_timestamp(1_700_000_000_000);
    let temporary_index = dir.path().join(".git/test-commit.index");
    let temporary_index_text = temporary_index.to_string_lossy();

    let tree = run_git(
        dir.path(),
        super::git_command(vec!["write-tree".to_owned()], None, &timestamp),
    );
    let parent = run_git(dir.path(), commit_tree_command(&tree, None, &timestamp));
    let store = store();
    let initial =
        crate::workflows::artefacts::CandidateCapture::capture_directory(dir.path(), &[], &store)
            .expect("initial");
    std::fs::remove_file(dir.path().join("old.txt")).expect("remove old file");
    std::fs::create_dir(dir.path().join("old.txt")).expect("replacement directory");
    std::fs::remove_file(dir.path().join("removed.txt")).expect("remove file");
    let target =
        crate::workflows::artefacts::CandidateCapture::capture_directory(dir.path(), &[], &store)
            .expect("target");
    let live_index = std::fs::read(dir.path().join(".git/index")).expect("live index");
    run_git(
        dir.path(),
        super::read_tree_command(&temporary_index_text, Some(&parent), &timestamp),
    );
    let regular = run_git(
        dir.path(),
        hash_object_command(b"regular\n".to_vec(), &timestamp),
    );
    let executable = run_git(
        dir.path(),
        hash_object_command(b"#!/bin/sh\n".to_vec(), &timestamp),
    );
    let mut index = super::index_removal_records(&initial, &target, GitObjectFormat::Sha1);
    for (path, mode, object) in [
        ("file.txt", "100644", regular.as_str()),
        ("script", "100755", executable.as_str()),
    ] {
        index.extend(format!("{mode} {object}\t{path}\0").into_bytes());
    }
    run_git(
        dir.path(),
        index_info_command(index, &temporary_index_text, &timestamp),
    );
    let tree = run_git(
        dir.path(),
        write_tree_command(&temporary_index_text, &timestamp),
    );
    let commit = run_git(
        dir.path(),
        commit_tree_command(&tree, Some(&parent), &timestamp),
    );

    assert!(parse_object_id(&tree, GitObjectFormat::Sha1).is_ok());
    assert!(parse_object_id(&commit, GitObjectFormat::Sha1).is_ok());
    assert_eq!(
        std::fs::read(dir.path().join(".git/index")).expect("unchanged index"),
        live_index
    );
    let committed_tree = std::process::Command::new("git")
        .args(["rev-parse", &format!("{commit}^{{tree}}")])
        .current_dir(dir.path())
        .output()
        .expect("tree");
    assert_eq!(
        String::from_utf8(committed_tree.stdout)
            .expect("tree id")
            .trim(),
        tree
    );
    let names = std::process::Command::new("git")
        .args(["ls-tree", "--name-only", &commit])
        .current_dir(dir.path())
        .output()
        .expect("tree names");
    assert_eq!(
        String::from_utf8(names.stdout).expect("names"),
        "file.txt\nkeep.txt\nscript\n"
    );
}
