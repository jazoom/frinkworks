use super::{
    ONE_AGENT_V1, PLAN_A_CHANGE_V1, SEQUENTIAL_TEAM_V1, SeedKey, WorkflowSeed,
    correctness_security_definition, implement_and_review_definition,
    implement_with_approval_definition, one_agent_definition, plan_a_change_definition,
    plan_then_implement_definition, review_current_code_definition,
    review_until_approved_definition, review_with_fixes_definition, sequential_team_definition,
};
use crate::agents::ToolId;
use crate::tests::test_environment_id;
use crate::workflows::catalogue::WorkflowCatalogue;
use crate::workflows::commands::{CommandSourceEffect, SystemCommandId};
use crate::workflows::definition::{
    ArtefactKind, ArtefactSource, OutputKind, StepAction, StepEnvironment, WorkflowDefinition,
};

fn named(name: &str) -> WorkflowDefinition {
    let definition = one_agent_definition(test_environment_id());
    WorkflowDefinition::from_parts(
        name.to_owned(),
        test_environment_id(),
        definition.roles().to_vec(),
        definition.steps().to_vec(),
    )
    .expect("named")
}

fn names(catalogue: &WorkflowCatalogue) -> Vec<String> {
    let mut names: Vec<_> = catalogue
        .list()
        .into_iter()
        .map(|record| record.definition.name().to_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn first_open_seeds_ordinary_workflows_once() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("workflows.json");
    let first = WorkflowCatalogue::open(path.clone(), test_environment_id()).expect("open");
    assert_eq!(
        names(&first),
        vec![
            "Implement and review".to_owned(),
            "Implement with approval".to_owned(),
            "Plan a change".to_owned(),
            "Plan then implement".to_owned(),
            "Review current code".to_owned(),
            "Task loop".to_owned(),
        ]
    );
    assert_eq!(first.applied_seed_count(), 6);
    let ids: Vec<_> = first.list().into_iter().map(|record| record.id).collect();
    let second = WorkflowCatalogue::open(path, test_environment_id()).expect("reopen");
    assert_eq!(second.list().len(), 6);
    assert_eq!(second.applied_seed_count(), 6);
    let reopened: Vec<_> = second.list().into_iter().map(|record| record.id).collect();
    assert_eq!(reopened, ids);
}

#[test]
fn restart_preserves_an_edited_seeded_workflow() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("workflows.json");
    let catalogue = WorkflowCatalogue::open(path.clone(), test_environment_id()).expect("open");
    let seeded = catalogue
        .list()
        .into_iter()
        .find(|record| record.definition.name() == "Plan a change")
        .expect("seed");
    catalogue
        .update(&seeded.id, seeded.revision, named("Edited plan"))
        .expect("edit");
    let reopened = WorkflowCatalogue::open(path, test_environment_id()).expect("reopen");
    let loaded = reopened.get(&seeded.id).expect("loaded");
    assert_eq!(loaded.definition.name(), "Edited plan");
    assert_eq!(reopened.applied_seed_count(), 6);
}

#[test]
fn restart_does_not_restore_a_deleted_seeded_workflow() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("workflows.json");
    let catalogue = WorkflowCatalogue::open(path.clone(), test_environment_id()).expect("open");
    let seeded = catalogue
        .list()
        .into_iter()
        .find(|record| record.definition.name() == "Plan a change")
        .expect("seed");
    catalogue
        .delete(&seeded.id, seeded.revision)
        .expect("delete");
    let remaining = vec![
        "Implement and review".to_owned(),
        "Implement with approval".to_owned(),
        "Plan then implement".to_owned(),
        "Review current code".to_owned(),
        "Task loop".to_owned(),
    ];
    assert_eq!(names(&catalogue), remaining);
    assert!(catalogue.retired_ids().contains(&seeded.id));
    assert_eq!(catalogue.applied_seed_count(), 6);
    let reopened = WorkflowCatalogue::open(path, test_environment_id()).expect("reopen");
    assert_eq!(names(&reopened), remaining);
    assert!(reopened.retired_ids().contains(&seeded.id));
    assert_eq!(reopened.applied_seed_count(), 6);
}

#[test]
fn a_present_seed_key_is_not_reapplied_from_code() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("workflows.json");
    let catalogue = WorkflowCatalogue::open_with_seeds(
        path.clone(),
        &[WorkflowSeed {
            key: SeedKey::parse(PLAN_A_CHANGE_V1).expect("key"),
            definition: named("Custom"),
        }],
    )
    .expect("open");
    assert_eq!(catalogue.list()[0].definition.name(), "Custom");
    let reopened = WorkflowCatalogue::open(path, test_environment_id()).expect("production reopen");
    assert_eq!(
        names(&reopened),
        vec![
            "Custom".to_owned(),
            "Implement and review".to_owned(),
            "Implement with approval".to_owned(),
            "Plan then implement".to_owned(),
            "Review current code".to_owned(),
            "Task loop".to_owned(),
        ]
    );
}

