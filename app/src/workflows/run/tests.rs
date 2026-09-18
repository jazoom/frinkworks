pub(super) fn next_ordinal_for(attempts: &[AttemptRecord], step: &StepKey) -> u32 {
    super::next_ordinal(attempts, step)
}

use crate::tests::test_environment_id;
impl WorkflowRun {
    pub(crate) fn configured(
        id: RunId,
        created_at_ms: u64,
        agent_id: super::AgentId,
        pinned: PinnedWorkflowDefinition,
        environments: super::ResolvedEnvironmentSet,
    ) -> Self {
        Self::create(
            id,
            created_at_ms,
            Some(agent_id),
            super::RunKind::Configured,
            pinned,
            environments,
        )
    }
}

use super::{
    ActionKind, AttemptCleanupRecord, AttemptId, AttemptRecord, AttemptResult, AttemptSandboxKind,
    AttemptSandboxRecord, AttemptState, EscalationReason, FailureCategory, ReviewRoute, RunState,
    TransitionError, WorkflowRun,
};
use crate::agents::{AccessMode, ToolId};
use crate::tests::{test_agent_capabilities, test_command_capabilities};
use crate::workflows::definition::{
    ASSISTANT_REPLY, AgentAuthority, AgentStep, ArtefactKind, ArtefactSource, CandidateAuthority,
    InputKey, OutputKey, OutputKind, PinnedWorkflowDefinition, RequiredInput, RequiredOutput,
    RoleDefinition, RoleKey, StepAction, StepDefinition, StepEnvironment, StepKey, SystemCommandId,
    SystemCommandStep, WorkflowDefinition, candidate_revision_output, initial_candidate_input,
};
use crate::workflows::id::RunId;

fn definition() -> WorkflowDefinition {
    two_step(false)
}

fn two_step(include_command: bool) -> WorkflowDefinition {
    let role = RoleDefinition::new(
        RoleKey::parse("agent").expect("role"),
        "Maintainer".to_owned(),
        String::new(),
        String::new(),
    )
    .expect("role");
    let authority = AgentAuthority::new(vec![ToolId::List], Vec::new()).expect("authority");
    let reply = StepDefinition {
        key: StepKey::parse("reply").expect("step"),
        name: "Reply".to_owned(),
        inputs: vec![initial_candidate_input()],
        action: StepAction::Agent(AgentStep {
            environment: StepEnvironment::WorkflowDefault,
            role: RoleKey::parse("agent").expect("role"),
            candidate_authority: CandidateAuthority::Edit,
            authority,
            settings: crate::workflows::definition::ModelStepSettings::SameAsRunDefaults,
            required_outputs: vec![
                RequiredOutput {
                    key: OutputKey::parse(ASSISTANT_REPLY).expect("output"),
                    kind: OutputKind::AssistantReply,
                },
                candidate_revision_output(),
            ],
        }),
        review: None,
    };
    let mut steps = vec![reply];
    if include_command {
        steps.push(StepDefinition {
            key: StepKey::parse("status").expect("step"),
            name: "Status".to_owned(),
            inputs: vec![RequiredInput {
                key: InputKey::parse("candidate").expect("input"),
                kind: ArtefactKind::CandidateRevision,
                source: ArtefactSource::StepOutput {
                    step: StepKey::parse("reply").expect("step"),
                    output: OutputKey::parse("candidate").expect("output"),
                },
            }],
            action: StepAction::SystemCommand(SystemCommandStep {
                environment: StepEnvironment::WorkflowDefault,
                command: SystemCommandId::RepositoryStatus,
                required_outputs: Vec::new(),
            }),
            review: None,
        });
    }
    WorkflowDefinition::from_parts(
        "Maintainer".to_owned(),
        test_environment_id(),
        vec![role],
        steps,
    )
    .expect("definition")
}

fn new_run() -> WorkflowRun {
    let definition = definition();
    let environments = crate::tests::test_environment_set(&definition);
    WorkflowRun::configured(
        RunId::generate().expect("run"),
        10,
        crate::agents::AgentId::generate().expect("agent"),
        PinnedWorkflowDefinition::pin(None, definition),
        environments,
    )
}

fn sandbox_record(run: &WorkflowRun) -> AttemptSandboxRecord {
    let digest = run.environments.steps[0].snapshot_digest.clone();
    AttemptSandboxRecord {
        kind: AttemptSandboxKind::IsolatedAttempt,
        snapshot_digest: digest,
    }
}

fn start(run: &mut WorkflowRun) -> AttemptId {
    let attempt = AttemptId::generate().expect("attempt");
    let step = run
        .pinned
        .definition
        .step(run.ready_step().expect("step"))
        .expect("step");
    let caps = match run.directory_settings() {
        Some(settings) => {
            let effective = run.phase_settings(&step.key).cloned().unwrap_or(settings);
            let authority = crate::execution::ProjectFreeAuthority::from_snapshot(1, &effective)
                .expect("authority");
            crate::workflows::capabilities::AttemptCapabilities::derive_project_free(
                step, &authority,
            )
            .expect("project-free capabilities")
        }
        None if matches!(
            step.action,
            crate::workflows::definition::StepAction::SystemCommand(_)
        ) =>
        {
            test_command_capabilities()
        }
        None => test_agent_capabilities(),
    };
    run.start_attempt(attempt, Vec::new(), caps, sandbox_record(run), 11)
        .expect("start");
    attempt
}

fn complete(run: &mut WorkflowRun, attempt: AttemptId, at_ms: u64) {
    run.record_cleanup(attempt, AttemptCleanupRecord::Complete)
        .expect("cleanup");
    run.complete_attempt(attempt, at_ms).expect("complete");
}

fn fail(run: &mut WorkflowRun, attempt: AttemptId, category: FailureCategory, at_ms: u64) {
    run.record_cleanup(attempt, AttemptCleanupRecord::Complete)
        .expect("cleanup");
    run.fail_attempt(attempt, category, at_ms).expect("fail");
}

#[test]
fn an_attempt_persists_the_exact_initial_context_packet() {
    let mut run = new_run();
    let attempt = start(&mut run);
    let packet_bytes = 4 + br#"{"role":"user","text":"Task"}"#.len() as u64;
    let total_bytes = packet_bytes
        + super::super::input_context::RESERVED_MODEL_OUTPUT_BYTES as u64
        + super::super::input_context::RESERVED_TOOL_WORK_BYTES as u64;
    let packet = super::super::input_context::AttemptContextPacket {
        prompt: "Task".to_owned(),
        messages: vec![super::super::input_context::ContextMessage::User(
            "Task".to_owned(),
        )],
        tools: Vec::new(),
        source_available: "Files through tools.".to_owned(),
        excluded_context: "Conversation excluded.".to_owned(),
        project_instructions: super::super::input_context::ProjectInstructionSnapshot {
            candidate: None,
            guest_path: "AGENTS.md".to_owned(),
            state: super::super::input_context::ProjectInstructionState::Absent,
        },
        budget: super::super::input_context::ContextBudget {
            packet_bytes,
            reserved_output_bytes: super::super::input_context::RESERVED_MODEL_OUTPUT_BYTES as u64,
            reserved_tool_bytes: super::super::input_context::RESERVED_TOOL_WORK_BYTES as u64,
            total_bytes,
            estimated_input_tokens: packet_bytes.div_ceil(4),
            estimated_total_tokens: total_bytes.div_ceil(4),
            model_context_limit: None,
        },
    };
    run.record_initial_context(attempt, packet.clone())
        .expect("context");
    let mut changed = packet.clone();
    changed.source_available = "Different source description".to_owned();
    assert_eq!(
        run.record_initial_context(attempt, changed),
        Err(TransitionError::Invalid)
    );
    assert_eq!(
        run.record_initial_context(
            AttemptId::generate().expect("stale attempt"),
            packet.clone()
        ),
        Err(TransitionError::Invalid)
    );
    let loaded = WorkflowRun::from_file(run.to_file()).expect("round trip");
    assert_eq!(loaded.attempts[0].initial_context, Some(packet));
}

