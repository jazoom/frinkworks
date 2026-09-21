use super::{
    AgentToolContext, MAXIMUM_TOOL_BYTES, ToolFailureKind, advertised, authorised_tool,
    definitions_for, mark_truncated, redact, valid_output_reference,
};
use crate::agents::{AccessMode, AgentId, AgentRecord, DirectoryGrant, DirectoryPolicy, ToolId};
use crate::conversations::ConversationId;
use crate::execution::ToolLocation;
use crate::sessions::{Job, JobId};

fn policy() -> DirectoryPolicy {
    DirectoryPolicy::from_record_with_primary(
        &AgentRecord {
            id: AgentId::generate().expect("id"),
            revision: 1,
            name: "Agent".to_owned(),
            instructions: String::new(),
            selection: None,
            tools: ToolId::ALL.to_vec(),
            network: crate::agents::NetworkAccess::None,
            directories: vec![
                DirectoryGrant {
                    alias: "project".to_owned(),
                    host_path: "/tmp/project".into(),
                    access: AccessMode::ReadWrite,
                },
                DirectoryGrant {
                    alias: "docs".to_owned(),
                    host_path: "/tmp/docs".into(),
                    access: AccessMode::ReadOnly,
                },
            ],
            primary_directory: "project".to_owned(),
        },
        "project",
    )
}

#[test]
fn definitions_and_dispatch_use_the_same_selected_tool_set() {
    let selected = [ToolId::Read, ToolId::List];
    let names: Vec<_> = definitions_for(&selected, ToolLocation::Sandbox)
        .into_iter()
        .map(|definition| definition.name)
        .collect();
    assert_eq!(names, vec!["read", "list"]);
    assert_eq!(
        authorised_tool(&selected, "read", ToolLocation::Sandbox),
        Some(ToolId::Read)
    );
    assert_eq!(
        authorised_tool(&selected, "write", ToolLocation::Sandbox),
        None
    );
    assert_eq!(
        authorised_tool(&selected, "forged", ToolLocation::Sandbox),
        None
    );
    assert_eq!(
        advertised(&[ToolId::List, ToolId::Run], ToolLocation::Host),
        vec![ToolId::Run]
    );
    assert_eq!(
        advertised(
            &[ToolId::Read, ToolId::Edit, ToolId::Run],
            ToolLocation::Host
        ),
        vec![ToolId::Run]
    );
    assert_eq!(
        authorised_tool(&[ToolId::List, ToolId::Run], "list", ToolLocation::Host),
        None
    );
    assert_eq!(
        authorised_tool(&[ToolId::Edit, ToolId::Run], "edit", ToolLocation::Host),
        None
    );
}

#[test]
fn guest_path_accepts_each_grant() {
    let policy = policy();
    assert_eq!(policy.resolve("").expect("default").0, "/project");
    assert_eq!(
        policy.resolve("src/main.rs").expect("relative"),
        ("/project/src/main.rs".to_owned(), AccessMode::ReadWrite)
    );
    assert_eq!(
        policy.resolve("/access/docs/readme").expect("docs").0,
        "/access/docs/readme"
    );
}

#[test]
fn guest_path_rejects_escape_and_control() {
    let policy = policy();
    assert_eq!(
        policy.resolve(".."),
        Err("Stay inside a granted directory.")
    );
    assert_eq!(
        policy.resolve("/etc/passwd"),
        Err("Stay inside a granted directory.")
    );
    assert_eq!(
        policy.resolve("/project/../../secret"),
        Err("Stay inside a granted directory.")
    );
    assert_eq!(
        policy.resolve("/tmp/\u{0000}x"),
        Err("That path is not valid.")
    );
}

#[test]
fn write_paths_are_read_only_outside_writable_grants() {
    let policy = policy();
    assert_eq!(
        policy.resolve("/access/docs/readme").expect("docs").1,
        AccessMode::ReadOnly
    );
    assert!(
        policy
            .resolve("/project/note.txt")
            .expect("write")
            .1
            .is_writable()
    );
}

#[test]
fn redact_removes_the_vault_secret() {
    assert_eq!(
        redact("token sk-secret in output", Some("sk-secret")),
        "token [redacted] in output"
    );
    assert_eq!(redact("plain", None), "plain");
    assert_eq!(redact("plain", Some("")), "plain");
}

#[test]
fn truncated_tool_output_carries_a_bounded_marker() {
    let mut output = "x".repeat(MAXIMUM_TOOL_BYTES);
    mark_truncated(&mut output);
    assert_eq!(output.len(), MAXIMUM_TOOL_BYTES);
    assert!(output.ends_with("[output truncated]"));
}