#[test]
fn a_name_collision_creates_a_suffixed_ordinary_record() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("workflows.json");
    let catalogue = WorkflowCatalogue::open_with_seeds(path.clone(), &[]).expect("open");
    let user = catalogue
        .create(named("Plan a change"))
        .expect("user record");
    let reopened = WorkflowCatalogue::open(path, test_environment_id()).expect("seed later");
    let records = reopened.list();
    assert!(
        records
            .iter()
            .any(|record| record.id == user.id && record.definition.name() == "Plan a change")
    );
    assert!(
        records
            .iter()
            .any(|record| record.definition.name() == "Plan a change 2")
    );
    assert!(
        records
            .iter()
            .any(|record| record.definition.name() == "Implement and review")
    );
}

#[test]
fn production_seed_keys_are_stable() {
    let seeds = super::production_seeds(test_environment_id());
    assert_eq!(
        seeds
            .iter()
            .map(|seed| seed.key.as_str())
            .collect::<Vec<_>>(),
        vec![
            "plan-a-change-v1",
            "review-current-code-v1",
            "implement-with-approval-v1",
            "implement-and-review-v1",
            "plan-then-implement-v1",
            "task-loop-v1",
        ]
    );
}

#[test]
fn plan_and_review_starters_are_read_only() {
    let environment = test_environment_id();
    for definition in [
        plan_a_change_definition(environment),
        review_current_code_definition(environment),
    ] {
        assert!(definition.steps().iter().all(|step| {
            !step.writes_primary_source()
                && !matches!(
                    &step.action,
                    StepAction::SystemCommand(command)
                        if command.command == SystemCommandId::CommitCandidate
                )
                && !matches!(step.action, StepAction::HumanGate(_))
        }));
    }
    assert!(
        plan_a_change_definition(environment).steps()[0]
            .required_outputs()
            .iter()
            .any(|output| output.kind == OutputKind::Plan)
    );
    assert!(
        review_current_code_definition(environment).steps()[0]
            .required_outputs()
            .iter()
            .any(|output| output.kind == OutputKind::ReviewReport)
    );
}

#[test]
fn code_changing_starters_require_human_approval_before_application() {
    let environment = test_environment_id();
    let approval = implement_with_approval_definition(environment);
    assert!(matches!(
        approval.steps()[1].action,
        StepAction::HumanGate(_)
    ));
    assert!(matches!(
        approval.steps()[2].action,
        StepAction::SystemCommand(ref command)
            if command.command == SystemCommandId::ApplyChanges
    ));
    assert!(
        approval.steps()[2]
            .inputs
            .iter()
            .any(|input| input.kind == ArtefactKind::HumanDecision)
    );

    let reviewed = implement_and_review_definition(environment);
    assert!(matches!(
        reviewed.steps()[2].action,
        StepAction::HumanGate(_)
    ));
    let commit = &reviewed.steps()[3];
    assert!(
        commit
            .inputs
            .iter()
            .any(|input| input.kind == ArtefactKind::ReviewReport)
    );
    assert!(
        commit
            .inputs
            .iter()
            .any(|input| input.kind == ArtefactKind::HumanDecision)
    );
}

#[test]
fn plan_then_implement_accepts_a_plan_before_code_and_keeps_code_approval_separate() {
    let definition = plan_then_implement_definition(test_environment_id());
    let checkpoint = definition
        .steps()
        .iter()
        .find(|step| step.name == "Accept the plan")
        .expect("plan checkpoint");
    let StepAction::HumanGate(action) = &checkpoint.action else {
        panic!("plan checkpoint action");
    };
    assert_eq!(action.required_output.kind, OutputKind::PlanDecision);
    assert!(
        checkpoint
            .inputs
            .iter()
            .all(|input| input.kind == ArtefactKind::Plan)
    );
    let implementation = definition
        .steps()
        .iter()
        .find(|step| step.name == "Implement the accepted plan")
        .expect("implementation");
    assert!(
        implementation
            .inputs
            .iter()
            .any(|input| input.kind == ArtefactKind::PlanDecision)
    );
    let commit = definition.steps().last().expect("commit");
    assert!(
        commit
            .inputs
            .iter()
            .any(|input| input.kind == ArtefactKind::HumanDecision)
    );
    assert!(
        !commit
            .inputs
            .iter()
            .any(|input| input.kind == ArtefactKind::PlanDecision)
    );
    assert!(
        definition
            .with_commit_policy(crate::workflows::definition::CommitPolicy::AutomaticAfterReview)
            .is_err()
    );
}

