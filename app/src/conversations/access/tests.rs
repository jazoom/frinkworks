use super::*;

fn resolve_grant(
    grant: &ConversationGrant,
    project: &ProjectRecord,
    conversation_id: crate::conversations::ConversationId,
    conversation_revision: u32,
    preset: Option<&crate::agents::AgentRecord>,
) -> Result<ConversationAuthority, ConversationAccessError> {
    resolve_grant_with_context(
        grant,
        project,
        conversation_id,
        conversation_revision,
        NetworkAccess::None,
        Vec::new(),
        preset,
    )
}
use crate::agents::{
    AccessMode, AgentId, AgentRecord, DirectoryGrant, NetworkAccess, ToolId, guest_path_for,
};
use crate::projects::{ProjectId, ProjectRecord, ProjectStore};

fn project(path: &std::path::Path) -> ProjectRecord {
    ProjectRecord {
        id: ProjectId::generate().expect("project"),
        revision: 4,
        name: "Project".to_owned(),
        host_path: path.to_path_buf(),
        created_at_ms: 1,
    }
}

fn preset(path: &std::path::Path, tools: Vec<ToolId>, network: NetworkAccess) -> AgentRecord {
    AgentRecord {
        id: AgentId::generate().expect("agent"),
        revision: 2,
        name: "Preset".to_owned(),
        instructions: String::new(),
        selection: None,
        tools,
        network,
        directories: vec![DirectoryGrant {
            alias: "project".to_owned(),
            host_path: path.to_path_buf(),
            access: AccessMode::ReadOnly,
        }],
        primary_directory: "project".to_owned(),
    }
}

#[test]
fn phase_snapshots_reject_expansions_omitted_sources_and_replaced_roots() {
    let tree = tempfile::tempdir().unwrap();
    let path = tree.path().join("source");
    std::fs::create_dir(&path).unwrap();
    let project = project(&path);
    let grant = ConversationGrant {
        project_id: project.id,
        project_revision: project.revision,
        authority_revision: 1,
        access: AccessMode::ReadOnly,
    };
    let authority = resolve_grant(
        &grant,
        &project,
        crate::conversations::ConversationId::generate().unwrap(),
        1,
        None,
    )
    .unwrap()
    .effective;
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(
            crate::providers::ProviderKind::Xai,
            "grok-4.6".to_owned(),
            None,
        )
        .unwrap(),
        String::new(),
        vec![ToolId::Read],
        crate::environments::EnvironmentId::generate().unwrap(),
    )
    .unwrap();
    assert!(apply_settings_ceiling(&authority, &settings).is_err());
    let settings = settings
        .with_directories(vec![
            crate::execution::DirectoryGrant::from_selected(&path, &[]).unwrap(),
        ])
        .unwrap();
    assert!(apply_settings_ceiling(&authority, &settings).is_ok());
    let mut expanded = settings.clone();
    expanded.network = NetworkAccess::Public;
    assert!(apply_settings_ceiling(&authority, &expanded).is_err());
    expanded = settings.clone();
    expanded.tools.push(ToolId::Write);
    assert!(apply_settings_ceiling(&authority, &expanded).is_err());
    expanded = settings.clone();
    expanded.directories[0].access = crate::execution::DirectoryAccess::ReviewBeforeApply;
    assert!(apply_settings_ceiling(&authority, &expanded).is_err());
    std::fs::rename(&path, tree.path().join("original")).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(apply_settings_ceiling(&authority, &settings).is_err());
}

#[test]
fn a_direct_conversation_grant_has_the_read_only_ceiling() {
    let directory = tempfile::tempdir().expect("directory");
    let project = project(directory.path());
    let conversation = crate::conversations::ConversationId::generate().expect("conversation");
    let grant = ConversationGrant {
        project_id: project.id,
        project_revision: project.revision,
        authority_revision: 1,
        access: AccessMode::ReadOnly,
    };
    let authority = resolve_grant(&grant, &project, conversation, 1, None)
        .expect("authority")
        .effective;

    assert_eq!(
        authority.tools,
        vec![ToolId::List, ToolId::Read, ToolId::Run]
    );
    assert_eq!(authority.grant_access, AccessMode::ReadOnly);
    assert_eq!(authority.network, NetworkAccess::None);
    assert_eq!(authority.policy.primary_guest(), "/project");
}

