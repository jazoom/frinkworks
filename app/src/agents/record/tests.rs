use std::path::PathBuf;

use super::{
    AccessMode, AgentDraft, AgentError, AgentRecord, DirectoryGrant, MAXIMUM_GRANTS,
    MAXIMUM_INSTRUCTION_BYTES, MAXIMUM_NAME_BYTES, MAXIMUM_NETWORK_DOMAINS, NetworkAccess,
    canonical_directory,
};
use crate::agents::id::AgentId;
use crate::agents::tool_id::ToolId;
use crate::providers::{ModelSelection, ProviderKind};

fn draft(dir: &std::path::Path) -> AgentDraft {
    AgentDraft {
        name: "Repository maintainer".to_owned(),
        instructions: String::new(),
        selection: None,
        tools: ToolId::ALL.to_vec(),
        network: NetworkAccess::None,
        directories: vec![DirectoryGrant {
            alias: "project".to_owned(),
            host_path: dir.to_path_buf(),
            access: AccessMode::ReadWrite,
        }],
        primary_directory: "project".to_owned(),
    }
}

#[test]
fn valid_draft_canonicalises_the_host_path() {
    let dir = tempfile::tempdir().expect("dir");
    let validated = draft(dir.path()).validate().expect("valid");
    assert_eq!(
        validated.directories[0].host_path,
        dir.path().canonicalize().expect("canonical")
    );
}

#[test]
fn bounds_reject_empty_and_oversized_names() {
    let dir = tempfile::tempdir().expect("dir");
    let mut item = draft(dir.path());
    item.name = "  ".to_owned();
    assert_eq!(item.validate().err(), Some(AgentError::Name));
    let mut item = draft(dir.path());
    item.name = "a".repeat(MAXIMUM_NAME_BYTES + 1);
    assert_eq!(item.validate().err(), Some(AgentError::Name));
}

#[test]
fn instructions_may_be_empty_but_not_oversized() {
    let dir = tempfile::tempdir().expect("dir");
    assert!(draft(dir.path()).validate().is_ok());
    let mut item = draft(dir.path());
    item.instructions = "a".repeat(MAXIMUM_INSTRUCTION_BYTES + 1);
    assert_eq!(item.validate().err(), Some(AgentError::Instructions));
}

#[test]
fn unknown_or_duplicate_tools_are_rejected() {
    let dir = tempfile::tempdir().expect("dir");
    let mut item = draft(dir.path());
    item.tools = vec![ToolId::List, ToolId::List];
    assert_eq!(item.validate().err(), Some(AgentError::Tools));
}

#[test]
fn absent_primary_directory_is_valid_only_without_directory_grants() {
    let dir = tempfile::tempdir().expect("dir");
    let mut item = draft(dir.path());
    item.primary_directory.clear();
    assert_eq!(item.validate().err(), Some(AgentError::Primary));

    let mut item = draft(dir.path());
    item.directories.clear();
    assert_eq!(item.clone().validate().err(), Some(AgentError::Primary));
    item.primary_directory.clear();
    assert!(item.validate().is_ok());
}

#[test]
fn tool_ceilings_do_not_require_a_directory_grant() {
    let dir = tempfile::tempdir().expect("dir");
    let mut item = draft(dir.path());
    item.directories.clear();
    item.primary_directory.clear();
    item.tools = vec![ToolId::Write];
    let validated = item.validate().expect("preset");
    assert_eq!(validated.tools, vec![ToolId::Write]);
    assert!(validated.directories.is_empty());
}

#[test]
fn overlapping_host_grants_are_rejected() {
    let dir = tempfile::tempdir().expect("dir");
    let nested = dir.path().join("nested");
    std::fs::create_dir(&nested).expect("nested");
    let mut item = draft(dir.path());
    item.directories.push(DirectoryGrant {
        alias: "nested".to_owned(),
        host_path: nested,
        access: AccessMode::ReadOnly,
    });
    assert_eq!(item.validate().err(), Some(AgentError::NestedPath));
}

#[test]
fn duplicate_aliases_are_rejected() {
    let dir = tempfile::tempdir().expect("dir");
    let other = tempfile::tempdir().expect("other");
    let mut item = draft(dir.path());
    item.directories.push(DirectoryGrant {
        alias: "project".to_owned(),
        host_path: other.path().to_path_buf(),
        access: AccessMode::ReadOnly,
    });
    assert_eq!(item.validate().err(), Some(AgentError::DuplicateAlias));
}

