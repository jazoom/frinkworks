use super::*;

use crate::agents::GUEST_PROJECT;

pub(crate) fn test_agent_capabilities() -> AttemptCapabilities {
    AttemptCapabilities {
        schema: CAPABILITY_SCHEMA,
        agent_revision: 1,
        tools: vec![ToolId::List],
        directories: vec![CapabilityDirectory {
            alias: PRIMARY_SOURCE_ALIAS.to_owned(),
            guest_path: GUEST_PROJECT.to_owned(),
            access: AccessMode::ReadWrite,
            role: DirectoryRole::PrimarySource,
        }],
        source_location: PrimarySourceLocation::AttemptWorkspace,
        git_admin: AccessMode::ReadOnly,
        network: NetworkCapability::None,
    }
}
pub(crate) fn test_command_capabilities() -> AttemptCapabilities {
    AttemptCapabilities {
        schema: CAPABILITY_SCHEMA,
        agent_revision: 1,
        tools: Vec::new(),
        directories: vec![CapabilityDirectory {
            alias: PRIMARY_SOURCE_ALIAS.to_owned(),
            guest_path: GUEST_PROJECT.to_owned(),
            access: AccessMode::ReadOnly,
            role: DirectoryRole::PrimarySource,
        }],
        source_location: PrimarySourceLocation::AttemptWorkspace,
        git_admin: AccessMode::ReadOnly,
        network: NetworkCapability::None,
    }
}

use super::{
    AttemptCapabilities, CapabilityError, DirectoryRole, NetworkCapability, PrimarySourceLocation,
};
use crate::agents::{AccessMode, AgentId, AgentRecord, DirectoryGrant, NetworkAccess, ToolId};
use crate::tools::SUBMIT_WORKFLOW_OUTPUT;
use crate::workflows::definition::{
    AgentAuthority, AgentStep, CandidateAuthority, GuestDirectoryAccess, OutputKey, OutputKind,
    RequiredOutput, RoleKey, StepAction, StepDefinition, StepEnvironment, StepKey, SystemCommandId,
    SystemCommandStep,
};

fn agent(tools: Vec<ToolId>, writable: bool) -> AgentRecord {
    agent_with_primary(tools, writable, "project")
}

fn agent_with_primary(tools: Vec<ToolId>, writable: bool, primary: &str) -> AgentRecord {
    AgentRecord {
        id: AgentId::generate().expect("id"),
        revision: 1,
        name: "Agent".to_owned(),
        instructions: String::new(),
        selection: None,
        tools,
        network: NetworkAccess::None,
        directories: vec![DirectoryGrant {
            alias: primary.to_owned(),
            host_path: "/tmp/project".into(),
            access: if writable {
                AccessMode::ReadWrite
            } else {
                AccessMode::ReadOnly
            },
        }],
        primary_directory: primary.to_owned(),
    }
}

fn derive(
    step: &StepDefinition,
    agent: &AgentRecord,
) -> Result<AttemptCapabilities, CapabilityError> {
    AttemptCapabilities::derive(step, agent, &agent.primary_directory)
}

fn agent_step(tools: Vec<ToolId>, writable: bool) -> StepDefinition {
    let authority = AgentAuthority::new(tools, Vec::new()).expect("authority");
    StepDefinition {
        key: StepKey::parse("work").expect("step"),
        name: "Work".to_owned(),
        inputs: Vec::new(),
        action: StepAction::Agent(AgentStep {
            role: RoleKey::parse("agent").expect("role"),
            environment: StepEnvironment::WorkflowDefault,
            candidate_authority: if writable {
                CandidateAuthority::Edit
            } else {
                CandidateAuthority::ReadOnly
            },
            authority,
            settings: crate::workflows::definition::ModelStepSettings::SameAsRunDefaults,
            required_outputs: Vec::new(),
        }),
        review: None,
    }
}

fn status_step() -> StepDefinition {
    StepDefinition {
        key: StepKey::parse("status").expect("step"),
        name: "Status".to_owned(),
        inputs: Vec::new(),
        action: StepAction::SystemCommand(SystemCommandStep {
            command: SystemCommandId::RepositoryStatus,
            environment: StepEnvironment::WorkflowDefault,
            required_outputs: Vec::new(),
        }),
        review: None,
    }
}