#[test]
fn fixing_review_cross_run_and_cross_attempt_provenance_fails_load() {
    enum Corruption {
        CandidateRun,
        ReportRun,
        CandidateAttempt,
        ReportAttempt,
    }

    for corruption in [
        Corruption::CandidateRun,
        Corruption::ReportRun,
        Corruption::CandidateAttempt,
        Corruption::ReportAttempt,
    ] {
        let run = completed_fixing_review_run();
        let mut file = run.to_file();
        let candidate = file
            .artefacts
            .iter()
            .position(|record| {
                matches!(record.summary, super::SummaryFile::Candidate { .. })
                    && matches!(
                        record.provenance.producer,
                        super::ProducerFile::StepAttempt { .. }
                    )
            })
            .expect("candidate");
        let report = file
            .artefacts
            .iter()
            .position(|record| matches!(record.summary, super::SummaryFile::Review { .. }))
            .expect("report");
        let record = match corruption {
            Corruption::CandidateRun | Corruption::CandidateAttempt => {
                &mut file.artefacts[candidate]
            }
            Corruption::ReportRun | Corruption::ReportAttempt => &mut file.artefacts[report],
        };
        match corruption {
            Corruption::CandidateRun | Corruption::ReportRun => {
                record.provenance.run_id = "a".repeat(32);
            }
            Corruption::CandidateAttempt | Corruption::ReportAttempt => {
                let super::ProducerFile::StepAttempt { attempt_id, .. } =
                    &mut record.provenance.producer
                else {
                    panic!("step producer")
                };
                *attempt_id = "b".repeat(32);
            }
        }
        assert_eq!(
            WorkflowRun::from_file(file).err(),
            Some(super::RunRecordError::Corrupt)
        );
    }
}

fn completed_fixing_review_run() -> WorkflowRun {
    let role_key = RoleKey::parse("reviewer").expect("role");
    let step = StepDefinition {
        key: StepKey::parse("fixing-reviewer").expect("step"),
        name: "Fixing reviewer".to_owned(),
        inputs: vec![initial_candidate_input()],
        action: StepAction::Agent(AgentStep {
            environment: StepEnvironment::WorkflowDefault,
            role: role_key.clone(),
            candidate_authority: CandidateAuthority::Edit,
            authority: AgentAuthority::new(vec![ToolId::List], Vec::new()).expect("authority"),
            settings: crate::workflows::definition::ModelStepSettings::SameAsRunDefaults,
            required_outputs: vec![
                RequiredOutput {
                    key: OutputKey::parse(ASSISTANT_REPLY).expect("reply"),
                    kind: OutputKind::AssistantReply,
                },
                candidate_revision_output(),
                RequiredOutput {
                    key: OutputKey::parse("review").expect("review"),
                    kind: OutputKind::ReviewReport,
                },
            ],
        }),
        review: None,
    };
    let definition = WorkflowDefinition::from_parts(
        "Fixing review".to_owned(),
        test_environment_id(),
        vec![
            RoleDefinition::new(
                role_key,
                "Reviewer".to_owned(),
                String::new(),
                String::new(),
            )
            .expect("role"),
        ],
        vec![step],
    )
    .expect("definition");
    let environments = crate::tests::test_environment_set(&definition);
    let mut run = WorkflowRun::configured(
        RunId::generate().expect("run"),
        10,
        crate::agents::AgentId::generate().expect("agent"),
        PinnedWorkflowDefinition::pin(None, definition),
        environments,
    );
    let initial = test_artefact_record(
        run.id,
        ArtefactKind::CandidateRevision,
        crate::workflows::artefacts::ArtefactProducer::RunSourceCapture,
        Vec::new(),
    );
    let initial_reference = crate::workflows::artefacts::ArtefactReference {
        id: initial.id,
        kind: initial.kind,
        artefact_hash: initial.artefact_hash,
    };
    run.record_initial_candidate(initial).expect("initial");
    let attempt = AttemptId::generate().expect("attempt");
    let inputs = vec![super::AttemptArtefactInput {
        key: InputKey::parse("candidate").expect("input"),
        artefact: initial_reference,
    }];
    run.start_attempt(
        attempt,
        inputs.clone(),
        test_agent_capabilities(),
        sandbox_record(&run),
        11,
    )
    .expect("start");
    let producer = |output: &str| crate::workflows::artefacts::ArtefactProducer::StepAttempt {
        attempt_id: attempt,
        step: StepKey::parse("fixing-reviewer").expect("step"),
        output: Some(OutputKey::parse(output).expect("output")),
        disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
    };
    let candidate = test_artefact_record(
        run.id,
        ArtefactKind::CandidateRevision,
        producer("candidate"),
        inputs.iter().map(|input| input.artefact.clone()).collect(),
    );
    let candidate_reference = crate::workflows::artefacts::ArtefactReference {
        id: candidate.id,
        kind: candidate.kind,
        artefact_hash: candidate.artefact_hash,
    };
    let report = test_artefact_record(
        run.id,
        ArtefactKind::ReviewReport,
        producer("review"),
        vec![candidate_reference.clone()],
    );
    let report_reference = crate::workflows::artefacts::ArtefactReference {
        id: report.id,
        kind: report.kind,
        artefact_hash: report.artefact_hash,
    };
    run.record_attempt_outputs(
        attempt,
        vec![candidate, report],
        vec![
            super::AttemptArtefactOutput {
                key: OutputKey::parse("candidate").expect("output"),
                artefact: candidate_reference.clone(),
            },
            super::AttemptArtefactOutput {
                key: OutputKey::parse("review").expect("output"),
                artefact: report_reference,
            },
        ],
        Some(candidate_reference.clone()),
        super::ObservedCandidate::Exact {
            artefact: candidate_reference,
        },
    )
    .expect("outputs");
    run.record_cleanup(attempt, AttemptCleanupRecord::Complete)
        .expect("cleanup");
    run.complete_attempt(attempt, 12).expect("complete");
    run
}

fn test_artefact_record(
    run_id: RunId,
    kind: ArtefactKind,
    producer: crate::workflows::artefacts::ArtefactProducer,
    inputs: Vec<crate::workflows::artefacts::ArtefactReference>,
) -> crate::workflows::artefacts::ArtefactRecord {
    let candidate = crate::workflows::artefacts::CandidateHash::of(b"candidate");
    crate::workflows::artefacts::ArtefactRecord {
        id: crate::workflows::ArtefactId::generate().expect("artefact"),
        kind,
        artefact_hash: crate::workflows::artefacts::ArtefactHash::of(
            b"test",
            kind.as_str().as_bytes(),
        ),
        object_hash: crate::workflows::artefacts::ObjectHash::of(kind.as_str().as_bytes()),
        payload_bytes: 1,
        created_at_ms: 11,
        provenance: crate::workflows::artefacts::ArtefactProvenance {
            run_id,
            producer,
            inputs,
        },
        summary: match kind {
            ArtefactKind::CandidateRevision => {
                crate::workflows::artefacts::ArtefactSummary::Candidate {
                    candidate,
                    entries: 0,
                    bytes: 0,
                    disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
                }
            }
            ArtefactKind::ReviewReport => crate::workflows::artefacts::ArtefactSummary::Review {
                candidate,
                verdict: crate::workflows::artefacts::ReviewVerdict::Approved,
            },
            _ => panic!("test artefact kind"),
        },
    }
}

#[test]
fn creation_stores_ready_without_attempts() {
    let run = new_run();
    assert!(matches!(run.state, RunState::Ready { .. }));
    assert!(run.attempts.is_empty());
    assert!(run.gates.is_empty());
}

#[test]
fn parallel_attempts_are_rejected() {
    let mut run = new_run();
    start(&mut run);
    let second = AttemptId::generate().expect("attempt");
    assert_eq!(
        run.start_attempt(
            second,
            Vec::new(),
            test_agent_capabilities(),
            sandbox_record(&run),
            12,
        ),
        Err(TransitionError::Invalid)
    );
    assert_eq!(run.attempts.len(), 1);
    assert!(matches!(run.state, RunState::Active { .. }));
}

#[test]
fn stale_completions_are_rejected() {
    let mut run = new_run();
    start(&mut run);
    let stale = AttemptId::generate().expect("attempt");
    assert_eq!(
        run.complete_attempt(stale, 12),
        Err(TransitionError::Invalid)
    );
    assert!(matches!(run.state, RunState::Active { .. }));
}

#[test]
fn transition_times_cannot_move_backwards() {
    let mut run = new_run();
    let attempt = start(&mut run);
    assert_eq!(
        run.fail_attempt(attempt, FailureCategory::Provider, 10),
        Err(TransitionError::Invalid)
    );
    assert!(matches!(run.state, RunState::Active { .. }));
}

#[test]
fn duplicate_terminal_results_are_rejected() {
    let mut run = new_run();
    let attempt = start(&mut run);
    complete(&mut run, attempt, 12);
    assert_eq!(
        run.complete_attempt(attempt, 13),
        Err(TransitionError::Invalid)
    );
    assert_eq!(
        run.fail_attempt(attempt, FailureCategory::Provider, 13),
        Err(TransitionError::Invalid)
    );
}