#[test]
fn grant_count_is_bounded() {
    let mut dirs = Vec::new();
    for _ in 0..=MAXIMUM_GRANTS {
        dirs.push(tempfile::tempdir().expect("dir"));
    }
    let directories = dirs
        .iter()
        .enumerate()
        .map(|(index, dir)| DirectoryGrant {
            alias: format!("d{index}"),
            host_path: dir.path().to_path_buf(),
            access: AccessMode::ReadWrite,
        })
        .collect();
    let item = AgentDraft {
        name: "Many".to_owned(),
        instructions: String::new(),
        selection: None,
        tools: vec![ToolId::List],
        network: NetworkAccess::None,
        directories,
        primary_directory: "d0".to_owned(),
    };
    assert_eq!(item.validate().err(), Some(AgentError::GrantCount));
}

#[test]
fn relative_and_missing_paths_are_rejected() {
    let mut item = AgentDraft {
        name: "Agent".to_owned(),
        instructions: String::new(),
        selection: None,
        tools: Vec::new(),
        network: NetworkAccess::None,
        directories: vec![DirectoryGrant {
            alias: "project".to_owned(),
            host_path: PathBuf::from("relative"),
            access: AccessMode::ReadWrite,
        }],
        primary_directory: "project".to_owned(),
    };
    assert_eq!(item.clone().validate().err(), Some(AgentError::Path));
    item.directories[0].host_path = PathBuf::from("/no/such/frinkworks-agent-dir");
    assert_eq!(item.validate().err(), Some(AgentError::PathMissing));
}

#[test]
fn file_round_trip_keeps_the_identifier() {
    let dir = tempfile::tempdir().expect("dir");
    let mut draft = draft(dir.path());
    draft.selection =
        Some(ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None).expect("model"));
    draft.network = NetworkAccess::Restricted(vec!["registry.npmjs.org".to_owned()]);
    let validated = draft.validate().expect("valid");
    let record = AgentRecord {
        id: AgentId::generate().expect("id"),
        revision: 3,
        name: validated.name,
        instructions: validated.instructions,
        selection: validated.selection,
        tools: validated.tools,
        network: validated.network,
        directories: validated.directories,
        primary_directory: validated.primary_directory,
    };
    let restored = AgentRecord::from_file(record.to_file()).expect("file");
    assert_eq!(restored.id, record.id);
    assert_eq!(restored.revision, 3);
    assert_eq!(restored.name, "Repository maintainer");
    assert_eq!(
        restored.network,
        NetworkAccess::Restricted(vec!["registry.npmjs.org".to_owned()])
    );
    assert_eq!(restored.selection, record.selection);
}

#[test]
fn unknown_record_fields_are_rejected() {
    let dir = tempfile::tempdir().expect("dir");
    let validated = draft(dir.path()).validate().expect("valid");
    let record = AgentRecord {
        id: AgentId::generate().expect("id"),
        revision: 1,
        name: validated.name,
        instructions: validated.instructions,
        selection: None,
        tools: validated.tools,
        network: validated.network,
        directories: validated.directories,
        primary_directory: validated.primary_directory,
    };
    let mut value = serde_json::to_value(record.to_file()).expect("json");
    value["removed-field"] = serde_json::json!(true);

    assert!(serde_json::from_value::<super::AgentFile>(value).is_err());
}

#[test]
fn restricted_network_domains_are_normalised_and_bounded() {
    let dir = tempfile::tempdir().expect("dir");
    let mut item = draft(dir.path());
    item.network = NetworkAccess::Restricted(vec![
        ".Registry.NPMJS.org.".to_owned(),
        "github.com".to_owned(),
        "REGISTRY.npmjs.org".to_owned(),
    ]);
    let validated = item.validate().expect("network");
    assert_eq!(
        validated.network,
        NetworkAccess::Restricted(vec![
            "registry.npmjs.org".to_owned(),
            "github.com".to_owned(),
        ])
    );

    for invalid in [
        "com",
        "https://example.com",
        "example.com/path",
        "*.example.com",
    ] {
        let mut item = draft(dir.path());
        item.network = NetworkAccess::Restricted(vec![invalid.to_owned()]);
        assert_eq!(
            item.validate().err(),
            Some(AgentError::Network),
            "{invalid}"
        );
    }

    let mut item = draft(dir.path());
    item.network = NetworkAccess::Restricted(
        (0..=MAXIMUM_NETWORK_DOMAINS)
            .map(|index| format!("domain-{index}.example"))
            .collect(),
    );
    assert_eq!(item.validate().err(), Some(AgentError::Network));
}

#[test]
fn canonical_directory_rejects_a_file() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("file");
    std::fs::write(&file, b"x").expect("file");
    assert_eq!(
        canonical_directory(&file).err(),
        Some(AgentError::NotADirectory)
    );
}

#[test]
fn aliases_must_start_with_a_letter() {
    let dir = tempfile::tempdir().expect("dir");
    let mut item = draft(dir.path());
    item.directories[0].alias = "1docs".to_owned();
    assert_eq!(item.validate().err(), Some(AgentError::Alias));
    let mut item = draft(dir.path());
    item.directories[0].alias = "docs/src".to_owned();
    assert_eq!(item.validate().err(), Some(AgentError::Alias));
}