#[test]
fn an_empty_preset_tool_ceiling_blocks_guest_tools_without_granting_network() {
    let directory = tempfile::tempdir().expect("directory");
    let project = project(directory.path());
    let preset = preset(directory.path(), Vec::new(), NetworkAccess::Public);
    let grant = ConversationGrant {
        project_id: project.id,
        project_revision: project.revision,
        authority_revision: 1,
        access: AccessMode::ReadOnly,
    };
    let authority = resolve_grant(
        &grant,
        &project,
        crate::conversations::ConversationId::generate().expect("conversation"),
        1,
        Some(&preset),
    )
    .expect("authority")
    .effective;

    assert!(authority.tools.is_empty());
    assert_eq!(authority.network, NetworkAccess::None);
}

#[test]
fn a_writable_conversation_grant_includes_write_in_the_candidate() {
    let directory = tempfile::tempdir().expect("directory");
    let project = project(directory.path());
    let conversation = crate::conversations::ConversationId::generate().expect("conversation");
    let grant = ConversationGrant {
        project_id: project.id,
        project_revision: project.revision,
        authority_revision: 1,
        access: AccessMode::ReadWrite,
    };
    let authority = resolve_grant(&grant, &project, conversation, 1, None)
        .expect("authority")
        .effective;

    assert_eq!(
        authority.tools,
        vec![ToolId::List, ToolId::Read, ToolId::Run, ToolId::Write]
    );
    assert_eq!(authority.grant_access, AccessMode::ReadWrite);
    assert_eq!(authority.policy.primary_access(), AccessMode::ReadWrite);
    assert_eq!(authority.network, NetworkAccess::None);
}

#[test]
fn a_changed_project_revision_invalidates_the_candidate_authority() {
    let directory = tempfile::tempdir().expect("directory");
    let project = project(directory.path());
    let grant = ConversationGrant {
        project_id: project.id,
        project_revision: project.revision,
        authority_revision: 1,
        access: AccessMode::ReadOnly,
    };
    let authority = resolve_grant(
        &grant,
        &project,
        crate::conversations::ConversationId::generate().expect("conversation"),
        1,
        None,
    )
    .expect("authority")
    .effective;
    let mut changed = project.clone();
    changed.revision += 1;

    assert_eq!(
        authority.revalidate_project(&changed),
        Err(AuthorityError::Stale)
    );
}

#[test]
fn a_preset_directory_ceiling_must_match_the_canonical_project() {
    let project_dir = tempfile::tempdir().expect("project");
    let other_dir = tempfile::tempdir().expect("other");
    let project = project(project_dir.path());
    let preset = preset(other_dir.path(), ToolId::ALL.to_vec(), NetworkAccess::None);
    let grant = ConversationGrant {
        project_id: project.id,
        project_revision: project.revision,
        authority_revision: 1,
        access: AccessMode::ReadOnly,
    };

    assert_eq!(
        resolve_grant(
            &grant,
            &project,
            crate::conversations::ConversationId::generate().expect("conversation"),
            1,
            Some(&preset),
        )
        .err(),
        Some(ConversationAccessError::MissingGrant)
    );
}

#[test]
fn secondary_aliases_are_stable_bounded_and_not_project_names() {
    let id = ProjectId::parse("0123456789abcdef0123456789abcdef").expect("project");
    let alias = secondary_alias(id);

    assert_eq!(alias, "pabencwpcnlzxxqci2fm6e2xtpp");
    assert!(alias.starts_with('p'));
    assert!(
        alias
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    );
    assert!(alias.len() <= 32);
    assert_ne!(alias, "project");
}

