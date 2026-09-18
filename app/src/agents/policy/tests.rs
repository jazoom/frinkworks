use super::DirectoryPolicy;
use crate::agents::record::{AccessMode, AgentRecord, DirectoryGrant, NetworkAccess};
use crate::agents::{AgentId, ToolId};

fn record(primary: &str, grants: Vec<DirectoryGrant>) -> AgentRecord {
    AgentRecord {
        id: AgentId::generate().expect("id"),
        revision: 1,
        name: "Agent".to_owned(),
        instructions: String::new(),
        selection: None,
        tools: vec![ToolId::List],
        network: NetworkAccess::None,
        directories: grants,
        primary_directory: primary.to_owned(),
    }
}

#[test]
fn primary_grant_is_mounted_at_project() {
    let policy = DirectoryPolicy::from_record_with_primary(
        &record(
            "code",
            vec![
                DirectoryGrant {
                    alias: "code".to_owned(),
                    host_path: "/tmp/code".into(),
                    access: AccessMode::ReadWrite,
                },
                DirectoryGrant {
                    alias: "docs".to_owned(),
                    host_path: "/tmp/docs".into(),
                    access: AccessMode::ReadOnly,
                },
            ],
        ),
        "code",
    );
    assert_eq!(policy.primary_guest(), "/project");
    assert_eq!(policy.resolve("").expect("default").0, "/project");
    assert_eq!(
        policy.resolve("src/main.rs").expect("relative").0,
        "/project/src/main.rs"
    );
    assert_eq!(
        policy.resolve("/access/docs/readme").expect("docs").0,
        "/access/docs/readme"
    );
    assert_eq!(
        policy.resolve("/access/docs/readme").expect("docs").1,
        AccessMode::ReadOnly
    );
}

#[test]
fn resolve_rejects_escape_and_unknown_roots() {
    let policy = DirectoryPolicy::from_record_with_primary(
        &record(
            "project",
            vec![DirectoryGrant {
                alias: "project".to_owned(),
                host_path: "/tmp/code".into(),
                access: AccessMode::ReadWrite,
            }],
        ),
        "project",
    );
    assert_eq!(
        policy.resolve(".."),
        Err("Stay inside a granted directory.")
    );
    assert_eq!(
        policy.resolve("/etc/passwd"),
        Err("Stay inside a granted directory.")
    );
    assert_eq!(
        policy.resolve("/tmp/\u{0000}x"),
        Err("That path is not valid.")
    );
}

#[test]
fn a_selected_non_primary_grant_is_mounted_at_project() {
    let record = record(
        "docs",
        vec![
            DirectoryGrant {
                alias: "code".to_owned(),
                host_path: "/tmp/code".into(),
                access: AccessMode::ReadWrite,
            },
            DirectoryGrant {
                alias: "docs".to_owned(),
                host_path: "/tmp/docs".into(),
                access: AccessMode::ReadOnly,
            },
        ],
    );
    let policy = DirectoryPolicy::from_record_with_primary(&record, "code");
    assert_eq!(policy.primary_alias(), "code");
    assert_eq!(policy.primary_guest(), "/project");
    assert_eq!(policy.primary_access(), AccessMode::ReadWrite);
    assert_eq!(
        policy.resolve("/access/docs/readme").expect("docs").0,
        "/access/docs/readme"
    );
    assert_eq!(
        policy.resolve("/access/docs/readme").expect("docs").1,
        AccessMode::ReadOnly
    );
}
