use super::*;

#[test]
fn launch_forms_reject_retired_commit_policy_and_duplicate_fields() {
    for fields in [
        vec![
            ("revision".into(), "1".into()),
            ("workflow".into(), "x".into()),
            ("brief".into(), "Task".into()),
            ("commit_policy".into(), "automatic-after-review".into()),
        ],
        vec![
            ("revision".into(), "1".into()),
            ("revision".into(), "2".into()),
        ],
    ] {
        assert!(parse_fields::<WorkflowLaunchForm>(fields).is_err());
    }
}

#[test]
fn read_review_never_inherits_live_write_permissions() {
    let root = tempfile::tempdir().unwrap();
    let mut grant = crate::execution::DirectoryGrant::from_selected(root.path(), &[]).unwrap();
    grant.access = crate::execution::DirectoryAccess::Write;
    let mut settings = workflows::tests::settings();
    settings.directories = vec![grant];
    settings.host_approval = crate::execution::HostApprovalPolicy::Automatic;
    let definition = workflows::seeds::implement_and_review_definition(settings.environment)
        .with_conversation_settings(&settings)
        .unwrap();
    let authority = crate::execution::ProjectFreeAuthority::from_settings(1, &settings).unwrap();
    for (index, step) in definition.steps().iter().enumerate() {
        let caps =
            workflows::capabilities::AttemptCapabilities::derive_project_free(step, &authority)
                .unwrap();
        assert_eq!(caps.directories[0].access.is_writable(), index == 0);
    }
    assert_eq!(definition.steps().len(), 2);
}

#[test]
fn additional_access_includes_yolo_and_host_expansion() {
    let defaults = workflows::tests::settings();
    let mut automatic = defaults.clone();
    automatic.host_approval = crate::execution::HostApprovalPolicy::Automatic;
    assert!(workflows::definition::additional_access(
        &defaults, &automatic
    ));
    let mut host = defaults.clone();
    host.location = crate::execution::ToolLocation::Host;
    assert!(workflows::definition::additional_access(&defaults, &host));
}

#[test]
fn malformed_and_foreign_phase_tokens_do_not_resolve() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let definition =
        workflows::seeds::plan_a_change_definition(crate::tests::test_environment_id());
    for token in ["not-json", "{}", "{\"step\":\"foreign\"}"] {
        assert!(resolve_phase_models(&state, &definition, &[token.to_owned()]).is_err());
    }
}