#[test]
fn secondary_context_is_read_only_at_a_stable_guest_alias() {
    let primary_dir = tempfile::tempdir().expect("primary");
    let secondary_dir = tempfile::tempdir().expect("secondary");
    let primary = project(primary_dir.path());
    let secondary = project(secondary_dir.path());
    let alias = secondary_alias(secondary.id);
    let grant = ConversationGrant {
        project_id: primary.id,
        project_revision: primary.revision,
        authority_revision: 1,
        access: AccessMode::ReadWrite,
    };
    let authority = resolve_grant_with_context(
        &grant,
        &primary,
        crate::conversations::ConversationId::generate().expect("conversation"),
        1,
        NetworkAccess::Restricted(vec!["example.com".to_owned()]),
        vec![PolicyGrant {
            alias: alias.clone(),
            guest_path: guest_path_for(&alias, "project"),
            host_path: secondary.host_path.clone(),
            access: AccessMode::ReadOnly,
        }],
        None,
    )
    .expect("authority")
    .effective;

    assert_eq!(
        authority.network,
        NetworkAccess::Restricted(vec!["example.com".to_owned()])
    );
    assert_eq!(
        authority
            .policy
            .resolve(&format!("/access/{alias}/README"))
            .expect("secondary path"),
        (format!("/access/{alias}/README"), AccessMode::ReadOnly)
    );
    assert_eq!(
        authority
            .policy
            .grants()
            .iter()
            .map(|grant| (grant.alias.as_str(), grant.access))
            .collect::<Vec<_>>(),
        vec![
            ("project", AccessMode::ReadWrite),
            (alias.as_str(), AccessMode::ReadOnly)
        ]
    );
}

#[test]
fn secondary_alias_and_canonical_path_collisions_are_rejected() {
    let primary_dir = tempfile::tempdir().expect("primary");
    let secondary_dir = tempfile::tempdir().expect("secondary");
    let primary = project(primary_dir.path());
    let secondary = project(secondary_dir.path());
    let primary_grant = ConversationGrant {
        project_id: primary.id,
        project_revision: primary.revision,
        authority_revision: 1,
        access: AccessMode::ReadOnly,
    };
    let alias = secondary_alias(secondary.id);
    let duplicate_alias = PolicyGrant {
        alias: alias.clone(),
        guest_path: guest_path_for(&alias, "project"),
        host_path: primary.host_path.join("nested"),
        access: AccessMode::ReadOnly,
    };
    assert_eq!(
        resolve_grant_with_context(
            &primary_grant,
            &primary,
            crate::conversations::ConversationId::generate().expect("conversation"),
            1,
            NetworkAccess::None,
            vec![duplicate_alias.clone(), duplicate_alias],
            None,
        )
        .err(),
        Some(ConversationAccessError::Alias)
    );

    let duplicate_path = PolicyGrant {
        alias: "other-context".to_owned(),
        guest_path: "/access/other-context".to_owned(),
        host_path: primary.host_path.clone(),
        access: AccessMode::ReadOnly,
    };
    assert_eq!(
        resolve_grant_with_context(
            &primary_grant,
            &primary,
            crate::conversations::ConversationId::generate().expect("conversation"),
            1,
            NetworkAccess::None,
            vec![duplicate_path],
            None,
        )
        .err(),
        Some(ConversationAccessError::DuplicatePath)
    );
}

#[test]
fn private_workspace_uses_the_copied_settings_without_a_live_preset_ceiling() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let preset = state
        .agents
        .create(crate::agents::AgentDraft {
            name: "Offline".to_owned(),
            instructions: String::new(),
            selection: None,
            tools: vec![ToolId::Run],
            network: NetworkAccess::None,
            directories: Vec::new(),
            primary_directory: String::new(),
        })
        .unwrap();
    let selection = crate::providers::ModelSelection::new(
        crate::providers::ProviderKind::Xai,
        "test-model".to_owned(),
        None,
    )
    .unwrap();
    let mut model = crate::conversations::ConversationModelConfiguration::from_agent_snapshot(
        &preset,
        selection,
        crate::tests::test_environment_id(),
    );
    model.settings.network = NetworkAccess::Public;
    model.settings.tools.push(ToolId::Write);
    let record = state
        .conversations
        .create_saved(
            crate::conversations::ConversationId::generate().unwrap(),
            None,
            None,
            Some(model),
            Vec::new(),
        )
        .unwrap();
    let authority = crate::execution::ProjectFreeAuthority::from_settings(
        record.revision,
        &record.model.as_ref().unwrap().settings,
    )
    .unwrap();
    assert_eq!(authority.network, NetworkAccess::Public);
    assert_eq!(authority.tools, vec![ToolId::Run, ToolId::Write]);
    assert!(authority.policy.grants().is_empty());
    assert!(record.model.as_ref().unwrap().preset.is_none());
}

