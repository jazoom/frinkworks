use super::compose_role;
use crate::agents::policy::DirectoryPolicy;
use crate::agents::record::{AccessMode, AgentRecord, DirectoryGrant, NetworkAccess};
use crate::agents::{AgentId, ToolId};

#[test]
fn composed_preamble_omits_host_paths() {
    let record = AgentRecord {
        id: AgentId::generate().expect("id"),
        revision: 2,
        name: "Maintainer".to_owned(),
        instructions: "Keep public interfaces stable.".to_owned(),
        selection: None,
        tools: vec![ToolId::List, ToolId::Read],
        network: NetworkAccess::None,
        directories: vec![DirectoryGrant {
            alias: "project".to_owned(),
            host_path: "/home/user/src/secret-repo".into(),
            access: AccessMode::ReadWrite,
        }],
        primary_directory: "project".to_owned(),
    };
    let policy = DirectoryPolicy::from_record_with_primary(&record, &record.primary_directory);
    let preamble = compose_role(
        &record.name,
        "",
        &record.instructions,
        &record.tools,
        &policy,
    );
    assert!(preamble.contains("Power Plant contract"));
    assert!(preamble.contains("Keep public interfaces stable."));
    assert!(preamble.contains("/project"));
    assert!(preamble.contains("- list"));
    assert!(preamble.contains("- read"));
    assert!(!preamble.contains("secret-repo"));
    assert!(!preamble.contains("/home/user"));
    let host = policy.on_host(&record.directories[0].host_path);
    let preamble = compose_role(&record.name, "", &record.instructions, &record.tools, &host);
    assert!(preamble.contains("/home/user/src/secret-repo"));
    assert!(preamble.contains("File changes take effect immediately."));
    assert!(!preamble.contains("Host paths are not available."));
    assert!(!preamble.contains("/project"));
}