#[test]
fn starter_authority_effects_match_their_code_effects() {
    let environment = test_environment_id();
    let read_only = [ToolId::List, ToolId::Read, ToolId::Run];
    let readonly_grant = [("project".to_owned(), crate::agents::AccessMode::ReadOnly)];
    for definition in [
        plan_a_change_definition(environment),
        review_current_code_definition(environment),
    ] {
        assert!(crate::workflows::catalogue::definition_fits_agent(
            &definition,
            &read_only,
            &readonly_grant,
            "project"
        ));
    }
    assert!(!crate::workflows::catalogue::definition_fits_agent(
        &implement_with_approval_definition(environment),
        &read_only,
        &readonly_grant,
        "project"
    ));
    let writable_grant = [("project".to_owned(), crate::agents::AccessMode::ReadWrite)];
    assert!(crate::workflows::catalogue::definition_fits_agent(
        &implement_and_review_definition(environment),
        &ToolId::ALL,
        &writable_grant,
        "project"
    ));
}

#[test]
fn sequential_team_uses_the_supplied_environment_without_seed_provenance() {
    let environment = test_environment_id();
    let definition = sequential_team_definition(environment);
    assert_eq!(definition.default_environment(), environment);
    assert!(
        definition
            .steps()
            .iter()
            .all(|step| step.environment() == Some(StepEnvironment::WorkflowDefault))
    );
    let bytes = serde_json::to_vec(&definition.to_file()).expect("json");
    let text = String::from_utf8(bytes).expect("utf8");
    assert!(!text.contains(SEQUENTIAL_TEAM_V1));
    assert!(!text.contains(ONE_AGENT_V1));
    assert!(!text.contains("built_in"));
    assert!(!text.contains("built-in"));
}

#[test]
fn sequential_team_authority_and_handoff_match_the_validator() {
    let definition = sequential_team_definition(test_environment_id());
    assert_eq!(definition.roles().len(), 3);
    assert_eq!(definition.steps().len(), 4);
    assert_eq!(definition.first_step().as_str(), "planner");

    let planner = definition.step(definition.first_step()).expect("planner");
    let StepAction::Agent(action) = &planner.action else {
        panic!("planner");
    };
    assert_eq!(
        action.authority.tools,
        vec![ToolId::List, ToolId::Read, ToolId::Run]
    );
    assert_eq!(
        action.candidate_authority,
        crate::workflows::definition::CandidateAuthority::ReadOnly
    );
    assert!(action.authority.directories.is_empty());
    assert_eq!(
        planner.inputs[0].source,
        ArtefactSource::RunInitialCandidate
    );
    assert!(
        action
            .required_outputs
            .iter()
            .any(|output| output.kind == OutputKind::Plan)
    );

    let implementer = definition
        .step(&crate::workflows::definition::StepKey::parse("implementer").expect("key"))
        .expect("implementer");
    let StepAction::Agent(action) = &implementer.action else {
        panic!("implementer");
    };
    assert_eq!(action.authority.tools, ToolId::ALL.to_vec());
    assert_eq!(
        action.candidate_authority,
        crate::workflows::definition::CandidateAuthority::Edit
    );
    assert!(action.authority.directories.is_empty());
    assert!(implementer.inputs.iter().any(|input| {
        input.kind == ArtefactKind::Plan
            && matches!(
                input.source,
                ArtefactSource::StepOutput { ref step, .. } if step.as_str() == "planner"
            )
    }));

    let reviewer = definition
        .step(&crate::workflows::definition::StepKey::parse("reviewer").expect("key"))
        .expect("reviewer");
    let StepAction::Agent(action) = &reviewer.action else {
        panic!("reviewer");
    };
    assert_eq!(
        action.candidate_authority,
        crate::workflows::definition::CandidateAuthority::ReadOnly
    );
    assert!(action.authority.directories.is_empty());
    assert!(
        action
            .required_outputs
            .iter()
            .any(|output| output.kind == OutputKind::ReviewReport)
    );

    let commit = definition
        .step(&crate::workflows::definition::StepKey::parse("commit").expect("key"))
        .expect("commit");
    let StepAction::SystemCommand(action) = &commit.action else {
        panic!("commit");
    };
    assert_eq!(action.command, SystemCommandId::CommitCandidate);
    assert_eq!(
        commit.command_source_effect(),
        Some(CommandSourceEffect::Commit)
    );
    assert_eq!(
        action.required_outputs[0].key.as_str(),
        "committed-candidate"
    );
}

