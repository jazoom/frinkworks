use super::*;

impl super::WorkflowCatalogue {
    pub(crate) fn in_memory() -> Self {
        Self {
            path: None,
            inner: Mutex::new(empty_state()),
            unavailable_starters: Vec::new(),
        }
    }
    pub(crate) fn open(
        path: PathBuf,
        default_environment: crate::environments::EnvironmentId,
    ) -> Result<Self, CatalogueError> {
        Self::open_with_seeds(
            path,
            &crate::workflows::seeds::production_seeds(default_environment),
        )
    }
    pub(crate) fn retired_ids(&self) -> Vec<WorkflowId> {
        self.lock().retired_workflow_ids.clone()
    }
    pub(crate) fn applied_seed_count(&self) -> usize {
        self.lock().applied_seeds.len()
    }
}

use super::{
    CatalogueError, ResolveWorkflowError, SELECTION_TOKEN_BYTES, WorkflowCatalogue,
    WorkflowSelection, definition_fits_agent,
};
use crate::agents::{AccessMode, ToolId};
use crate::tests::{test_environment_id, test_named_definition};
use crate::workflows::definition::{
    ASSISTANT_REPLY, AgentAuthority, AgentStep, CandidateAuthority, OutputKey, OutputKind,
    RequiredOutput, RoleDefinition, RoleKey, StepAction, StepDefinition, StepEnvironment, StepKey,
    SystemCommandId, SystemCommandStep, WorkflowDefinition, candidate_revision_output,
    initial_candidate_input,
};
use crate::workflows::id::WorkflowId;
use crate::workflows::seeds::{SeedKey, WorkflowSeed};

fn named(name: &str) -> WorkflowDefinition {
    test_named_definition(name)
}

fn write_catalogue(path: &std::path::Path, json: &str) {
    std::fs::write(path, json).expect("write");
}

#[test]
fn create_assigns_a_random_identifier_and_first_revision() {
    let catalogue = WorkflowCatalogue::in_memory();
    let record = catalogue.create(named("Planner")).expect("create");
    assert_eq!(record.revision, 1);
    assert_eq!(record.created_at_ms, record.updated_at_ms);
    assert_eq!(record.definition_version, record.definition.version());
    assert_eq!(catalogue.list().len(), 1);
    assert_eq!(catalogue.get(&record.id).expect("get").id, record.id);
}

#[test]
fn absent_starters_do_not_break_a_full_existing_catalogue() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("workflows.json");
    let catalogue = WorkflowCatalogue::open_with_seeds(path.clone(), &[]).expect("open");
    for index in 0..super::MAXIMUM_WORKFLOWS {
        catalogue
            .create(named(&format!("Existing {index}")))
            .expect("create existing workflow");
    }
    let starter = WorkflowSeed {
        key: SeedKey::parse("capacity-starter-v1").expect("seed key"),
        definition: named("Starter workflow"),
    };
    let reopened = WorkflowCatalogue::open_with_seeds(path, &[starter]).expect("reopen");
    assert_eq!(reopened.list().len(), super::MAXIMUM_WORKFLOWS);
    assert_eq!(
        reopened.unavailable_starters(),
        vec!["Starter workflow".to_owned()]
    );
}