fn commit_step() -> StepDefinition {
    StepDefinition {
        key: StepKey::parse("commit").expect("step"),
        name: "Commit".to_owned(),
        inputs: Vec::new(),
        action: StepAction::SystemCommand(SystemCommandStep {
            command: SystemCommandId::CommitCandidate,
            environment: StepEnvironment::WorkflowDefault,
            required_outputs: vec![RequiredOutput {
                key: OutputKey::parse("committed-candidate").expect("output"),
                kind: OutputKind::CandidateRevision,
            }],
        }),
        review: None,
    }
}

#[test]
fn capability_policy_table() {
    let ceiling = agent(ToolId::ALL.to_vec(), true);

    let over = agent_step(vec![ToolId::List, ToolId::Write], true);
    assert_eq!(
        derive(&over, &agent(vec![ToolId::List], true)),
        Err(CapabilityError::Authority)
    );

    let planner = derive(
        &agent_step(vec![ToolId::List, ToolId::Read, ToolId::Run], false),
        &ceiling,
    )
    .expect("planner");
    assert_eq!(planner.schema, super::CAPABILITY_SCHEMA);
    assert_eq!(planner.agent_revision, ceiling.revision);
    assert_eq!(
        planner.primary().map(|directory| directory.access),
        Some(AccessMode::ReadOnly)
    );
    assert_eq!(planner.git_admin, AccessMode::ReadOnly);
    assert_eq!(
        planner.source_location,
        PrimarySourceLocation::AttemptWorkspace
    );
    assert_eq!(planner.network, NetworkCapability::None);

    let custom_primary = agent_with_primary(ToolId::ALL.to_vec(), true, "source");
    let implementer =
        derive(&agent_step(ToolId::ALL.to_vec(), true), &custom_primary).expect("implementer");
    assert_eq!(
        implementer
            .primary()
            .map(|directory| (directory.alias.as_str(), directory.access)),
        Some(("source", AccessMode::ReadWrite))
    );
    assert_eq!(
        implementer.source_location,
        PrimarySourceLocation::AttemptWorkspace
    );
    assert_eq!(implementer.git_admin, AccessMode::ReadOnly);
    assert!(
        !implementer
            .tools
            .iter()
            .any(|tool| tool.as_str() == SUBMIT_WORKFLOW_OUTPUT)
    );

    let reviewer = derive(
        &agent_step(vec![ToolId::List, ToolId::Read, ToolId::Run], false),
        &ceiling,
    )
    .expect("reviewer");
    assert_eq!(
        reviewer.primary().map(|directory| directory.access),
        Some(AccessMode::ReadOnly)
    );
    assert_eq!(reviewer.git_admin, AccessMode::ReadOnly);

    let status = derive(&status_step(), &ceiling).expect("status");
    assert!(status.tools.is_empty());
    assert_eq!(status.network, NetworkCapability::None);
    assert_eq!(status.git_admin, AccessMode::ReadOnly);
    assert_eq!(
        status.source_location,
        PrimarySourceLocation::AttemptWorkspace
    );
    assert_eq!(
        status.primary().map(|directory| directory.access),
        Some(AccessMode::ReadOnly)
    );

    let commit = derive(&commit_step(), &ceiling).expect("commit");
    assert!(commit.tools.is_empty());
    assert_eq!(commit.network, NetworkCapability::None);
    assert_eq!(commit.git_admin, AccessMode::ReadWrite);
    assert_eq!(commit.source_location, PrimarySourceLocation::UserProject);
    assert_eq!(
        commit.primary().map(|directory| directory.access),
        Some(AccessMode::ReadWrite)
    );
    assert_eq!(
        commit.primary().map(|directory| directory.role),
        Some(DirectoryRole::PrimarySource)
    );
}

