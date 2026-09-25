pub(crate) use super::capabilities::tests::test_agent_capabilities;
pub(crate) use super::definition::tests::{test_environment_id, test_named_definition};
pub(crate) use super::resolve::tests::test_set as test_environment_set;

use super::definition::{PinnedWorkflowDefinition, StepAction, WorkflowDefinition};
use super::run::{AttemptCleanupRecord, AttemptSandboxKind, AttemptSandboxRecord};
use super::{AttemptId, RunId, RunKind, WorkflowRun};

pub(crate) fn settings() -> crate::execution::ExecutionSettings {
    crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(
            crate::providers::ProviderKind::Xai,
            "test-model".to_owned(),
            None,
        )
        .unwrap(),
        String::new(),
        crate::agents::ToolId::ALL.to_vec(),
        test_environment_id(),
    )
    .unwrap()
}

impl WorkflowRun {
    pub(crate) fn configured(
        id: RunId,
        created_at_ms: u64,
        agent: crate::agents::AgentId,
        pinned: PinnedWorkflowDefinition,
        environments: super::resolve::ResolvedEnvironmentSet,
    ) -> Self {
        Self::create(
            id,
            created_at_ms,
            Some(agent),
            RunKind::Configured,
            pinned,
            environments,
        )
    }

    pub(crate) fn create(
        id: RunId,
        created_at_ms: u64,
        agent_id: Option<crate::agents::AgentId>,
        kind: RunKind,
        pinned: PinnedWorkflowDefinition,
        environments: super::resolve::ResolvedEnvironmentSet,
    ) -> Self {
        let phase_models = pinned
            .definition
            .steps()
            .iter()
            .filter(|step| matches!(step.action, StepAction::Agent(_)))
            .map(|step| {
                let StepAction::Agent(action) = &step.action else {
                    unreachable!()
                };
                let mut settings = action.settings.resolve(&settings());
                settings.environment = pinned.definition.effective_environment(step);
                super::PhaseModelSelection {
                    step: step.key.clone(),
                    selection: settings.model.clone(),
                    instructions: settings.instructions.clone(),
                    preset: None,
                    settings: Some(settings),
                }
            })
            .collect();
        let mut run = Self::create_source_free_for_conversation(
            id,
            created_at_ms,
            crate::conversations::ConversationId::generate().unwrap(),
            pinned,
            environments,
            phase_models,
            settings(),
        );
        run.kind = kind;
        if let Some(agent) = agent_id {
            run.agent_id = Some(agent);
            run.conversation_id = None;
        }
        run
    }
}

pub(crate) fn run(definition: WorkflowDefinition) -> WorkflowRun {
    let environments = test_environment_set(&definition);
    WorkflowRun::create(
        RunId::generate().unwrap(),
        1,
        None,
        RunKind::Configured,
        PinnedWorkflowDefinition::pin(None, definition),
        environments,
    )
}

pub(crate) fn start(
    run: &mut WorkflowRun,
    inputs: Vec<super::run::AttemptArtefactInput>,
    at: u64,
) -> AttemptId {
    let step = run
        .pinned
        .definition
        .step(run.ready_step().unwrap())
        .unwrap();
    let authority = crate::execution::ProjectFreeAuthority::from_snapshot(
        1,
        run.phase_settings(&step.key).unwrap(),
    )
    .unwrap();
    let capabilities =
        super::capabilities::AttemptCapabilities::derive_project_free(step, &authority).unwrap();
    let sandbox = AttemptSandboxRecord {
        kind: AttemptSandboxKind::IsolatedAttempt,
        snapshot_digest: run
            .environments
            .steps
            .iter()
            .find(|binding| binding.step == step.key)
            .unwrap()
            .snapshot_digest
            .clone(),
    };
    let attempt = run
        .revision_reservation
        .as_ref()
        .map(|reservation| reservation.attempt)
        .unwrap_or_else(|| AttemptId::generate().unwrap());
    run.start_attempt(attempt, inputs, capabilities, sandbox, at)
        .unwrap();
    attempt
}

pub(crate) fn complete_plan(
    run: &mut WorkflowRun,
    store: &super::artefacts::WorkflowArtefactRepository,
    markdown: &str,
    at: u64,
) -> super::artefacts::ArtefactReference {
    let step = run
        .pinned
        .definition
        .step(run.ready_step().unwrap())
        .unwrap()
        .clone();
    let inputs = run
        .current_plan()
        .into_iter()
        .map(|plan| super::run::AttemptArtefactInput {
            key: super::definition::InputKey::parse("plan").unwrap(),
            artefact: plan,
        })
        .collect();
    let attempt = start(run, inputs, at);
    let (bytes, _, hash) = super::artefacts::payload::encode_plan(markdown, None).unwrap();
    let output = step
        .required_outputs()
        .iter()
        .find(|output| output.kind == super::definition::OutputKind::Plan)
        .unwrap();
    let record = super::artefacts::ArtefactRecord {
        id: super::ArtefactId::generate().unwrap(),
        kind: super::definition::ArtefactKind::Plan,
        artefact_hash: hash,
        object_hash: store.publish(&bytes).unwrap(),
        payload_bytes: bytes.len() as u64,
        created_at_ms: at + 1,
        provenance: super::artefacts::ArtefactProvenance {
            run_id: run.id,
            producer: super::artefacts::ArtefactProducer::StepAttempt {
                attempt_id: attempt,
                step: step.key.clone(),
                output: Some(output.key.clone()),
                disposition: super::artefacts::ProductionDisposition::RequiredOutput,
            },
            inputs: run
                .attempts
                .last()
                .unwrap()
                .inputs
                .iter()
                .map(|input| input.artefact.clone())
                .collect(),
        },
        summary: super::artefacts::ArtefactSummary::Plan {
            markdown_bytes: markdown.len() as u64,
        },
    };
    let reference = super::artefacts::ArtefactReference {
        id: record.id,
        kind: record.kind,
        artefact_hash: record.artefact_hash,
    };
    run.record_attempt_outputs(
        attempt,
        vec![record],
        vec![super::run::AttemptArtefactOutput {
            key: output.key.clone(),
            artefact: reference.clone(),
        }],
    )
    .unwrap();
    run.record_cleanup(attempt, AttemptCleanupRecord::Complete)
        .unwrap();
    run.complete_attempt(attempt, at + 1).unwrap();
    reference
}