#[test]
fn absent_starters_respect_the_serialised_byte_limit_without_changing_existing_data() {
    use crate::workflows::definition::{MAXIMUM_PROMPT_DEFAULTS_BYTES, MAXIMUM_ROLES};

    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("workflows.json");
    let base = crate::workflows::seeds::plan_a_change_definition(test_environment_id());
    let mut roles = base.roles().to_vec();
    roles[0].prompt_defaults = "\"".repeat(MAXIMUM_PROMPT_DEFAULTS_BYTES);
    for index in 1..MAXIMUM_ROLES {
        let mut role = roles[0].clone();
        role.key = RoleKey::parse(&format!("role-{index}")).expect("role key");
        roles.push(role);
    }
    let steps = roles
        .iter()
        .enumerate()
        .map(|(index, role)| {
            let mut step = base.steps()[0].clone();
            step.key = StepKey::parse(&format!("step-{index}")).expect("step key");
            let StepAction::Agent(action) = &mut step.action else {
                unreachable!()
            };
            action.role = role.key.clone();
            step
        })
        .collect();
    let large = WorkflowDefinition::from_parts(
        "Large starter".to_owned(),
        test_environment_id(),
        roles,
        steps,
    )
    .expect("large definition");
    let mut state = empty_state();
    loop {
        let definition = named_seed_definition(&state.workflows, &large).expect("unique name");
        state.workflows.push(WorkflowRecord {
            id: unused_identifier(&state).expect("id"),
            revision: 1,
            definition_version: definition.version(),
            definition,
            created_at_ms: 1,
            updated_at_ms: 1,
        });
        if matches!(encode_state(&state), Err(CatalogueError::Full)) {
            state.workflows.pop();
            break;
        }
    }
    assert!(state.workflows.len() < MAXIMUM_WORKFLOWS);
    persist(Some(&path), &state).expect("persist existing catalogue");
    let before = std::fs::read(&path).expect("existing bytes");
    let starter = WorkflowSeed {
        key: SeedKey::parse("large-starter-v1").expect("seed key"),
        definition: large,
    };
    let reopened = WorkflowCatalogue::open_with_seeds(path.clone(), &[starter]).expect("reopen");
    assert_eq!(reopened.list().len(), state.workflows.len());
    assert_eq!(reopened.applied_seed_count(), 0);
    assert_eq!(reopened.unavailable_starters(), vec!["Large starter"]);
    assert_eq!(std::fs::read(path).expect("retained bytes"), before);
}

#[test]
fn create_rejects_duplicate_names_without_case_differences() {
    let catalogue = WorkflowCatalogue::in_memory();
    catalogue.create(named("Planner")).expect("create");
    assert_eq!(
        catalogue.create(named("planner")).err(),
        Some(CatalogueError::DuplicateName)
    );
}

#[test]
fn identical_updates_do_not_change_revision_or_version() {
    let catalogue = WorkflowCatalogue::in_memory();
    let created = catalogue.create(named("Planner")).expect("create");
    let updated = catalogue
        .update(&created.id, created.revision, named("Planner"))
        .expect("update");
    assert_eq!(updated.revision, created.revision);
    assert_eq!(updated.definition_version, created.definition_version);
    assert_eq!(updated.updated_at_ms, created.updated_at_ms);
}

#[test]
fn a_material_update_increments_revision_and_version() {
    let catalogue = WorkflowCatalogue::in_memory();
    let created = catalogue.create(named("Planner")).expect("create");
    let updated = catalogue
        .update(&created.id, created.revision, named("Reviewer"))
        .expect("update");
    assert_eq!(updated.revision, created.revision + 1);
    assert_ne!(updated.definition_version, created.definition_version);
    assert_eq!(updated.definition.name(), "Reviewer");
}

#[test]
fn stale_updates_and_deletes_conflict() {
    let catalogue = WorkflowCatalogue::in_memory();
    let created = catalogue.create(named("Planner")).expect("create");
    catalogue
        .update(&created.id, created.revision, named("Reviewer"))
        .expect("update");
    assert_eq!(
        catalogue
            .update(&created.id, created.revision, named("Later"))
            .err(),
        Some(CatalogueError::Conflict)
    );
    assert_eq!(
        catalogue.delete(&created.id, created.revision).err(),
        Some(CatalogueError::Conflict)
    );
}

#[test]
fn delete_retires_the_identifier_and_keeps_no_active_record() {
    let catalogue = WorkflowCatalogue::in_memory();
    let created = catalogue.create(named("Planner")).expect("create");
    catalogue
        .delete(&created.id, created.revision)
        .expect("delete");
    assert!(catalogue.get(&created.id).is_none());
    assert!(catalogue.retired_ids().contains(&created.id));
    let later = catalogue.create(named("Planner")).expect("recreate");
    assert_ne!(later.id, created.id);
}

