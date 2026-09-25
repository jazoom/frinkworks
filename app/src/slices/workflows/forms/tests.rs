use super::{FormError, FormIntent, PhasePurpose, WorkflowFormState};

fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_owned(), value.to_owned())
}

fn valid_pairs() -> Vec<(String, String)> {
    vec![
        pair("intent", "save"),
        pair("name", "Implementation and review"),
        pair(
            "default-environment",
            &crate::tests::test_environment_id().as_hex(),
        ),
        pair("step_0_name", "Implement the change"),
        pair("step_0_purpose", "implementation"),
        pair("step_0_review-policy", "none"),
        pair("step_1_name", "Review the change"),
        pair("step_1_purpose", "read-only-review"),
        pair("step_1_review-policy", "none"),
    ]
}

#[test]
fn authoring_rejects_obsolete_modes_and_actions() {
    for field in ["execution-mode", "commit-policy", "shell"] {
        let mut pairs = valid_pairs();
        pairs.push(pair(field, "once"));
        assert!(WorkflowFormState::parse(pairs).is_err());
    }
    for intent in [
        "update-mode",
        "add-phase:saved-plan-implementation",
        "add-phase:code-approval",
        "add-phase:commit",
        "explode",
    ] {
        let mut pairs = valid_pairs();
        pairs[0] = pair("intent", intent);
        assert!(WorkflowFormState::parse(pairs).is_err());
    }
}

#[test]
fn duplicate_sparse_and_malformed_fields_are_rejected() {
    for (field, error) in [
        ("name", FormError::DuplicateField),
        ("step_3_name", FormError::Sparse),
        ("step_01_name", FormError::Index),
        ("step_0_tool_destroy", FormError::UnknownField),
    ] {
        let mut pairs = valid_pairs();
        pairs.push(pair(field, "extra"));
        assert_eq!(WorkflowFormState::parse(pairs).err(), Some(error));
    }
}

#[test]
fn purpose_phases_preserve_keys_and_optional_connections() {
    let mut pairs = valid_pairs();
    pairs.extend([
        pair("step_0_output_0_key", "notes"),
        pair("step_0_output_0_kind", "plan"),
        pair("step_1_input_0_key", "requirements"),
        pair("step_1_input_0_kind", "plan"),
        pair("step_1_input_0_source", "step-output:phase-1:notes"),
    ]);
    let (mut form, _) = WorkflowFormState::parse(pairs).unwrap();
    let key = form.steps[0].key.clone();
    form.steps[0].name = "New name".into();
    form.steps[0].tools.clear();
    super::normalise_phase_contracts(&mut form.steps);
    let definition = form.to_definition().unwrap();
    assert_eq!(form.steps[0].key, key);
    assert!(
        definition.steps()[1]
            .inputs
            .iter()
            .any(|input| input.key.as_str() == "requirements")
    );
    assert!(!super::can_remove_step(&form.steps, 0));
    let crate::workflows::definition::StepAction::Agent(action) = &definition.steps()[0].action
    else {
        panic!("agent")
    };
    assert!(action.authority.tools.is_empty());
    form.steps[1].inputs[0].source = "step-output:missing:notes".into();
    assert!(form.to_definition().is_err());
}

#[test]
fn plan_checkpoint_requires_plan_provenance() {
    let (mut form, _) = WorkflowFormState::parse(valid_pairs()).unwrap();
    form.apply(FormIntent::SetPhasePurpose {
        step: 0,
        purpose: PhasePurpose::Planning,
    })
    .unwrap();
    form.apply(FormIntent::AddPhase(PhasePurpose::PlanReview))
        .unwrap();
    form.apply(FormIntent::AddPhase(PhasePurpose::PlanCheckpoint))
        .unwrap();
    form.apply(FormIntent::AddPhase(PhasePurpose::Implementation))
        .unwrap();
    let definition = form.to_definition().unwrap();
    assert!(
        matches!(&definition.steps()[3].action, crate::workflows::definition::StepAction::HumanGate(action) if action.is_plan_checkpoint())
    );
    assert!(
        definition.steps()[4]
            .inputs
            .iter()
            .any(|input| input.kind == crate::workflows::definition::ArtefactKind::PlanDecision)
    );
}

#[test]
fn custom_settings_reject_unknown_model_sources_and_providers() {
    for (source, provider) in [("override", "not-a-provider"), ("bogus", "xai")] {
        let mut pairs = valid_pairs();
        pairs.extend([
            pair("step_0_settings-source", source),
            pair("step_0_provider", provider),
            pair("step_0_model", "model"),
            pair("step_0_network", "none"),
        ]);
        let (form, _) = WorkflowFormState::parse(pairs).unwrap();
        assert!(form.to_definition().is_err());
    }
}