#[test]
fn network_intersection_never_expands_a_conversation_selection() {
    assert_eq!(
        intersect_network(
            &NetworkAccess::Public,
            Some(&NetworkAccess::Restricted(vec![
                "api.example.com".to_owned()
            ])),
        ),
        NetworkAccess::Restricted(vec!["api.example.com".to_owned()])
    );
    assert_eq!(
        intersect_network(
            &NetworkAccess::Restricted(vec!["example.com".to_owned()]),
            Some(&NetworkAccess::Restricted(vec![
                "api.example.com".to_owned()
            ])),
        ),
        NetworkAccess::Restricted(vec!["api.example.com".to_owned()])
    );
    assert_eq!(
        intersect_network(
            &NetworkAccess::Restricted(vec!["example.com".to_owned()]),
            Some(&NetworkAccess::Restricted(vec!["other.example".to_owned()])),
        ),
        NetworkAccess::None
    );
}

#[test]
fn resolving_a_conversation_materialises_each_secondary_project_by_id() {
    let primary_dir = tempfile::tempdir().expect("primary");
    let secondary_dir = tempfile::tempdir().expect("secondary");
    for directory in [primary_dir.path(), secondary_dir.path()] {
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(directory)
                .status()
                .expect("git")
                .success()
        );
    }
    let projects = ProjectStore::in_memory();
    let primary = projects
        .create("Primary".to_owned(), primary_dir.path().to_path_buf())
        .expect("primary project");
    let secondary = projects
        .create("Secondary".to_owned(), secondary_dir.path().to_path_buf())
        .expect("secondary project");
    let conversation_id = crate::conversations::ConversationId::generate().expect("conversation");
    let record = crate::conversations::ConversationRecord {
        id: conversation_id,
        revision: 4,
        title: "Discussion".to_owned(),
        title_pending: false,
        projects: vec![primary.id, secondary.id],
        grants: vec![
            ConversationGrant {
                project_id: primary.id,
                project_revision: primary.revision,
                authority_revision: 2,
                access: AccessMode::ReadOnly,
            },
            ConversationGrant {
                project_id: secondary.id,
                project_revision: secondary.revision,
                authority_revision: 3,
                access: AccessMode::ReadWrite,
            },
        ],
        execution_target: Some(primary.id),
        network: NetworkAccess::None,
        model: None,
        directory_approvals: Vec::new(),

        source_candidate_review: None,
        candidate_reviews: Vec::new(),
        candidate_review_context: None,
        messages: Vec::new(),
        active_job: None,
        created_at_ms: 1,
        updated_at_ms: 1,
    };

    let authority =
        resolve_workflow_authority(&record, &projects, &crate::agents::AgentStore::in_memory())
            .expect("authority")
            .expect("target authority")
            .effective;
    let alias = secondary_alias(secondary.id);
    assert_eq!(authority.grant_access, AccessMode::ReadOnly);
    assert_eq!(authority.policy.primary_guest(), "/project");
    assert_eq!(
        authority.policy.resolve(&format!("/access/{alias}/src")),
        Ok((format!("/access/{alias}/src"), AccessMode::ReadOnly))
    );

    let mut stale = record;
    stale.grants[1].project_revision += 1;
    assert_eq!(
        resolve_workflow_authority(&stale, &projects, &crate::agents::AgentStore::in_memory())
            .err(),
        Some(ConversationAccessError::Stale)
    );
}