#[test]
fn review_with_fixes_uses_exact_independent_review_edges() {
    let definition = review_with_fixes_definition(test_environment_id());
    let independent = definition
        .step(&crate::workflows::definition::StepKey::parse("independent-reviewer").expect("key"))
        .expect("independent reviewer");
    assert!(independent.inputs.iter().any(|input| {
        input.kind == ArtefactKind::CandidateRevision
            && matches!(
                &input.source,
                ArtefactSource::StepOutput { step, output }
                    if step.as_str() == "fixing-reviewer" && output.as_str() == "candidate"
            )
    }));
    assert!(independent.inputs.iter().any(|input| {
        input.kind == ArtefactKind::ReviewReport
            && matches!(
                &input.source,
                ArtefactSource::StepOutput { step, output }
                    if step.as_str() == "fixing-reviewer" && output.as_str() == "review"
            )
    }));
    let commit = definition
        .step(&crate::workflows::definition::StepKey::parse("commit").expect("key"))
        .expect("commit");
    assert!(commit.inputs.iter().any(|input| {
        input.kind == ArtefactKind::CandidateRevision
            && matches!(
                &input.source,
                ArtefactSource::StepOutput { step, output }
                    if step.as_str() == "fixing-reviewer" && output.as_str() == "candidate"
            )
    }));
    assert!(commit.inputs.iter().any(|input| {
        input.kind == ArtefactKind::ReviewReport
            && matches!(
                &input.source,
                ArtefactSource::StepOutput { step, output }
                    if step.as_str() == "independent-reviewer" && output.as_str() == "review"
            )
    }));
}

#[test]
fn review_until_approved_loops_to_the_implementer_and_hands_off_its_report() {
    let definition = review_until_approved_definition(test_environment_id());
    let reviewer = definition
        .step(&crate::workflows::definition::StepKey::parse("reviewer").expect("key"))
        .expect("reviewer");
    let policy = reviewer.review.as_ref().expect("review policy");
    assert_eq!(policy.revision_target.as_str(), "implementer");
    assert_eq!(
        definition.next_step(&reviewer.key).map(|key| key.as_str()),
        Some("commit")
    );
    assert_eq!(policy.attempt_limit, 3);
    assert!(reviewer.inputs.iter().any(|input| {
        input.kind == ArtefactKind::CandidateRevision
            && input.source == ArtefactSource::RunCurrentCandidate
    }));
    let commit = definition
        .step(&crate::workflows::definition::StepKey::parse("commit").expect("key"))
        .expect("commit");
    assert!(commit.inputs.iter().any(|input| {
        input.kind == ArtefactKind::ReviewReport
            && matches!(&input.source, ArtefactSource::StepOutput { step, output } if step.as_str() == "reviewer" && output.as_str() == "review")
    }));
}

#[test]
fn correctness_and_security_review_hands_both_current_reports_to_commit() {
    let definition = correctness_security_definition(test_environment_id());
    for (step_key, phase, approved) in [
        ("correctness-review", 1, "security-review"),
        ("security-review", 2, "commit"),
    ] {
        let key = crate::workflows::definition::StepKey::parse(step_key).expect("key");
        let step = definition.step(&key).expect("review step");
        let policy = step.review.as_ref().expect("review policy");
        assert_eq!(definition.review_phase(&key), Some(phase));
        assert_eq!(policy.revision_target.as_str(), "implementer");
        assert_eq!(
            definition.next_step(&key).map(|target| target.as_str()),
            Some(approved)
        );
    }
    let commit = definition
        .step(&crate::workflows::definition::StepKey::parse("commit").expect("key"))
        .expect("commit");
    let report_sources: Vec<_> = commit
        .inputs
        .iter()
        .filter(|input| input.kind == ArtefactKind::ReviewReport)
        .filter_map(|input| match &input.source {
            ArtefactSource::StepOutput { step, output } => Some((step.as_str(), output.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        report_sources,
        [
            ("correctness-review", "review"),
            ("security-review", "review")
        ]
    );
}

#[test]
fn scheduler_inputs_contain_no_seed_provenance() {
    let definition = one_agent_definition(test_environment_id());
    let bytes = serde_json::to_vec(&definition.to_file()).expect("json");
    let text = String::from_utf8(bytes).expect("utf8");
    assert!(!text.contains(ONE_AGENT_V1));
    assert!(!text.contains("built_in"));
    assert!(!text.contains("built-in"));
    assert!(matches!(definition.steps()[0].action, StepAction::Agent(_)));
}