#[test]
fn mutations_after_a_terminal_state_are_rejected() {
    let mut run = new_run();
    let attempt = start(&mut run);
    fail(&mut run, attempt, FailureCategory::Provider, 12);
    assert_eq!(run.cancel(13), Err(TransitionError::Invalid));
    assert_eq!(
        run.start_attempt(
            AttemptId::generate().expect("attempt"),
            Vec::new(),
            test_agent_capabilities(),
            sandbox_record(&run),
            13,
        ),
        Err(TransitionError::Invalid)
    );
    assert_eq!(run.interrupt(13), Err(TransitionError::Invalid));
}

#[test]
fn completed_attempts_advance_by_vector_position() {
    let definition = two_step(true);
    let environments = crate::tests::test_environment_set(&definition);
    let mut run = WorkflowRun::configured(
        RunId::generate().expect("run"),
        10,
        crate::agents::AgentId::generate().expect("agent"),
        PinnedWorkflowDefinition::pin(None, definition),
        environments,
    );
    let first = AttemptId::generate().expect("attempt");
    run.start_attempt(
        first,
        Vec::new(),
        test_agent_capabilities(),
        sandbox_record(&run),
        11,
    )
    .expect("start");
    complete(&mut run, first, 12);
    assert!(matches!(run.state, RunState::Ready { ref step } if step.as_str() == "status"));
    let second = AttemptId::generate().expect("attempt");
    run.start_attempt(
        second,
        Vec::new(),
        test_command_capabilities(),
        sandbox_record(&run),
        13,
    )
    .expect("start");
    complete(&mut run, second, 14);
    assert_eq!(run.state, RunState::Completed);
}

#[test]
fn failed_attempts_move_the_run_to_failed() {
    let mut run = new_run();
    let attempt = start(&mut run);
    fail(&mut run, attempt, FailureCategory::Command, 12);
    assert_eq!(run.state, RunState::Failed);
}

#[test]
fn cancellation_moves_the_run_directly_to_cancelled() {
    let mut run = new_run();
    let attempt = start(&mut run);
    run.record_cleanup(attempt, AttemptCleanupRecord::Complete)
        .expect("cleanup");
    run.cancel(12).expect("cancel");
    assert_eq!(run.state, RunState::Cancelled);
    assert_eq!(run.attempts[0].state, AttemptState::Cancelled);
}

#[test]
fn attempt_ordinals_count_repeated_step_attempts() {
    let step = StepKey::parse("reply").expect("step");
    let first = AttemptRecord {
        id: AttemptId::generate().expect("attempt"),
        step: step.clone(),
        ordinal: 1,
        action_kind: ActionKind::Agent,
        started_at_ms: 1,
        finished_at_ms: Some(2),
        state: AttemptState::Failed,
        result: Some(AttemptResult::Failed {
            category: FailureCategory::Provider,
        }),
        review_route: None,
        inputs: Vec::new(),
        outputs: Vec::new(),
        capabilities: test_agent_capabilities(),
        sandbox: AttemptSandboxRecord {
            kind: AttemptSandboxKind::IsolatedAttempt,
            snapshot_digest: crate::environments::SnapshotDigest::parse(&format!(
                "sha256:{}",
                "a".repeat(64)
            ))
            .expect("digest"),
        },
        cleanup: AttemptCleanupRecord::Complete,
        initial_context: None,
        apply_transaction: None,
        commit_transaction: None,
        commit_result: None,
        direct_changes: None,
    };
    let second = AttemptRecord {
        id: AttemptId::generate().expect("attempt"),
        step: step.clone(),
        ordinal: 2,
        action_kind: ActionKind::Agent,
        started_at_ms: 3,
        finished_at_ms: None,
        state: AttemptState::Active,
        result: None,
        review_route: None,
        inputs: Vec::new(),
        outputs: Vec::new(),
        capabilities: test_agent_capabilities(),
        sandbox: AttemptSandboxRecord {
            kind: AttemptSandboxKind::IsolatedAttempt,
            snapshot_digest: crate::environments::SnapshotDigest::parse(&format!(
                "sha256:{}",
                "a".repeat(64)
            ))
            .expect("digest"),
        },
        cleanup: AttemptCleanupRecord::Pending,
        initial_context: None,
        apply_transaction: None,
        commit_transaction: None,
        commit_result: None,
        direct_changes: None,
    };
    assert_eq!(next_ordinal_for(&[], &step), 1);
    assert_eq!(next_ordinal_for(std::slice::from_ref(&first), &step), 2);
    assert_eq!(next_ordinal_for(&[first, second], &step), 3);
}

fn artefact_reference(
    record: &crate::workflows::artefacts::ArtefactRecord,
) -> crate::workflows::artefacts::ArtefactReference {
    crate::workflows::artefacts::ArtefactReference {
        id: record.id,
        kind: record.kind,
        artefact_hash: record.artefact_hash,
    }
}

fn review_loop_run() -> WorkflowRun {
    review_run(crate::workflows::seeds::review_until_approved_definition(
        test_environment_id(),
    ))
}

fn review_run(definition: WorkflowDefinition) -> WorkflowRun {
    let environments = crate::tests::test_environment_set(&definition);
    let mut run = WorkflowRun::configured(
        RunId::generate().expect("run"),
        10,
        crate::agents::AgentId::generate().expect("agent"),
        PinnedWorkflowDefinition::pin(None, definition),
        environments,
    );
    let initial = test_artefact_record(
        run.id,
        ArtefactKind::CandidateRevision,
        crate::workflows::artefacts::ArtefactProducer::RunSourceCapture,
        Vec::new(),
    );
    run.record_initial_candidate(initial).expect("initial");
    run
}

fn step_sandbox(run: &WorkflowRun, step: &StepKey) -> AttemptSandboxRecord {
    AttemptSandboxRecord {
        kind: AttemptSandboxKind::IsolatedAttempt,
        snapshot_digest: run
            .environments
            .steps
            .iter()
            .find(|binding| &binding.step == step)
            .expect("step environment")
            .snapshot_digest
            .clone(),
    }
}

fn current_candidate(run: &WorkflowRun) -> crate::workflows::artefacts::ArtefactReference {
    let super::RunSource::Captured { source } = &run.source else {
        panic!("captured source")
    };
    source.accepted.clone()
}

fn complete_implementation(
    run: &mut WorkflowRun,
    at_ms: u64,
) -> (AttemptId, crate::workflows::artefacts::ArtefactReference) {
    let step = StepKey::parse("implementer").expect("step");
    assert_eq!(run.ready_step(), Some(&step));
    let input = current_candidate(run);
    let inputs = vec![super::AttemptArtefactInput {
        key: InputKey::parse("candidate").expect("input"),
        artefact: input.clone(),
    }];
    let attempt = AttemptId::generate().expect("attempt");
    run.start_attempt(
        attempt,
        inputs.clone(),
        test_agent_capabilities(),
        step_sandbox(run, &step),
        at_ms,
    )
    .expect("start implementation");
    let candidate = test_artefact_record(
        run.id,
        ArtefactKind::CandidateRevision,
        crate::workflows::artefacts::ArtefactProducer::StepAttempt {
            attempt_id: attempt,
            step,
            output: Some(OutputKey::parse("candidate").expect("output")),
            disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
        },
        vec![input],
    );
    let reference = artefact_reference(&candidate);
    run.record_attempt_outputs(
        attempt,
        vec![candidate],
        vec![super::AttemptArtefactOutput {
            key: OutputKey::parse("candidate").expect("output"),
            artefact: reference.clone(),
        }],
        Some(reference.clone()),
        super::ObservedCandidate::Exact {
            artefact: reference.clone(),
        },
    )
    .expect("implementation output");
    run.record_cleanup(attempt, AttemptCleanupRecord::Complete)
        .expect("cleanup");
    run.complete_attempt(attempt, at_ms + 1)
        .expect("complete implementation");
    (attempt, reference)
}