#[test]
fn file_tools_require_success_and_exclude_stderr_from_content() {
    use crate::execution::command::{
        CommandChunk, CommandResult, CommandStream, CommandTermination,
    };

    let chunks = vec![
        CommandChunk {
            stream: CommandStream::Stdout,
            text: "file contents".to_owned(),
        },
        CommandChunk {
            stream: CommandStream::Stderr,
            text: "diagnostic".to_owned(),
        },
    ];
    assert_eq!(
        super::plain_capture(
            "read".to_owned(),
            Ok(CommandResult::new(
                chunks.clone(),
                CommandTermination::Exited(0)
            ))
        )
        .ok(),
        Some("file contents".to_owned()),
    );
    for termination in [
        CommandTermination::Exited(1),
        CommandTermination::Unknown,
        CommandTermination::ResourceLimit,
    ] {
        assert!(
            super::plain_capture(
                "read".to_owned(),
                Ok(CommandResult::new(chunks.clone(), termination))
            )
            .is_err()
        );
    }
}

#[test]
fn only_server_generated_references_resolve_retained_output() {
    assert!(valid_output_reference(&"a".repeat(32)));
    assert!(!valid_output_reference("../../../etc/passwd"));
    assert!(!valid_output_reference(""));
    assert!(!valid_output_reference(&"a".repeat(31)));
    assert!(!valid_output_reference(&"z".repeat(32)));
    assert!(!valid_output_reference("/tmp/output"));
}

fn tool_job() -> std::sync::Arc<Job> {
    Job::for_conversation(
        JobId::generate().expect("job id"),
        ConversationId::generate().expect("conversation"),
    )
}

fn sandbox_context<'a>(
    policy: &'a DirectoryPolicy,
    job: &'a Job,
    tools: &'a [ToolId],
) -> AgentToolContext<'a> {
    AgentToolContext {
        sandbox: None,
        policy,
        job,
        tools,
        location: ToolLocation::Sandbox,
        host: None,
        secret: None,
        outputs: None,
        output_scope: None,
        output_drafts: None,
        required_outputs: &[],
    }
}

#[tokio::test]
async fn path_escape_is_an_authority_failure() {
    let policy = policy();
    let job = tool_job();
    let tools = [ToolId::Read];
    let context = sandbox_context(&policy, &job, &tools);
    let trace = super::invoke(
        &context,
        "call-1",
        "read",
        &serde_json::json!({"path": ".."}),
    )
    .await;
    assert_eq!(trace.failure, Some(ToolFailureKind::Authority));
    assert!(trace.command.is_none());
}

#[tokio::test]
async fn read_only_write_is_an_authority_failure() {
    let policy = policy();
    let job = tool_job();
    let tools = [ToolId::Write];
    let context = sandbox_context(&policy, &job, &tools);
    let trace = super::invoke(
        &context,
        "call-1",
        "write",
        &serde_json::json!({"path": "/access/docs/readme", "contents": "x"}),
    )
    .await;
    assert_eq!(trace.failure, Some(ToolFailureKind::Authority));
    assert!(trace.command.is_none());
}

#[tokio::test]
async fn recoverable_read_error_is_ordinary() {
    let policy = policy();
    let job = tool_job();
    let tools = [ToolId::Read];
    let context = sandbox_context(&policy, &job, &tools);
    let trace = super::invoke(&context, "call-1", "read", &serde_json::json!({"path": ""})).await;
    assert_eq!(trace.failure, Some(ToolFailureKind::Ordinary));
    assert_eq!(trace.output, "Choose a file to read.");
    assert!(trace.command.is_none());
}

#[tokio::test]
async fn read_offset_zero_is_ordinary() {
    let policy = policy();
    let job = tool_job();
    let tools = [ToolId::Read];
    let context = sandbox_context(&policy, &job, &tools);
    let trace = super::invoke(
        &context,
        "call-1",
        "read",
        &serde_json::json!({"path": "src/main.rs", "offset": 0}),
    )
    .await;
    assert_eq!(trace.failure, Some(ToolFailureKind::Ordinary));
    assert_eq!(trace.output, "Read offset must be a 1-based line number.");
}

#[tokio::test]
async fn edit_escape_and_read_only_are_authority_failures() {
    let policy = policy();
    let job = tool_job();
    let tools = [ToolId::Edit];
    let context = sandbox_context(&policy, &job, &tools);
    let escape = super::invoke(
        &context,
        "call-1",
        "edit",
        &serde_json::json!({"path": "..", "edits": [{"search": "a", "replace": "b"}]}),
    )
    .await;
    assert_eq!(escape.failure, Some(ToolFailureKind::Authority));
    let read_only = super::invoke(
        &context,
        "call-1",
        "edit",
        &serde_json::json!({
            "path": "/access/docs/readme",
            "edits": [{"search": "a", "replace": "b"}]
        }),
    )
    .await;
    assert_eq!(read_only.failure, Some(ToolFailureKind::Authority));
    assert!(read_only.command.is_none());
}

#[tokio::test]
async fn host_run_without_host_context_is_an_authority_failure() {
    let policy = policy();
    let job = tool_job();
    let tools = [ToolId::Run];
    let mut context = sandbox_context(&policy, &job, &tools);
    context.location = ToolLocation::Host;
    let trace = super::invoke(
        &context,
        "call-1",
        "run",
        &serde_json::json!({"command": "echo hi", "explanation": "probe"}),
    )
    .await;
    assert_eq!(trace.failure, Some(ToolFailureKind::Authority));
    assert!(trace.command.is_none());
}