#[test]
fn agent_network_access_is_pinned() {
    for (access, expected) in [
        (NetworkAccess::None, NetworkCapability::None),
        (
            NetworkAccess::Restricted(vec!["npmjs.org".to_owned()]),
            NetworkCapability::Restricted(vec!["npmjs.org".to_owned()]),
        ),
        (NetworkAccess::Public, NetworkCapability::Public),
    ] {
        let mut configured = agent(ToolId::ALL.to_vec(), true);
        configured.network = access;
        let capabilities = derive(
            &agent_step(vec![ToolId::List, ToolId::Read, ToolId::Run], true),
            &configured,
        )
        .expect("capabilities");
        assert_eq!(capabilities.network, expected);
    }
}

#[test]
fn selected_grant_replaces_the_saved_primary_for_attempt_authority() {
    let selected = AgentRecord {
        id: AgentId::generate().expect("id"),
        revision: 3,
        name: "Agent".to_owned(),
        instructions: String::new(),
        selection: None,
        tools: vec![ToolId::List],
        network: NetworkAccess::None,
        directories: vec![
            DirectoryGrant {
                alias: "docs".to_owned(),
                host_path: "/tmp/docs".into(),
                access: AccessMode::ReadOnly,
            },
            DirectoryGrant {
                alias: "code".to_owned(),
                host_path: "/tmp/code".into(),
                access: AccessMode::ReadWrite,
            },
        ],
        primary_directory: "docs".to_owned(),
    };
    let mut step = agent_step(vec![ToolId::List], true);
    let StepAction::Agent(action) = &mut step.action else {
        panic!("agent step");
    };
    action.authority = AgentAuthority::new(
        vec![ToolId::List],
        vec![GuestDirectoryAccess {
            alias: "docs".to_owned(),
            access: AccessMode::ReadOnly,
        }],
    )
    .expect("authority");

    let capabilities = AttemptCapabilities::derive(&step, &selected, "code").expect("selected");
    assert_eq!(capabilities.agent_revision, selected.revision);
    assert_eq!(
        capabilities
            .directories
            .iter()
            .map(|directory| {
                (
                    directory.alias.as_str(),
                    directory.guest_path.as_str(),
                    directory.access,
                    directory.role,
                )
            })
            .collect::<Vec<_>>(),
        [
            (
                "code",
                "/project",
                AccessMode::ReadWrite,
                DirectoryRole::PrimarySource,
            ),
            (
                "docs",
                "/access/docs",
                AccessMode::ReadOnly,
                DirectoryRole::SecondaryContext,
            ),
        ]
    );
    assert_eq!(
        AttemptCapabilities::derive(&commit_step(), &selected, "docs"),
        Err(CapabilityError::Authority)
    );
}

#[test]
fn read_only_reviews_stay_read_only_with_reviewed_settings() {
    let root = tempfile::tempdir().unwrap();
    let mut grant = crate::execution::DirectoryGrant::from_selected(root.path(), &[]).unwrap();
    grant.access = crate::execution::DirectoryAccess::ReviewBeforeApply;
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(
            crate::providers::ProviderKind::Xai,
            "grok-4.6".to_owned(),
            None,
        )
        .unwrap(),
        String::new(),
        crate::agents::ToolId::ALL.to_vec(),
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_directories(vec![grant])
    .unwrap();
    let authority = crate::execution::ProjectFreeAuthority::from_settings(1, &settings).unwrap();
    let mut step = agent_step(vec![ToolId::List, ToolId::Read, ToolId::Run], false);
    let StepAction::Agent(action) = &mut step.action else {
        panic!("agent");
    };
    action.authority = AgentAuthority::new(
        vec![ToolId::List, ToolId::Read, ToolId::Run],
        settings
            .directories
            .iter()
            .map(|grant| GuestDirectoryAccess {
                alias: grant.alias.clone(),
                access: AccessMode::ReadOnly,
            })
            .collect(),
    )
    .unwrap();
    let capabilities =
        AttemptCapabilities::derive_project_free(&step, &authority).expect("read-only");
    assert!(
        capabilities
            .directories
            .iter()
            .all(|directory| directory.access == AccessMode::ReadOnly)
    );
}
