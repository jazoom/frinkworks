use super::*;
use crate::tests::test_environment_id;

pub(crate) fn test_set(definition: &WorkflowDefinition) -> ResolvedEnvironmentSet {
    let preparation_id = PreparationId::parse(&"b".repeat(32)).expect("prep");
    let snapshot = crate::tests::sample_snapshot(preparation_id);
    let recipe_version = EnvironmentRecipeVersion::parse(&"c".repeat(64)).expect("recipe");
    let mut environments = Vec::new();
    let mut steps = Vec::new();
    for (step, environment_id) in step_environment_ids(definition) {
        if !environments
            .iter()
            .any(|item: &ResolvedEnvironment| item.environment_id == environment_id)
        {
            environments.push(ResolvedEnvironment {
                environment_id,
                name: "Alpine Git".to_owned(),
                preparation_id,
                recipe_version,
                snapshot: snapshot.clone(),
            });
        }
        steps.push(ResolvedStepEnvironment {
            step,
            environment_id,
            preparation_id,
            snapshot_digest: snapshot.snapshot_digest.clone(),
        });
    }
    ResolvedEnvironmentSet {
        environments,
        steps,
    }
}

use super::{ResolveEnvironmentError, resolve_environments, validate_replacement_environment};
use crate::agents::ToolId;
use crate::environments::{
    EnvironmentCatalogue, EnvironmentDraft, EnvironmentSnapshotRepository, SnapshotAvailability,
};
use crate::tests::sample_snapshot;
use crate::workflows::definition::{
    ASSISTANT_REPLY, AgentAuthority, AgentStep, OutputKey, OutputKind, RequiredOutput,
    RoleDefinition, RoleKey, StepAccess, StepAction, StepDefinition, StepEnvironment, StepKey,
    WorkflowDefinition,
};

fn draft(name: &str) -> EnvironmentDraft {
    EnvironmentDraft {
        name: name.to_owned(),
        oci_image: "alpine/git".to_owned(),
        setup_script: String::new(),
    }
}

fn definition(default: crate::environments::EnvironmentId) -> WorkflowDefinition {
    let role = RoleDefinition::new(
        RoleKey::parse("agent").expect("role"),
        "Coding agent".to_owned(),
        String::new(),
        String::new(),
    )
    .expect("role");
    let authority = AgentAuthority::new(vec![ToolId::List], Vec::new()).expect("authority");
    WorkflowDefinition::from_parts(
        "One step".to_owned(),
        default,
        vec![role],
        vec![StepDefinition {
            key: StepKey::parse("work").expect("step"),
            name: "Work on task".to_owned(),
            inputs: Vec::new(),
            action: StepAction::Agent(AgentStep {
                environment: StepEnvironment::WorkflowDefault,
                role: RoleKey::parse("agent").expect("role"),
                directory_access: StepAccess::Write,
                authority,
                settings: crate::workflows::definition::ModelStepSettings::SameAsRunDefaults,
                required_outputs: vec![RequiredOutput {
                    key: OutputKey::parse(ASSISTANT_REPLY).expect("output"),
                    kind: OutputKind::AssistantReply,
                }],
            }),
            review: None,
        }],
    )
    .expect("definition")
}

#[tokio::test]
async fn missing_environments_are_rejected() {
    let catalogue = EnvironmentCatalogue::in_memory();
    let snapshots = EnvironmentSnapshotRepository::in_memory();
    let error = resolve_environments(&definition(test_environment_id()), &catalogue, &snapshots)
        .await
        .err();
    assert_eq!(error, Some(ResolveEnvironmentError::Missing));
}

#[tokio::test]
async fn unready_environments_are_rejected() {
    let catalogue = EnvironmentCatalogue::in_memory();
    let snapshots = EnvironmentSnapshotRepository::in_memory();
    let (record, _) = catalogue.create(draft("Alpine Git")).expect("create");
    let error = resolve_environments(&definition(record.id), &catalogue, &snapshots)
        .await
        .err();
    assert_eq!(error, Some(ResolveEnvironmentError::NotReady));
}

#[tokio::test]
async fn unavailable_snapshots_are_rejected() {
    let catalogue = EnvironmentCatalogue::in_memory();
    let snapshots = EnvironmentSnapshotRepository::in_memory();
    let (record, preparation) = catalogue.create(draft("Alpine Git")).expect("create");
    catalogue.claim_oldest_queued().expect("claim");
    let snapshot = sample_snapshot(preparation.id);
    snapshots.mark(snapshot.artifact_key.clone(), SnapshotAvailability::Corrupt);
    catalogue
        .finish_ready(&preparation.id, snapshot, preparation.log)
        .expect("ready");
    let error = resolve_environments(&definition(record.id), &catalogue, &snapshots)
        .await
        .err();
    assert_eq!(error, Some(ResolveEnvironmentError::Unavailable));
}

#[tokio::test]
async fn replacement_checks_reject_unknown_identities_without_fallback() {
    let catalogue = EnvironmentCatalogue::in_memory();
    let snapshots = EnvironmentSnapshotRepository::in_memory();
    let error = validate_replacement_environment(&catalogue, &snapshots, test_environment_id())
        .await
        .err();
    assert_eq!(error, Some(ResolveEnvironmentError::Missing));
}

#[tokio::test]
async fn replacement_checks_reject_unready_environments_without_fallback() {
    let catalogue = EnvironmentCatalogue::in_memory();
    let snapshots = EnvironmentSnapshotRepository::in_memory();
    let (record, _) = catalogue.create(draft("Alpine Git")).expect("create");
    let error = validate_replacement_environment(&catalogue, &snapshots, record.id)
        .await
        .err();
    assert_eq!(error, Some(ResolveEnvironmentError::NotReady));
}

#[tokio::test]
async fn replacement_checks_accept_ready_environments() {
    let catalogue = EnvironmentCatalogue::in_memory();
    let snapshots = EnvironmentSnapshotRepository::in_memory();
    let (record, preparation) = catalogue.create(draft("Alpine Git")).expect("create");
    catalogue.claim_oldest_queued().expect("claim");
    let snapshot = sample_snapshot(preparation.id);
    snapshots.mark(
        snapshot.artifact_key.clone(),
        SnapshotAvailability::Available,
    );
    catalogue
        .finish_ready(&preparation.id, snapshot, preparation.log)
        .expect("ready");
    validate_replacement_environment(&catalogue, &snapshots, record.id)
        .await
        .expect("ready environment");
}