fn complete_review(
    run: &mut WorkflowRun,
    verdict: crate::workflows::artefacts::ReviewVerdict,
    at_ms: u64,
) -> crate::workflows::artefacts::ArtefactReference {
    let step = StepKey::parse("reviewer").expect("step");
    assert_eq!(run.ready_step(), Some(&step));
    let input = current_candidate(run);
    let inputs = vec![super::AttemptArtefactInput {
        key: InputKey::parse("candidate").expect("input"),
        artefact: input.clone(),
    }];
    let attempt = AttemptId::generate().expect("attempt");
    let mut capabilities = test_agent_capabilities();
    capabilities.directories[0].access = crate::agents::AccessMode::ReadOnly;
    run.start_attempt(
        attempt,
        inputs,
        capabilities,
        step_sandbox(run, &step),
        at_ms,
    )
    .expect("start review");
    let mut report = test_artefact_record(
        run.id,
        ArtefactKind::ReviewReport,
        crate::workflows::artefacts::ArtefactProducer::StepAttempt {
            attempt_id: attempt,
            step,
            output: Some(OutputKey::parse("review").expect("output")),
            disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
        },
        vec![input.clone()],
    );
    let crate::workflows::artefacts::ArtefactSummary::Review {
        verdict: report_verdict,
        ..
    } = &mut report.summary
    else {
        panic!("review summary")
    };
    *report_verdict = verdict;
    let reference = artefact_reference(&report);
    run.record_attempt_outputs(
        attempt,
        vec![report],
        vec![super::AttemptArtefactOutput {
            key: OutputKey::parse("review").expect("output"),
            artefact: reference.clone(),
        }],
        None,
        super::ObservedCandidate::Exact { artefact: input },
    )
    .expect("review output");
    run.record_cleanup(attempt, AttemptCleanupRecord::Complete)
        .expect("cleanup");
    run.complete_attempt(attempt, at_ms + 1)
        .expect("complete review");
    reference
}

#[test]
fn review_verdicts_select_all_four_routes() {
    use crate::workflows::artefacts::ReviewVerdict;

    enum ExpectedRoute {
        Approved,
        Revision,
        Blocked,
        AttemptLimit,
    }

    for (verdict, prior_revisions, expected) in [
        (ReviewVerdict::Approved, 0, ExpectedRoute::Approved),
        (ReviewVerdict::RevisionRequired, 0, ExpectedRoute::Revision),
        (ReviewVerdict::Blocked, 0, ExpectedRoute::Blocked),
        (
            ReviewVerdict::RevisionRequired,
            2,
            ExpectedRoute::AttemptLimit,
        ),
    ] {
        let mut run = review_loop_run();
        let mut time = 11;
        complete_implementation(&mut run, time);
        time += 2;
        for _ in 0..prior_revisions {
            complete_review(&mut run, ReviewVerdict::RevisionRequired, time);
            time += 2;
            complete_implementation(&mut run, time);
            time += 2;
        }
        let report = complete_review(&mut run, verdict, time);
        match expected {
            ExpectedRoute::Approved => {
                assert!(
                    matches!(&run.state, RunState::Ready { step } if step.as_str() == "commit")
                );
                assert_eq!(
                    run.attempts.last().and_then(|attempt| attempt.review_route),
                    Some(ReviewRoute::Approved)
                );
            }
            ExpectedRoute::Revision => {
                assert!(
                    matches!(&run.state, RunState::Ready { step } if step.as_str() == "implementer")
                );
                assert_eq!(
                    run.attempts.last().and_then(|attempt| attempt.review_route),
                    Some(ReviewRoute::RevisionRequested)
                );
            }
            ExpectedRoute::Blocked => {
                assert_eq!(
                    run.state,
                    RunState::Escalated {
                        step: StepKey::parse("reviewer").expect("step"),
                        report,
                        reason: EscalationReason::Blocked,
                    }
                );
                assert_eq!(
                    run.attempts.last().and_then(|attempt| attempt.review_route),
                    Some(ReviewRoute::BlockedEscalation)
                );
            }
            ExpectedRoute::AttemptLimit => {
                assert_eq!(
                    run.state,
                    RunState::Escalated {
                        step: StepKey::parse("reviewer").expect("step"),
                        report,
                        reason: EscalationReason::AttemptLimit,
                    }
                );
                assert_eq!(
                    run.attempts.last().and_then(|attempt| attempt.review_route),
                    Some(ReviewRoute::AttemptLimitEscalation)
                );
            }
        }
    }
}

#[test]
fn final_review_approval_completes_the_run() {
    let source = crate::workflows::seeds::review_until_approved_definition(test_environment_id());
    let definition = WorkflowDefinition::from_parts(
        source.name().to_owned(),
        source.default_environment(),
        source.roles().to_vec(),
        source.steps()[..2].to_vec(),
    )
    .expect("final review definition");
    let mut run = review_run(definition);

    complete_implementation(&mut run, 11);
    complete_review(
        &mut run,
        crate::workflows::artefacts::ReviewVerdict::Approved,
        13,
    );

    assert_eq!(run.state, RunState::Completed);
    assert_eq!(
        run.attempts.last().and_then(|attempt| attempt.review_route),
        Some(ReviewRoute::Approved)
    );
}

#[test]
fn repeated_steps_pin_the_current_candidate_and_increment_ordinals() {
    let mut run = review_loop_run();
    let (first_attempt, first_candidate) = complete_implementation(&mut run, 11);
    complete_review(
        &mut run,
        crate::workflows::artefacts::ReviewVerdict::RevisionRequired,
        13,
    );
    let input_candidate = current_candidate(&run);
    let (second_attempt, second_candidate) = complete_implementation(&mut run, 15);
    let repeated = run
        .attempts
        .iter()
        .find(|attempt| attempt.id == second_attempt)
        .expect("repeated attempt");

    assert_eq!(repeated.ordinal, 2);
    assert_eq!(repeated.inputs[0].artefact, input_candidate);
    assert_eq!(input_candidate, first_candidate);
    assert_ne!(second_candidate.id, first_candidate.id);
    assert_ne!(second_attempt, first_attempt);
}

#[test]
fn durable_review_routes_reject_altered_route_and_escalation_facts() {
    let mut approved = review_loop_run();
    complete_implementation(&mut approved, 11);
    complete_review(
        &mut approved,
        crate::workflows::artefacts::ReviewVerdict::Approved,
        13,
    );
    let mut altered_route = approved.to_file();
    let review = altered_route
        .attempts
        .iter_mut()
        .rev()
        .find(|attempt| attempt.review_route.is_some())
        .expect("review attempt");
    review.review_route = Some("revision-requested".to_owned());
    assert_eq!(
        WorkflowRun::from_file(altered_route).err(),
        Some(super::RunRecordError::Corrupt)
    );

    let mut altered_report_reference = approved.to_file();
    let review = altered_report_reference
        .attempts
        .iter_mut()
        .rev()
        .find(|attempt| attempt.review_route.is_some())
        .expect("review attempt");
    review
        .outputs
        .iter_mut()
        .find(|output| output.key == "review")
        .expect("review output")
        .artefact
        .artefact_hash = "0".repeat(64);
    assert_eq!(
        WorkflowRun::from_file(altered_report_reference).err(),
        Some(super::RunRecordError::Corrupt)
    );

    let mut blocked = review_loop_run();
    complete_implementation(&mut blocked, 11);
    complete_review(
        &mut blocked,
        crate::workflows::artefacts::ReviewVerdict::Blocked,
        13,
    );
    let mut altered_escalation = blocked.to_file();
    let super::RunStateFile::Escalated { reason, .. } = &mut altered_escalation.state else {
        panic!("escalated state")
    };
    *reason = "attempt-limit".to_owned();
    assert_eq!(
        WorkflowRun::from_file(altered_escalation).err(),
        Some(super::RunRecordError::Corrupt)
    );
}

fn reference_for(
    kind: ArtefactKind,
    marker: &[u8],
) -> crate::workflows::artefacts::ArtefactReference {
    crate::workflows::artefacts::ArtefactReference {
        id: crate::workflows::ArtefactId::generate().expect("artefact"),
        kind,
        artefact_hash: crate::workflows::artefacts::ArtefactHash::of(
            marker,
            kind.as_str().as_bytes(),
        ),
    }
}