#[test]
fn version_one_definition_hash_survives_reopen() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("workflows.json");
    let id;
    let version;
    {
        let catalogue = WorkflowCatalogue::open_with_seeds(path.clone(), &[]).expect("open");
        let record = catalogue.create(named("Planner")).expect("create");
        id = record.id;
        version = record.definition_version;
    }
    let bytes = std::fs::read(&path).expect("read");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert!(
        json["workflows"][0]["definition"]
            .get("first-step")
            .is_none()
    );
    assert_eq!(json["workflows"][0]["definition-version"], version.as_hex());

    let catalogue = WorkflowCatalogue::open_with_seeds(path, &[]).expect("reopen");
    let loaded = catalogue.get(&id).expect("loaded");
    assert_eq!(loaded.definition.name(), "Planner");
    assert_eq!(loaded.definition_version, version);
}

#[test]
fn corrupt_files_fail_open() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("workflows.json");
    write_catalogue(&path, "{");
    assert_eq!(
        WorkflowCatalogue::open_with_seeds(path, &[]).err(),
        Some(CatalogueError::Corrupt)
    );
}

#[test]
fn overlapping_active_and_retired_identifiers_are_rejected() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("workflows.json");
    let id = WorkflowId::generate().expect("id");
    let definition = named("Planner");
    let json = serde_json::json!({
        "file-version": 1,
        "applied-seeds": [],
        "retired-workflow-ids": [id.as_hex()],
        "workflows": [{
            "id": id.as_hex(),
            "revision": 1,
            "definition-version": definition.version().as_hex(),
            "definition": definition.to_file(),
            "created-at-ms": 1,
            "updated-at-ms": 1
        }]
    });
    write_catalogue(&path, &json.to_string());
    assert_eq!(
        WorkflowCatalogue::open_with_seeds(path, &[]).err(),
        Some(CatalogueError::Corrupt)
    );
}

#[test]
fn invalid_definition_digests_are_rejected() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("workflows.json");
    let id = WorkflowId::generate().expect("id");
    let definition = named("Planner");
    let json = serde_json::json!({
        "file-version": 1,
        "applied-seeds": [],
        "retired-workflow-ids": [],
        "workflows": [{
            "id": id.as_hex(),
            "revision": 1,
            "definition-version": "0".repeat(64),
            "definition": definition.to_file(),
            "created-at-ms": 1,
            "updated-at-ms": 1
        }]
    });
    write_catalogue(&path, &json.to_string());
    assert_eq!(
        WorkflowCatalogue::open_with_seeds(path, &[]).err(),
        Some(CatalogueError::Corrupt)
    );
}

#[test]
fn seed_ledger_entries_must_name_a_known_identifier() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("workflows.json");
    let json = serde_json::json!({
        "file-version": 1,
        "applied-seeds": [{
            "key": "one-agent-v1",
            "workflow-id": WorkflowId::generate().expect("id").as_hex()
        }],
        "retired-workflow-ids": [],
        "workflows": []
    });
    write_catalogue(&path, &json.to_string());
    assert_eq!(
        WorkflowCatalogue::open_with_seeds(path, &[]).err(),
        Some(CatalogueError::Corrupt)
    );
}

#[test]
fn malformed_seed_ledger_identifiers_are_rejected() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("workflows.json");
    let json = serde_json::json!({
        "file-version": 1,
        "applied-seeds": [{
            "key": "one-agent-v1",
            "workflow-id": "not-an-id"
        }],
        "retired-workflow-ids": [],
        "workflows": []
    });
    write_catalogue(&path, &json.to_string());
    assert_eq!(
        WorkflowCatalogue::open_with_seeds(path, &[]).err(),
        Some(CatalogueError::Corrupt)
    );
}

#[test]
fn selection_tokens_reject_malformed_syntax() {
    let id = WorkflowId::generate().expect("id");
    let version = named("Planner").version();
    let token = WorkflowSelection {
        workflow_id: id,
        definition_version: version,
    }
    .as_token();
    assert_eq!(token.len(), SELECTION_TOKEN_BYTES);
    assert_eq!(
        WorkflowSelection::parse(&token),
        Some(WorkflowSelection {
            workflow_id: id,
            definition_version: version
        })
    );
    assert!(WorkflowSelection::parse(&token.to_ascii_uppercase()).is_none());
    assert!(WorkflowSelection::parse(&token.replace(':', "/")).is_none());
    assert!(WorkflowSelection::parse(&format!(" {token}")).is_none());
    assert!(WorkflowSelection::parse(&format!("{token}a")).is_none());
    assert!(WorkflowSelection::parse("").is_none());
    assert!(WorkflowSelection::parse(&id.as_hex()).is_none());
}

