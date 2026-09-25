use super::*;

pub(crate) fn test_agent_capabilities() -> AttemptCapabilities {
    AttemptCapabilities {
        schema: CAPABILITY_SCHEMA,
        agent_revision: 1,
        tools: vec![ToolId::List],
        directories: Vec::new(),
        source_location: PrimarySourceLocation::PrivateWorkspace,
        git_admin: AccessMode::ReadOnly,
        network: NetworkCapability::None,
    }
}

#[test]
fn step_access_intersects_live_directory_permissions() {
    let root = tempfile::tempdir().unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(root.path(), &[]).unwrap();
    for access in [
        crate::execution::DirectoryAccess::Read,
        crate::execution::DirectoryAccess::Write,
    ] {
        let mut settings = crate::workflows::tests::settings();
        settings.directories = vec![crate::execution::DirectoryGrant {
            access,
            ..grant.clone()
        }];
        for definition in [
            crate::workflows::seeds::plan_a_change_definition(settings.environment),
            crate::workflows::seeds::implement_a_change_definition(settings.environment),
        ] {
            let definition = definition.with_conversation_settings(&settings).unwrap();
            let authority =
                crate::execution::ProjectFreeAuthority::from_settings(1, &settings).unwrap();
            let step = &definition.steps()[0];
            let caps = AttemptCapabilities::derive_project_free(step, &authority).unwrap();
            assert_eq!(
                caps.directories[0].access.is_writable(),
                access == crate::execution::DirectoryAccess::Write && step.writes_primary_source()
            );
            assert!(caps.tools.contains(&ToolId::Run));
            assert_eq!(caps.source_location, PrimarySourceLocation::UserProject);
        }
    }
}

#[test]
fn undeclared_tools_and_directory_escalation_are_rejected() {
    let settings = crate::workflows::tests::settings();
    let mut authority =
        crate::execution::ProjectFreeAuthority::from_settings(1, &settings).unwrap();
    authority.tools.clear();
    let definition = crate::workflows::seeds::implement_a_change_definition(settings.environment);
    assert_eq!(
        AttemptCapabilities::derive_project_free(&definition.steps()[0], &authority),
        Err(CapabilityError::Authority)
    );
}