#[test]
fn durable_commit_transactions_preserve_every_review_reference() {
    let candidate = reference_for(ArtefactKind::CandidateRevision, b"candidate");
    let first_review = reference_for(ArtefactKind::ReviewReport, b"first");
    let second_review = reference_for(ArtefactKind::ReviewReport, b"second");
    let attempt = AttemptRecord {
        id: AttemptId::generate().expect("attempt"),
        step: StepKey::parse("commit").expect("step"),
        ordinal: 1,
        action_kind: ActionKind::SystemCommand,
        started_at_ms: 1,
        finished_at_ms: None,
        state: AttemptState::Active,
        result: None,
        review_route: None,
        inputs: vec![
            super::AttemptArtefactInput {
                key: InputKey::parse("candidate").expect("input"),
                artefact: candidate.clone(),
            },
            super::AttemptArtefactInput {
                key: InputKey::parse("correctness").expect("input"),
                artefact: first_review.clone(),
            },
            super::AttemptArtefactInput {
                key: InputKey::parse("security").expect("input"),
                artefact: second_review.clone(),
            },
        ],
        outputs: Vec::new(),
        capabilities: test_command_capabilities(),
        sandbox: AttemptSandboxRecord {
            kind: AttemptSandboxKind::IsolatedAttempt,
            snapshot_digest: crate::environments::SnapshotDigest::parse(&format!(
                "sha256:{}",
                "a".repeat(64)
            ))
            .expect("digest"),
        },
        cleanup: AttemptCleanupRecord::Pending,
        initial_context: None,
        apply_transaction: None,
        commit_transaction: None,
        commit_result: None,
        direct_changes: None,
    };
    let transaction = crate::workflows::commit::CommitTransaction {
        state: crate::workflows::commit::CommitTransactionState::Prepared,
        candidate,
        reviews: vec![first_review.clone(), second_review.clone()],
        approval: None,
        expected_reference: "refs/heads/main".to_owned(),
        old_object: None,
        target_tree: None,
        expected_commit: None,
        timestamp: "1700000000 +0000".to_owned(),
    };
    assert!(super::valid_commit_transaction(&attempt, &transaction));

    let mut missing = transaction.clone();
    missing.reviews.pop();
    assert!(!super::valid_commit_transaction(&attempt, &missing));

    let mut duplicate = transaction;
    duplicate.reviews = vec![first_review.clone(), first_review];
    assert!(!super::valid_commit_transaction(&attempt, &duplicate));
}

#[test]
fn durable_commit_transactions_record_an_approved_human_decision() {
    let candidate = reference_for(ArtefactKind::CandidateRevision, b"candidate");
    let approval = reference_for(ArtefactKind::HumanDecision, b"decision");
    let attempt = AttemptRecord {
        id: AttemptId::generate().expect("attempt"),
        step: StepKey::parse("commit").expect("step"),
        ordinal: 1,
        action_kind: ActionKind::SystemCommand,
        started_at_ms: 1,
        finished_at_ms: None,
        state: AttemptState::Active,
        result: None,
        review_route: None,
        inputs: vec![
            super::AttemptArtefactInput {
                key: InputKey::parse("candidate").expect("input"),
                artefact: candidate.clone(),
            },
            super::AttemptArtefactInput {
                key: InputKey::parse("decision").expect("input"),
                artefact: approval.clone(),
            },
        ],
        outputs: Vec::new(),
        capabilities: test_command_capabilities(),
        sandbox: AttemptSandboxRecord {
            kind: AttemptSandboxKind::IsolatedAttempt,
            snapshot_digest: crate::environments::SnapshotDigest::parse(&format!(
                "sha256:{}",
                "a".repeat(64)
            ))
            .expect("digest"),
        },
        cleanup: AttemptCleanupRecord::Pending,
        initial_context: None,
        apply_transaction: None,
        commit_transaction: None,
        commit_result: None,
        direct_changes: None,
    };
    let transaction = crate::workflows::commit::CommitTransaction {
        state: crate::workflows::commit::CommitTransactionState::Prepared,
        candidate,
        reviews: Vec::new(),
        approval: Some(approval.clone()),
        expected_reference: "refs/heads/main".to_owned(),
        old_object: None,
        target_tree: None,
        expected_commit: None,
        timestamp: "1700000000 +0000".to_owned(),
    };
    assert!(super::valid_commit_transaction(&attempt, &transaction));

    let mut missing = transaction.clone();
    missing.approval = None;
    assert!(!super::valid_commit_transaction(&attempt, &missing));

    let mut other = transaction;
    other.approval = Some(reference_for(ArtefactKind::HumanDecision, b"other"));
    assert!(!super::valid_commit_transaction(&attempt, &other));
}

#[test]
fn configured_conversation_records_reject_missing_phase_selections() {
    let mut run = new_run();
    run.conversation_id =
        Some(crate::conversations::ConversationId::generate().expect("conversation"));
    assert_eq!(
        WorkflowRun::from_file(run.to_file()),
        Err(super::RunRecordError::Corrupt)
    );
}