#[test]
fn resolve_returns_missing_changed_and_pinned_copies() {
    let catalogue = WorkflowCatalogue::in_memory();
    let created = catalogue.create(named("Planner")).expect("create");
    let selection = WorkflowSelection {
        workflow_id: created.id,
        definition_version: created.definition_version,
    };
    let resolved = catalogue.resolve(&selection).expect("resolve");
    assert_eq!(resolved.pinned.workflow_id, Some(created.id));
    assert_eq!(resolved.pinned.version, created.definition_version);
    assert_eq!(resolved.record_revision, 1);

    let updated = catalogue
        .update(&created.id, created.revision, named("Reviewer"))
        .expect("update");
    assert_eq!(
        catalogue.resolve(&selection).err(),
        Some(ResolveWorkflowError::Changed)
    );
    let current = WorkflowSelection {
        workflow_id: created.id,
        definition_version: updated.definition_version,
    };
    assert!(catalogue.resolve(&current).is_ok());

    catalogue
        .delete(&created.id, updated.revision)
        .expect("delete");
    assert_eq!(
        catalogue.resolve(&current).err(),
        Some(ResolveWorkflowError::Missing)
    );
}

#[test]
fn definition_fits_agent_rejects_unknown_tools_and_stronger_access() {
    let definition = named("Planner");
    assert!(definition_fits_agent(
        &definition,
        &[ToolId::List, ToolId::Read],
        &[("project".to_owned(), AccessMode::ReadWrite)],
        "project"
    ));
    assert!(!definition_fits_agent(
        &definition,
        &[ToolId::Read],
        &[("project".to_owned(), AccessMode::ReadWrite)],
        "project"
    ));
    assert!(!definition_fits_agent(
        &definition,
        &[ToolId::List],
        &[("project".to_owned(), AccessMode::ReadOnly)],
        "project"
    ));
    assert!(!definition_fits_agent(
        &definition,
        &[ToolId::List],
        &[("docs".to_owned(), AccessMode::ReadWrite)],
        "project"
    ));
    assert!(definition_fits_agent(
        &definition,
        &[ToolId::List, ToolId::Read],
        &[("source".to_owned(), AccessMode::ReadWrite)],
        "source"
    ));
}

#[test]
fn command_steps_do_not_require_agent_authority() {
    let role = RoleDefinition::new(
        RoleKey::parse("agent").expect("role"),
        "Coding agent".to_owned(),
        String::new(),
        String::new(),
    )
    .expect("role");
    let authority = AgentAuthority::new(vec![ToolId::List], Vec::new()).expect("authority");
    let definition = WorkflowDefinition::from_parts(
        "Mixed".to_owned(),
        test_environment_id(),
        vec![role],
        vec![
            StepDefinition {
                key: StepKey::parse("status").expect("step"),
                name: "Status".to_owned(),
                inputs: vec![initial_candidate_input()],
                action: StepAction::SystemCommand(SystemCommandStep {
                    environment: StepEnvironment::WorkflowDefault,
                    command: SystemCommandId::RepositoryStatus,
                    required_outputs: Vec::new(),
                }),
                review: None,
            },
            StepDefinition {
                key: StepKey::parse("work").expect("step"),
                name: "Work".to_owned(),
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
            },
        ],
    )
    .expect("definition");
    assert!(definition_fits_agent(
        &definition,
        &[ToolId::List],
        &[("project".to_owned(), AccessMode::ReadWrite)],
        "project"
    ));
}

#[test]
fn earlier_catalogue_file_versions_are_rejected() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("workflows.json");
    let json = serde_json::json!({
        "file-version": 0,
        "applied-seeds": [],
        "retired-workflow-ids": [],
        "workflows": []
    });
    write_catalogue(&path, &json.to_string());
    assert_eq!(
        WorkflowCatalogue::open_with_seeds(path, &[]).err(),
        Some(CatalogueError::Corrupt)
    );
}
