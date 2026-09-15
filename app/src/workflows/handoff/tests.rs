use super::*;
use crate::workflows::{
    artefacts::{
        ArtefactProducer, ArtefactProvenance, ArtefactRecord, ArtefactReference, ArtefactSummary,
        CandidateCapture, ProductionDisposition, artefact_hash_for,
    },
    definition::{ArtefactKind, InputKey, OutputKey},
    run::{
        AttemptArtefactInput, AttemptArtefactOutput, AttemptCleanupRecord, AttemptSandboxKind,
        AttemptSandboxRecord, ObservedCandidate,
    },
};

pub(crate) fn prepared_run(
    state: &crate::state::AppState,
    conversation: &crate::conversations::ConversationRecord,
) -> (WorkflowRun, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("file.txt"), "original\n").unwrap();
    let mut grant = crate::execution::DirectoryGrant::from_selected(directory.path(), &[]).unwrap();
    grant.access = crate::execution::DirectoryAccess::ReviewBeforeApply;
    let mut settings = conversation.model.as_ref().unwrap().settings.clone();
    settings.directories = vec![grant];
    settings.tools = vec![crate::agents::ToolId::Read, crate::agents::ToolId::Write];
    let pinned = crate::workflows::pin_agent_work(&settings).unwrap();
    let environments = crate::tests::test_environment_set(&pinned.definition);
    let work = pinned.definition.steps()[0].clone();
    let mut run = WorkflowRun::create_source_free_for_conversation(
        RunId::generate().unwrap(),
        1,
        conversation.id,
        pinned,
        environments,
        vec![crate::workflows::PhaseModelSelection {
            step: work.key.clone(),
            selection: settings.model.clone(),
            instructions: settings.instructions.clone(),
            preset: None,
            settings: Some(settings.clone()),
        }],
    );
    let publish =
        |run: &WorkflowRun, producer: ArtefactProducer, inputs: Vec<ArtefactReference>| {
            let candidate = CandidateCapture::capture_set(
                &settings.directories,
                state.local_data.root(),
                &state.workflow_artefacts,
            )
            .unwrap();
            let bytes = candidate.manifest_bytes().unwrap();
            ArtefactRecord {
                id: crate::workflows::ArtefactId::generate().unwrap(),
                kind: ArtefactKind::CandidateRevision,
                artefact_hash: artefact_hash_for(ArtefactKind::CandidateRevision, 1, &bytes),
                object_hash: state.workflow_artefacts.publish(&bytes).unwrap(),
                payload_bytes: bytes.len() as u64,
                created_at_ms: if matches!(producer, ArtefactProducer::RunSourceCapture) {
                    2
                } else {
                    4
                },
                provenance: ArtefactProvenance {
                    run_id: run.id,
                    producer,
                    inputs,
                },
                summary: ArtefactSummary::Candidate {
                    candidate: candidate.candidate_hash,
                    entries: 1,
                    bytes: 9,
                    disposition: ProductionDisposition::RequiredOutput,
                },
            }
        };
    let reference = |record: &ArtefactRecord| ArtefactReference {
        id: record.id,
        kind: record.kind,
        artefact_hash: record.artefact_hash,
    };
    let initial = publish(&run, ArtefactProducer::RunSourceCapture, Vec::new());
    let baseline = reference(&initial);
    run.record_initial_candidate(initial).unwrap();
    let attempt = crate::workflows::AttemptId::generate().unwrap();
    let authority = crate::execution::ProjectFreeAuthority::from_settings(1, &settings).unwrap();
    run.start_attempt(
        attempt,
        vec![AttemptArtefactInput {
            key: InputKey::parse("candidate").unwrap(),
            artefact: baseline.clone(),
        }],
        crate::workflows::capabilities::AttemptCapabilities::derive_project_free(&work, &authority)
            .unwrap(),
        AttemptSandboxRecord {
            kind: AttemptSandboxKind::IsolatedAttempt,
            snapshot_digest: run.environments.steps[0].snapshot_digest.clone(),
        },
        3,
    )
    .unwrap();
    std::fs::write(directory.path().join("file.txt"), "prepared\n").unwrap();
    let candidate = publish(
        &run,
        ArtefactProducer::StepAttempt {
            attempt_id: attempt,
            step: work.key,
            output: Some(OutputKey::parse("candidate").unwrap()),
            disposition: ProductionDisposition::RequiredOutput,
        },
        vec![baseline.clone()],
    );
    let selected = reference(&candidate);
    std::fs::write(directory.path().join("file.txt"), "original\n").unwrap();
    run.record_attempt_outputs(
        attempt,
        vec![candidate],
        vec![AttemptArtefactOutput {
            key: OutputKey::parse("candidate").unwrap(),
            artefact: selected.clone(),
        }],
        Some(selected.clone()),
        ObservedCandidate::Exact {
            artefact: selected.clone(),
        },
    )
    .unwrap();
    run.record_cleanup(attempt, AttemptCleanupRecord::Complete)
        .unwrap();
    run.complete_attempt(attempt, 4).unwrap();
    run.open_gate(
        crate::workflows::GateId::generate().unwrap(),
        selected,
        baseline,
        5,
    )
    .unwrap();
    (run, directory)
}

#[test]
fn ownership_transfer_is_single_owner_and_preserves_baseline_and_progress_after_restart() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let conversation = state
        .conversations
        .create_saved(
            ConversationId::generate().unwrap(),
            None,
            None,
            Some(
                crate::conversations::ConversationModelConfiguration::direct(
                    crate::providers::ModelSelection::new(
                        crate::providers::ProviderKind::Xai,
                        "model".to_owned(),
                        None,
                    )
                    .unwrap(),
                    crate::tests::test_environment_id(),
                ),
            ),
            Vec::new(),
        )
        .unwrap();
    let (run, directory) = prepared_run(&state, &conversation);
    let store_root = tempfile::tempdir().unwrap();
    let store = crate::workflows::WorkflowRunStore::open(store_root.path().to_owned()).unwrap();
    store.create(run.clone()).unwrap();
    let destination = ConversationId::generate().unwrap();
    let fingerprint = run.handoff_fingerprint();
    std::fs::write(directory.path().join("file.txt"), "concurrent host edit\n").unwrap();
    let transferred = store
        .mutate(&run.id, |current| {
            current.transfer(&fingerprint, conversation.id, destination)
        })
        .unwrap();
    assert_eq!(transferred.source, run.source);
    assert_eq!(transferred.artefacts, run.artefacts);
    assert_eq!(transferred.gates, run.gates);
    assert_eq!(transferred.state, run.state);
    assert_eq!(transferred.phase_models, run.phase_models);
    assert!(
        store
            .mutate(&run.id, |current| current.transfer(
                &fingerprint,
                conversation.id,
                ConversationId::generate().unwrap()
            ))
            .is_err()
    );
    drop(store);
    let reopened = crate::workflows::WorkflowRunStore::open(store_root.path().to_owned()).unwrap();
    reopened.interrupt_active().unwrap();
    assert_eq!(reopened.get(&run.id), Some(transferred));
    assert!(reopened.for_conversation(&conversation.id).is_empty());
    assert_eq!(reopened.for_conversation(&destination).len(), 1);
    assert_eq!(
        std::fs::read_to_string(directory.path().join("file.txt")).unwrap(),
        "concurrent host edit\n"
    );
}