#[test]
fn run_records_round_trip() {
    let mut run = new_run();
    let phase = run.pinned.definition.first_step().clone();
    let selection = crate::providers::ModelSelection::new(
        crate::providers::ProviderKind::Xai,
        "grok-4.6".to_owned(),
        None,
    )
    .expect("selection");
    let directory = tempfile::tempdir().expect("directory");
    let mut grant =
        crate::execution::DirectoryGrant::from_selected(directory.path(), &[]).expect("grant");
    grant.access = crate::execution::DirectoryAccess::ReviewBeforeApply;
    let settings = crate::execution::ExecutionSettings::new(
        selection.clone(),
        "Pinned instructions".to_owned(),
        vec![ToolId::List],
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_directories(vec![grant.clone()])
    .unwrap()
    .with_git_destination(Some(grant.id))
    .unwrap();
    run.pinned = PinnedWorkflowDefinition::pin(
        None,
        run.pinned
            .definition
            .with_conversation_settings(&settings)
            .expect("settings"),
    );
    run.phase_models = vec![crate::workflows::PhaseModelSelection {
        step: phase,
        selection,
        instructions: "Pinned instructions".to_owned(),
        preset: Some(crate::workflows::PinnedPreset {
            id: crate::presets::PresetId::generate().expect("preset"),
            revision: 3,
            name: "Pinned preset".to_owned(),
        }),
        settings: Some(settings),
    }];
    let attempt = start(&mut run);
    complete(&mut run, attempt, 12);
    let loaded = WorkflowRun::from_file(run.to_file()).expect("round trip");
    assert_eq!(loaded, run);
    assert_eq!(loaded.attempts[0].review_route, None);
    let mut altered = run.clone();
    altered.phase_models[0]
        .settings
        .as_mut()
        .unwrap()
        .environment = crate::environments::EnvironmentId::generate().unwrap();
    assert_eq!(
        WorkflowRun::from_file(altered.to_file()),
        Err(super::RunRecordError::Corrupt)
    );

    let mut reviewed = review_loop_run();
    complete_implementation(&mut reviewed, 11);
    complete_review(
        &mut reviewed,
        crate::workflows::artefacts::ReviewVerdict::Approved,
        13,
    );
    let loaded = WorkflowRun::from_file(reviewed.to_file()).expect("review round trip");
    assert_eq!(loaded, reviewed);
    assert_eq!(
        loaded
            .attempts
            .last()
            .and_then(|attempt| attempt.review_route),
        Some(ReviewRoute::Approved)
    );
}

#[test]
fn run_json_omits_the_transition_array() {
    let mut run = new_run();
    let attempt = start(&mut run);
    complete(&mut run, attempt, 12);
    let value = serde_json::to_value(run.to_file()).expect("json");
    assert_eq!(value["record-version"], 1);
    assert_eq!(value["kind"], "configured");
    assert!(value.get("transitions").is_none());
    assert!(value["attempts"][0]["review-route"].is_null());
}

#[test]
fn obsolete_run_record_versions_are_rejected() {
    for version in [0, 2] {
        let mut file = new_run().to_file();
        file.record_version = version;
        assert_eq!(
            WorkflowRun::from_file(file).err(),
            Some(super::RunRecordError::Corrupt)
        );
    }
}

#[test]
fn run_kind_round_trips_without_a_project_catalogue() {
    for kind in [super::RunKind::Configured, super::RunKind::QuickTask] {
        let definition = definition();
        let environments = crate::tests::test_environment_set(&definition);
        let run = WorkflowRun::create(
            RunId::generate().expect("run"),
            10,
            Some(crate::agents::AgentId::generate().expect("agent")),
            kind,
            PinnedWorkflowDefinition::pin(None, definition),
            environments,
        );
        let loaded = WorkflowRun::from_file(run.to_file()).expect("load");
        assert_eq!(loaded.kind, kind);
        assert!(loaded.conversation_id.is_none());
        assert!(loaded.agent_id.is_some());
    }
}

#[test]
fn source_free_records_reject_source_and_identity_substitution() {
    let environment = crate::environments::EnvironmentId::generate().expect("environment");
    let pinned = crate::workflows::pin_project_free_quick_task_with_directories(
        &[crate::agents::ToolId::Run],
        "Use private scratch files.",
        environment,
        Vec::new(),
        false,
    )
    .expect("source-free quick task");
    let run = WorkflowRun::create_source_free_for_conversation(
        RunId::generate().expect("run"),
        10,
        crate::conversations::ConversationId::generate().expect("conversation"),
        pinned.clone(),
        crate::tests::test_environment_set(&pinned.definition),
        Vec::new(),
    );

    let loaded = WorkflowRun::from_file(run.to_file()).expect("load source-free run");
    assert!(loaded.agent_id.is_none());
    assert!(matches!(loaded.source, super::RunSource::None));
    assert!(loaded.artefacts.is_empty());
    for tamper in [
        |run: &mut WorkflowRun| run.agent_id = Some(super::AgentId::generate().unwrap()),
        |run: &mut WorkflowRun| run.conversation_id = None,
        |run: &mut WorkflowRun| run.source = super::RunSource::Pending,
    ] {
        let mut changed = run.clone();
        tamper(&mut changed);
        assert_eq!(
            WorkflowRun::from_file(changed.to_file()).err(),
            Some(super::RunRecordError::Corrupt)
        );
    }
}

#[test]
fn unknown_run_kind_fails_load() {
    let mut file = new_run().to_file();
    file.kind = "chat".to_owned();
    assert_eq!(
        WorkflowRun::from_file(file).err(),
        Some(super::RunRecordError::Corrupt)
    );
}

#[test]
fn restricted_network_capabilities_round_trip_and_reject_invalid_domains() {
    let mut capabilities = test_agent_capabilities();
    capabilities.network =
        super::NetworkCapability::Restricted(vec!["npmjs.org".to_owned(), "github.com".to_owned()]);
    let file = super::capabilities_to_file(&capabilities);
    assert_eq!(
        super::capabilities_from_file(file)
            .expect("capabilities")
            .network,
        capabilities.network
    );

    let mut file = super::capabilities_to_file(&capabilities);
    file.network_domains = vec!["com".to_owned()];
    assert_eq!(
        super::capabilities_from_file(file).err(),
        Some(super::RunRecordError::Corrupt)
    );

    let mut file = super::capabilities_to_file(&test_agent_capabilities());
    file.network = "provider-host".to_owned();
    assert_eq!(
        super::capabilities_from_file(file).err(),
        Some(super::RunRecordError::Corrupt)
    );
}

#[test]
fn incomplete_or_obsolete_attempt_fields_are_rejected() {
    let mut run = new_run();
    let _attempt = start(&mut run);
    let mut missing_route = serde_json::to_value(run.to_file()).expect("json");
    missing_route["attempts"][0]
        .as_object_mut()
        .expect("attempt")
        .remove("review-route");
    assert!(serde_json::from_value::<super::RunFile>(missing_route).is_err());

    let mut missing = serde_json::to_value(run.to_file()).expect("json");
    missing["attempts"][0]["capabilities"]
        .as_object_mut()
        .expect("capabilities")
        .remove("network-domains");
    assert!(serde_json::from_value::<super::RunFile>(missing).is_err());

    let mut obsolete = serde_json::to_value(run.to_file()).expect("json");
    obsolete["attempts"][0]["capabilities"]["secret"] = serde_json::json!("placeholder");
    assert!(serde_json::from_value::<super::RunFile>(obsolete).is_err());
}

#[test]
fn obsolete_capability_schemas_are_rejected() {
    let mut run = new_run();
    let _attempt = start(&mut run);
    let mut file = run.to_file();
    file.attempts[0].capabilities.schema = 2;
    assert_eq!(
        WorkflowRun::from_file(file).err(),
        Some(super::RunRecordError::Corrupt)
    );
}

#[test]
fn obsolete_transition_arrays_are_rejected() {
    let mut run = new_run();
    let attempt = start(&mut run);
    complete(&mut run, attempt, 12);
    let mut value = serde_json::to_value(run.to_file()).expect("json");
    value["transitions"] = serde_json::json!([]);
    assert!(serde_json::from_value::<super::RunFile>(value).is_err());
}

#[test]
fn snapshot_contradictions_fail_load() {
    let mut run = new_run();
    let attempt = start(&mut run);
    complete(&mut run, attempt, 12);
    let mut completed_as_failed = run.to_file();
    completed_as_failed.state = super::RunStateFile::Failed;
    assert_eq!(
        WorkflowRun::from_file(completed_as_failed).err(),
        Some(super::RunRecordError::Corrupt)
    );

    let mut route_on_normal = run.to_file();
    route_on_normal.attempts[0].review_route = Some("approved".to_owned());
    assert_eq!(
        WorkflowRun::from_file(route_on_normal).err(),
        Some(super::RunRecordError::Corrupt)
    );

    let mut reviewed = review_loop_run();
    complete_implementation(&mut reviewed, 11);
    complete_review(
        &mut reviewed,
        crate::workflows::artefacts::ReviewVerdict::Approved,
        13,
    );
    let mut missing_route = reviewed.to_file();
    let review = missing_route
        .attempts
        .iter_mut()
        .rev()
        .find(|attempt| attempt.review_route.is_some())
        .expect("review attempt");
    review.review_route = None;
    assert_eq!(
        WorkflowRun::from_file(missing_route).err(),
        Some(super::RunRecordError::Corrupt)
    );

    let mut active = new_run();
    start(&mut active);
    let mut terminal_active = active.to_file();
    terminal_active.state = super::RunStateFile::Completed;
    assert_eq!(
        WorkflowRun::from_file(terminal_active).err(),
        Some(super::RunRecordError::Corrupt)
    );

    let definition = two_step(true);
    let environments = crate::tests::test_environment_set(&definition);
    let mut missing_predecessor = WorkflowRun::configured(
        RunId::generate().expect("run"),
        10,
        crate::agents::AgentId::generate().expect("agent"),
        PinnedWorkflowDefinition::pin(None, definition),
        environments,
    );
    let first = start(&mut missing_predecessor);
    complete(&mut missing_predecessor, first, 12);
    let second = AttemptId::generate().expect("attempt");
    missing_predecessor
        .start_attempt(
            second,
            Vec::new(),
            test_command_capabilities(),
            sandbox_record(&missing_predecessor),
            13,
        )
        .expect("start");
    complete(&mut missing_predecessor, second, 14);
    let mut missing_predecessor = missing_predecessor.to_file();
    missing_predecessor.attempts.remove(0);
    assert_eq!(
        WorkflowRun::from_file(missing_predecessor).err(),
        Some(super::RunRecordError::Corrupt)
    );
}

fn quick_task_run(kind: super::RunKind) -> WorkflowRun {
    let pinned = crate::workflows::pin_quick_task(
        AccessMode::ReadWrite,
        &[ToolId::List],
        "Do the work.",
        test_environment_id(),
    )
    .expect("quick task");
    let environments = crate::tests::test_environment_set(&pinned.definition);
    WorkflowRun::create(
        RunId::generate().expect("run"),
        10,
        Some(crate::agents::AgentId::generate().expect("agent")),
        kind,
        pinned,
        environments,
    )
}

fn candidate_artefact(
    run_id: RunId,
    producer: crate::workflows::artefacts::ArtefactProducer,
    inputs: Vec<crate::workflows::artefacts::ArtefactReference>,
    seed: &[u8],
) -> crate::workflows::artefacts::ArtefactRecord {
    crate::workflows::artefacts::ArtefactRecord {
        id: crate::workflows::ArtefactId::generate().expect("artefact"),
        kind: ArtefactKind::CandidateRevision,
        artefact_hash: crate::workflows::artefacts::ArtefactHash::of(seed, b"candidate"),
        object_hash: crate::workflows::artefacts::ObjectHash::of(seed),
        payload_bytes: 1,
        created_at_ms: 11,
        provenance: crate::workflows::artefacts::ArtefactProvenance {
            run_id,
            producer,
            inputs,
        },
        summary: crate::workflows::artefacts::ArtefactSummary::Candidate {
            candidate: crate::workflows::artefacts::CandidateHash::of(seed),
            entries: 0,
            bytes: 0,
            disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
        },
    }
}

fn reach_quick_task_gate(run: &mut WorkflowRun, produced_seed: &[u8], exact_observed: bool) {
    let initial = candidate_artefact(
        run.id,
        crate::workflows::artefacts::ArtefactProducer::RunSourceCapture,
        Vec::new(),
        b"initial-tree",
    );
    let initial_ref = artefact_reference(&initial);
    run.record_initial_candidate(initial).expect("initial");
    let step = StepKey::parse("work").expect("work");
    let attempt = AttemptId::generate().expect("attempt");
    run.start_attempt(
        attempt,
        vec![super::AttemptArtefactInput {
            key: InputKey::parse("candidate").expect("input"),
            artefact: initial_ref.clone(),
        }],
        test_agent_capabilities(),
        step_sandbox(run, &step),
        11,
    )
    .expect("start");
    let produced = candidate_artefact(
        run.id,
        crate::workflows::artefacts::ArtefactProducer::StepAttempt {
            attempt_id: attempt,
            step: step.clone(),
            output: Some(OutputKey::parse("candidate").expect("output")),
            disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
        },
        vec![initial_ref],
        produced_seed,
    );
    let produced_ref = artefact_reference(&produced);
    run.record_attempt_outputs(
        attempt,
        vec![produced],
        vec![super::AttemptArtefactOutput {
            key: OutputKey::parse("candidate").expect("output"),
            artefact: produced_ref.clone(),
        }],
        Some(produced_ref.clone()),
        if exact_observed {
            super::ObservedCandidate::Exact {
                artefact: produced_ref,
            }
        } else {
            super::ObservedCandidate::Unknown
        },
    )
    .expect("outputs");
    run.record_cleanup(attempt, AttemptCleanupRecord::Complete)
        .expect("cleanup");
    run.complete_attempt(attempt, 12).expect("complete work");
}

#[test]
fn an_unchanged_quick_task_completes_without_a_gate() {
    let mut run = quick_task_run(super::RunKind::QuickTask);
    reach_quick_task_gate(&mut run, b"initial-tree", true);
    assert!(matches!(run.state, RunState::Ready { ref step } if step.as_str() == "gate"));
    run.complete_unchanged_task().expect("complete unchanged");
    assert_eq!(run.state, RunState::Completed);
    assert!(run.gates.is_empty());
    let loaded = WorkflowRun::from_file(run.to_file()).expect("round trip");
    assert_eq!(loaded, run);
}

#[test]
fn a_changed_quick_task_stays_at_the_human_gate() {
    let mut run = quick_task_run(super::RunKind::QuickTask);
    reach_quick_task_gate(&mut run, b"changed-tree", true);
    assert_eq!(run.complete_unchanged_task(), Err(TransitionError::Invalid));
    assert!(matches!(run.state, RunState::Ready { ref step } if step.as_str() == "gate"));
    assert!(run.gates.is_empty());
    run.state = RunState::Completed;
    assert_eq!(
        WorkflowRun::from_file(run.to_file()).err(),
        Some(super::RunRecordError::Corrupt)
    );
}

#[test]
fn a_configured_run_cannot_skip_its_human_gate() {
    let mut run = quick_task_run(super::RunKind::Configured);
    reach_quick_task_gate(&mut run, b"initial-tree", true);
    assert_eq!(run.complete_unchanged_task(), Err(TransitionError::Invalid));
    assert!(matches!(run.state, RunState::Ready { ref step } if step.as_str() == "gate"));
    run.state = RunState::Completed;
    assert_eq!(
        WorkflowRun::from_file(run.to_file()).err(),
        Some(super::RunRecordError::Corrupt)
    );
}

#[test]
fn unchanged_quick_task_completion_rejects_missing_and_unknown_candidates() {
    let mut missing = quick_task_run(super::RunKind::QuickTask);
    let initial = candidate_artefact(
        missing.id,
        crate::workflows::artefacts::ArtefactProducer::RunSourceCapture,
        Vec::new(),
        b"initial-tree",
    );
    let initial_ref = artefact_reference(&initial);
    missing.record_initial_candidate(initial).expect("initial");
    let step = StepKey::parse("work").expect("work");
    let attempt = AttemptId::generate().expect("attempt");
    missing
        .start_attempt(
            attempt,
            vec![super::AttemptArtefactInput {
                key: InputKey::parse("candidate").expect("input"),
                artefact: initial_ref,
            }],
            test_agent_capabilities(),
            step_sandbox(&missing, &step),
            11,
        )
        .expect("start");
    missing
        .record_cleanup(attempt, AttemptCleanupRecord::Complete)
        .expect("cleanup");
    missing
        .complete_attempt(attempt, 12)
        .expect("complete work");
    assert_eq!(
        missing.complete_unchanged_task(),
        Err(TransitionError::Invalid)
    );

    let mut unknown = quick_task_run(super::RunKind::QuickTask);
    reach_quick_task_gate(&mut unknown, b"initial-tree", false);
    assert_eq!(
        unknown.complete_unchanged_task(),
        Err(TransitionError::Invalid)
    );

    let mut mismatched = quick_task_run(super::RunKind::QuickTask);
    reach_quick_task_gate(&mut mismatched, b"initial-tree", true);
    let super::RunSource::Captured { source } = &mut mismatched.source else {
        panic!("captured source");
    };
    source.observed = super::ObservedCandidate::Exact {
        artefact: source.initial.clone(),
    };
    assert_eq!(
        mismatched.complete_unchanged_task(),
        Err(TransitionError::Invalid)
    );

    let mut ready = quick_task_run(super::RunKind::QuickTask);
    assert_eq!(
        ready.complete_unchanged_task(),
        Err(TransitionError::Invalid)
    );
}

#[test]
fn ordinary_quick_task_revisions_bind_the_candidate_and_enforce_limits() {
    use crate::workflows::gates::{GateRevision, HumanDecisionKind};

    let pinned = crate::workflows::pin_project_free_quick_task_with_directories(
        &[ToolId::List],
        "Change the file.",
        test_environment_id(),
        Vec::new(),
        true,
    )
    .expect("ordinary quick task");
    let gate_key = StepKey::parse("gate").expect("gate");
    let environments = crate::tests::test_environment_set(&pinned.definition);
    let mut run = WorkflowRun::create_source_free_for_conversation(
        RunId::generate().expect("run"),
        10,
        crate::conversations::ConversationId::generate().expect("conversation"),
        pinned,
        environments,
        Vec::new(),
    );
    let initial = candidate_artefact(
        run.id,
        crate::workflows::artefacts::ArtefactProducer::RunSourceCapture,
        Vec::new(),
        b"ordinary-initial",
    );
    let initial_ref = artefact_reference(&initial);
    run.record_initial_candidate(initial).expect("initial");
    let work = StepKey::parse("work").expect("work");
    let attempt = AttemptId::generate().expect("attempt");
    run.start_attempt(
        attempt,
        vec![super::AttemptArtefactInput {
            key: InputKey::parse("candidate").expect("input"),
            artefact: initial_ref.clone(),
        }],
        test_agent_capabilities(),
        step_sandbox(&run, &work),
        11,
    )
    .expect("start");
    let produced = candidate_artefact(
        run.id,
        crate::workflows::artefacts::ArtefactProducer::StepAttempt {
            attempt_id: attempt,
            step: work.clone(),
            output: Some(OutputKey::parse("candidate").expect("output")),
            disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
        },
        vec![initial_ref.clone()],
        b"ordinary-rejected",
    );
    let produced_ref = artefact_reference(&produced);
    run.record_attempt_outputs(
        attempt,
        vec![produced],
        vec![super::AttemptArtefactOutput {
            key: OutputKey::parse("candidate").expect("output"),
            artefact: produced_ref.clone(),
        }],
        Some(produced_ref.clone()),
        super::ObservedCandidate::Exact {
            artefact: produced_ref.clone(),
        },
    )
    .expect("outputs");
    run.record_cleanup(attempt, AttemptCleanupRecord::Complete)
        .expect("cleanup");
    run.complete_attempt(attempt, 12).expect("complete work");
    let gate_id = crate::workflows::GateId::generate().expect("gate");
    let gate = run
        .open_gate(gate_id, produced_ref.clone(), initial_ref.clone(), 13)
        .expect("gate");
    let run_id = run.id;
    let decision = |at: u64| crate::workflows::artefacts::ArtefactRecord {
        id: crate::workflows::ArtefactId::generate().expect("decision"),
        kind: ArtefactKind::HumanDecision,
        artefact_hash: crate::workflows::artefacts::ArtefactHash::of(
            format!("ordinary-decision-{at}").as_bytes(),
            b"human-decision",
        ),
        object_hash: crate::workflows::artefacts::ObjectHash::of(
            format!("ordinary-decision-{at}").as_bytes(),
        ),
        payload_bytes: 1,
        created_at_ms: at,
        provenance: crate::workflows::artefacts::ArtefactProvenance {
            run_id,
            producer: crate::workflows::artefacts::ArtefactProducer::HumanGate {
                gate_id,
                step: gate_key.clone(),
                output: OutputKey::parse("decision").expect("output"),
            },
            inputs: vec![produced_ref.clone()],
        },
        summary: crate::workflows::artefacts::ArtefactSummary::HumanDecision {
            candidate: crate::workflows::artefacts::CandidateHash::of(b"ordinary-rejected"),
            diff_base: crate::workflows::artefacts::CandidateHash::of(b"ordinary-initial"),
            decision: HumanDecisionKind::RevisionRequested,
        },
    };
    let stale_revision = GateRevision::new(99).expect("revision");
    assert_eq!(
        run.decide_gate(
            gate_id,
            stale_revision,
            decision(14),
            HumanDecisionKind::RevisionRequested,
            Some("Fix the candidate".to_owned()),
            Some(AttemptId::generate().expect("attempt")),
            14,
        ),
        Err(TransitionError::Invalid)
    );
    let reserved = AttemptId::generate().expect("attempt");
    run.decide_gate(
        gate_id,
        gate.revision,
        decision(14),
        HumanDecisionKind::RevisionRequested,
        Some("Fix the candidate".to_owned()),
        Some(reserved),
        14,
    )
    .expect("reserve revision");
    let reservation = run.revision_reservation.clone().expect("reservation");
    assert_eq!(reservation.candidate, produced_ref);
    assert_eq!(reservation.diff_base, initial_ref);
    assert_eq!(reservation.target.as_str(), "work");
    assert_eq!(reservation.feedback, "Fix the candidate");
    assert!(matches!(run.state, RunState::Ready { ref step } if step.as_str() == "work"));
    assert_eq!(
        run.decide_gate(
            gate_id,
            gate.revision,
            decision(15),
            HumanDecisionKind::RevisionRequested,
            Some("Fix the candidate".to_owned()),
            Some(AttemptId::generate().expect("attempt")),
            15,
        ),
        Err(TransitionError::Invalid)
    );
    let mut exhausted = run.clone();
    exhausted.revision_reservation = None;
    for sequence in [2u64, 3u64] {
        exhausted
            .gates
            .push(crate::workflows::gates::HumanGateRecord {
                id: crate::workflows::GateId::generate().expect("prior gate"),
                step: gate_key.clone(),
                sequence: sequence as u32,
                revision: GateRevision::new(sequence).expect("revision"),
                opened_at_ms: 10,
                closed_at_ms: Some(11),
                candidate: produced_ref.clone(),
                diff_base: initial_ref.clone(),
                state: crate::workflows::gates::HumanGateState::RevisionRequested,
                decision: None,
                output: OutputKey::parse("decision").expect("output"),
            });
    }
    let next_gate = crate::workflows::GateId::generate().expect("gate");
    exhausted
        .gates
        .push(crate::workflows::gates::HumanGateRecord {
            id: next_gate,
            step: gate_key.clone(),
            sequence: 4,
            revision: GateRevision::new(4).expect("revision"),
            opened_at_ms: 16,
            closed_at_ms: None,
            candidate: produced_ref.clone(),
            diff_base: initial_ref.clone(),
            state: crate::workflows::gates::HumanGateState::AwaitingDecision,
            decision: None,
            output: OutputKey::parse("decision").expect("output"),
        });
    exhausted.state = RunState::AwaitingHuman {
        step: gate_key.clone(),
        gate: next_gate,
    };
    exhausted
        .decide_gate(
            next_gate,
            GateRevision::new(4).expect("revision"),
            decision(17),
            HumanDecisionKind::RevisionRequested,
            Some("Fix the candidate".to_owned()),
            Some(AttemptId::generate().expect("attempt")),
            17,
        )
        .expect("exhaust limit");
    assert!(matches!(
        exhausted.state,
        RunState::Escalated {
            reason: EscalationReason::AttemptLimit,
            ..
        }
    ));
    assert!(exhausted.revision_reservation.is_none());
}

fn partial_transaction(
    outcome: crate::workflows::apply::ApplyRootOutcome,
) -> crate::workflows::apply::ApplyTransaction {
    use crate::execution::{CanonicalDirectoryIdentity, DirectoryGrantId};
    use crate::workflows::artefacts::{ArtefactHash, ArtefactReference, CandidateHash};
    let reference =
        |byte: u8, kind: crate::workflows::definition::ArtefactKind| ArtefactReference {
            id: crate::workflows::ArtefactId::generate().expect("artefact"),
            kind,
            artefact_hash: ArtefactHash::of(b"partial", &[byte]),
        };
    crate::workflows::apply::ApplyTransaction {
        state: crate::workflows::apply::ApplyTransactionState::Recovered,
        roots: vec![crate::workflows::apply::ApplyRoot {
            grant_id: DirectoryGrantId::generate().expect("grant"),
            alias: "fieldnotes".to_owned(),
            host_path: std::path::PathBuf::from("/tmp/fieldnotes"),
            identity: CanonicalDirectoryIdentity {
                device: 7,
                inode: 8,
            },
            baseline_candidate: CandidateHash::of(b"base"),
            candidate_hash: CandidateHash::of(b"candidate"),
            exclusions: Vec::new(),
            outcome,
        }],
        baseline: reference(
            1,
            crate::workflows::definition::ArtefactKind::CandidateRevision,
        ),
        candidate: reference(
            2,
            crate::workflows::definition::ArtefactKind::CandidateRevision,
        ),
        approval: reference(3, crate::workflows::definition::ArtefactKind::HumanDecision),
    }
}

fn active_run_with_apply(
    outcome: crate::workflows::apply::ApplyRootOutcome,
    state: crate::workflows::apply::ApplyTransactionState,
    cleanup: AttemptCleanupRecord,
) -> (WorkflowRun, AttemptId) {
    let mut run = new_run();
    let attempt = start(&mut run);
    let mut transaction = partial_transaction(outcome);
    transaction.state = state;
    run.record_apply_transaction(attempt, transaction)
        .expect("apply transaction");
    if cleanup != AttemptCleanupRecord::Pending {
        run.record_cleanup(attempt, cleanup).expect("cleanup");
    }
    (run, attempt)
}

#[test]
fn known_partial_settlement_ends_through_cancellation() {
    use crate::workflows::apply::{ApplyRootOutcome, ApplyTransactionState};
    let (mut run, _) = active_run_with_apply(
        ApplyRootOutcome::Applied,
        ApplyTransactionState::Recovered,
        AttemptCleanupRecord::Complete,
    );
    assert!(run.apply_is_known_partial());
    assert!(!run.apply_is_uncertain());
    assert!(run.partial_settlement_eligible());
    run.settle_known_partial(12).expect("settle");
    assert_eq!(run.state, RunState::Cancelled);
}

#[test]
fn uncertain_recovery_blocks_settlement_release() {
    use crate::workflows::apply::{ApplyRootOutcome, ApplyTransactionState};
    let (mut run, _) = active_run_with_apply(
        ApplyRootOutcome::Uncertain,
        ApplyTransactionState::RecoveryUncertain,
        AttemptCleanupRecord::Complete,
    );
    assert!(run.apply_is_uncertain());
    assert!(!run.apply_is_known_partial());
    assert!(!run.partial_settlement_eligible());
    assert!(run.settle_known_partial(12).is_err());
    assert!(!run.is_terminal());
}

#[test]
fn pending_cleanup_blocks_settlement_release() {
    use crate::workflows::apply::{ApplyRootOutcome, ApplyTransactionState};
    let (mut run, _) = active_run_with_apply(
        ApplyRootOutcome::Applied,
        ApplyTransactionState::Recovered,
        AttemptCleanupRecord::Pending,
    );
    assert!(run.apply_is_uncertain());
    assert!(!run.partial_settlement_eligible());
    assert!(run.settle_known_partial(12).is_err());
}

#[test]
fn terminal_partial_offers_no_redundant_settlement() {
    use crate::workflows::apply::{ApplyRootOutcome, ApplyTransactionState};
    let (mut run, attempt) = active_run_with_apply(
        ApplyRootOutcome::Applied,
        ApplyTransactionState::Recovered,
        AttemptCleanupRecord::Complete,
    );
    run.fail_attempt(attempt, FailureCategory::Apply, 12)
        .expect("fail");
    assert!(run.is_terminal());
    assert!(!run.partial_settlement_eligible());
    assert!(run.settle_known_partial(13).is_err());
}