#[test]
fn execution_preview_preserves_explicit_host_permissions_and_form_values() {
    let mut pairs = valid_pairs();
    pairs[0] = pair("intent", "preview-execution");
    pairs.extend([
        pair("step_0_settings-source", "override"),
        pair("step_0_location", "host"),
        pair("step_0_read-only", "/read"),
        pair("step_0_direct", "/write"),
    ]);
    let (mut form, intent) = WorkflowFormState::parse(pairs).unwrap();
    assert_eq!(intent, FormIntent::PreviewExecution);
    form.apply(intent).unwrap();
    assert_eq!(form.name, "Implementation and review");
    assert_eq!(form.steps[0].location, "host");
    assert_eq!(form.steps[0].settings_read_only, "/read");
    assert_eq!(form.steps[0].settings_direct, "/write");
}

#[test]
fn host_overrides_require_explicit_write_directories() {
    use crate::workflows::definition::{ModelStepSettings, StepAction};
    let root = tempfile::tempdir().unwrap();
    for (field, accepted) in [("direct", true), ("read-only", false)] {
        let mut pairs = valid_pairs();
        pairs.extend([
            pair("step_0_settings-source", "override"),
            pair("step_0_inherit-model", "1"),
            pair("step_0_inherit-environment", "1"),
            pair("step_0_inherit-network", "1"),
            pair("step_0_location", "host"),
            pair(&format!("step_0_{field}"), root.path().to_str().unwrap()),
        ]);
        let (form, _) = WorkflowFormState::parse(pairs).unwrap();
        let result = form.to_definition();
        assert_eq!(result.is_ok(), accepted);
        if let Ok(definition) = result {
            let StepAction::Agent(action) = &definition.steps()[0].action else {
                panic!("agent")
            };
            let ModelStepSettings::Override(overrides) = &action.settings else {
                panic!("override")
            };
            let mut settings = crate::workflows::tests::settings();
            settings.location = overrides.location.unwrap();
            settings.directories = overrides.directories.clone().unwrap();
            assert!(settings.host_access_allowed());
        }
    }
}

#[test]
fn directory_fields_retain_write_and_reject_overlap() {
    use crate::workflows::definition::{ModelStepSettings, StepAction};
    let root = tempfile::tempdir().unwrap();
    let mut pairs = valid_pairs();
    pairs.extend([
        pair("step_0_settings-source", "override"),
        pair("step_0_inherit-model", "1"),
        pair("step_0_inherit-environment", "1"),
        pair("step_0_inherit-network", "1"),
        pair("step_0_direct", root.path().to_str().unwrap()),
    ]);
    let (mut form, _) = WorkflowFormState::parse(pairs).unwrap();
    let definition = form.to_definition().unwrap();
    let StepAction::Agent(action) = &definition.steps()[0].action else {
        panic!("agent")
    };
    let ModelStepSettings::Override(settings) = &action.settings else {
        panic!("override")
    };
    assert_eq!(
        settings.directories.as_ref().unwrap()[0].access,
        crate::execution::DirectoryAccess::Write
    );
    form.steps[0].settings_read_only = root.path().display().to_string();
    assert!(form.to_definition().is_err());
}

#[test]
fn purpose_change_keeps_invalid_user_values_for_field_errors() {
    let (mut form, _) = WorkflowFormState::parse(valid_pairs()).unwrap();
    form.apply(FormIntent::AddPhase(PhasePurpose::PlanCheckpoint))
        .unwrap();
    form.apply(FormIntent::SetPhasePurpose {
        step: 2,
        purpose: PhasePurpose::Planning,
    })
    .unwrap();
    form.to_definition().unwrap();
    assert_ne!(form.steps[0].role, form.steps[2].role);
    form.steps[0].name.clear();
    form.steps[1].purpose = "unsupported-purpose".into();
    super::normalise_phase_contracts(&mut form.steps);
    let errors = form.to_definition().unwrap_err();
    assert!(!errors.steps[0].name.is_empty());
    assert!(!errors.steps[1].purpose.is_empty());
    assert!(form.steps[0].name.is_empty());
}

#[test]
fn malformed_environment_and_obsolete_artefacts_are_rejected() {
    let (mut form, _) = WorkflowFormState::parse(valid_pairs()).unwrap();
    form.default_environment = "not-an-id".into();
    assert!(form.to_definition().is_err());
    for kind in ["candidate-revision", "human-decision"] {
        let mut pairs = valid_pairs();
        pairs.extend([
            pair("step_0_output_0_key", "obsolete"),
            pair("step_0_output_0_kind", kind),
        ]);
        let (form, _) = WorkflowFormState::parse(pairs).unwrap();
        assert!(form.to_definition().is_err());
    }
}
