use super::{
    AgentToolContext, MAXIMUM_TOOL_BYTES, ToolFailureKind, authorised_tool, definitions_for,
    mark_truncated, redact, valid_output_reference,
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
    for location in [ToolLocation::Sandbox, ToolLocation::Host] {
        for selected in [ToolId::ALL.as_slice(), &[ToolId::Read, ToolId::List], &[]] {
            let definitions = definitions_for(selected, location);
            assert_eq!(definitions.len(), selected.len());
            for definition in definitions {
                assert_eq!(
                    authorised_tool(selected, &definition.name),
                    ToolId::parse(&definition.name)
                );
            }
            assert_eq!(authorised_tool(selected, "forged"), None);
        }
    }
    assert_eq!(authorised_tool(&[ToolId::Read], "write"), None);
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
        advertised_resources: &[],
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
        questions: None,
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
async fn host_tools_without_host_context_are_authority_failures() {
    let policy = policy();
    let job = tool_job();
    let mut context = sandbox_context(&policy, &job, &ToolId::ALL);
    context.location = ToolLocation::Host;
    for tool in ToolId::ALL {
        let trace = super::invoke(&context, "call-1", tool.as_str(), &serde_json::json!({})).await;
        assert_eq!(trace.failure, Some(ToolFailureKind::Authority));
        assert!(trace.command.is_none());
    }
}

struct HostFixture {
    state: crate::state::AppState,
    settings: crate::execution::ExecutionSettings,
    session: crate::sessions::SessionId,
    record: crate::conversations::ConversationRecord,
    job: std::sync::Arc<Job>,
    policy: DirectoryPolicy,
}

impl HostFixture {
    fn new(directory: &std::path::Path, global: Option<&std::path::Path>) -> Self {
        let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
        let settings = crate::execution::ExecutionSettings::new(
            crate::providers::ModelSelection::new(
                crate::providers::ProviderKind::Xai,
                "grok-4.6".to_owned(),
                None,
            )
            .unwrap(),
            String::new(),
            vec![ToolId::List, ToolId::Read, ToolId::Edit, ToolId::Write],
            crate::tests::test_environment_id(),
        )
        .unwrap()
        .with_location(ToolLocation::Host)
        .with_directories(vec![
            crate::execution::DirectoryGrant::from_selected(directory, &[]).unwrap(),
        ])
        .unwrap();
        let session = crate::sessions::generate_session_token().unwrap().id();
        state.sessions.insert(session);
        let record = state
            .conversations
            .create("Host file tools".to_owned())
            .unwrap();
        let record = state
            .conversations
            .update_execution_settings(&record.id, record.revision, settings.clone())
            .unwrap();
        let job = state
            .sessions
            .begin_conversation_job(&session, record.id)
            .unwrap();
        let record = state
            .conversations
            .begin_message_with_model(
                &record.id,
                record.revision,
                None,
                job.id(),
                "Use file tools".to_owned(),
            )
            .unwrap();
        let policy = crate::execution::ProjectFreeAuthority::from_settings(1, &settings)
            .unwrap()
            .policy
            .with_skill_root(crate::execution::global_skill_root(global))
            .on_host(directory);
        Self {
            state,
            settings,
            session,
            record,
            job,
            policy,
        }
    }

    fn approve(&self) {
        let token = self
            .state
            .access_consent
            .request_host_conversation(self.session, self.record.id, &self.settings)
            .unwrap();
        self.state
            .access_consent
            .approve_host_conversation(&token, self.session, self.record.id, &self.settings)
            .unwrap();
    }

    fn context(&self) -> AgentToolContext<'_> {
        let mut context = sandbox_context(&self.policy, &self.job, &self.settings.tools);
        context.location = ToolLocation::Host;
        context.secret = Some("sk-secret");
        context.host = Some(super::HostToolContext {
            state: &self.state,
            settings: &self.settings,
            secret: context.secret,
            session: self.session,
            conversation: self.record.id,
            execution_revision: self.record.revision,
            directory: self.settings.directories[0].host_path.clone(),
            run: None,
            step: None,
            attempt: None,
        });
        context
    }
}

#[tokio::test]
async fn host_file_tools_require_live_consent_without_run_authority() {
    let root = tempfile::tempdir().unwrap();
    let fixture = HostFixture::new(root.path(), None);
    assert!(fixture.settings.host_tools());
    let args = serde_json::json!({"path": "note.txt", "contents": "original"});
    let denied = super::invoke(&fixture.context(), "write", "write", &args).await;
    assert_eq!(denied.failure, Some(ToolFailureKind::Authority));
    assert!(!root.path().join("note.txt").exists());
    fixture.approve();
    let written = super::invoke(&fixture.context(), "write", "write", &args).await;
    assert_eq!(written.failure, None, "{}", written.output);
    fixture
        .state
        .access_consent
        .invalidate_conversation(fixture.record.id);
    let denied = super::invoke(
        &fixture.context(),
        "read",
        "read",
        &serde_json::json!({"path": "note.txt"}),
    )
    .await;
    assert_eq!(denied.failure, Some(ToolFailureKind::Authority));
}

#[tokio::test]
async fn host_file_tools_preserve_literal_paths_paging_and_edit_validation() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let fixture = HostFixture::new(root.path(), None);
    fixture.approve();
    let context = fixture.context();
    let path = "nested/quote' : $(false).txt";
    let written = super::invoke(
        &context,
        "write",
        "write",
        &serde_json::json!({
            "path": path, "contents": "one\nsk-secret\nthree\n"
        }),
    )
    .await;
    assert_eq!(written.failure, None, "{}", written.output);
    let read = super::invoke(
        &context,
        "read",
        "read",
        &serde_json::json!({"path": path, "offset": 2, "limit": 1}),
    )
    .await;
    assert_eq!(read.failure, None, "{}", read.output);
    let visible = redact(&read.output, context.secret);
    assert!(visible.contains("[redacted]"));
    assert!(!visible.contains("sk-secret"));
    assert!(!read.output.contains("three"));
    let invalid = super::invoke(
        &context,
        "edit",
        "edit",
        &serde_json::json!({"path": path, "edits": [{"search": "absent", "replace": "x"}]}),
    )
    .await;
    assert_eq!(invalid.failure, Some(ToolFailureKind::Ordinary));
    assert_eq!(
        std::fs::read_to_string(root.path().join(path)).unwrap(),
        "one\nsk-secret\nthree\n"
    );
    let edited = super::invoke(
        &context,
        "edit",
        "edit",
        &serde_json::json!({"path": path, "edits": [{"search": "three", "replace": "four"}]}),
    )
    .await;
    assert_eq!(edited.failure, None, "{}", edited.output);
    assert_eq!(
        std::fs::read_to_string(root.path().join(path)).unwrap(),
        "one\nsk-secret\nfour\n"
    );
    let listed = super::invoke(
        &context,
        "list",
        "list",
        &serde_json::json!({"path": "nested"}),
    )
    .await;
    assert_eq!(listed.failure, None, "{}", listed.output);
    assert!(listed.output.contains("$(false).txt"));
    let external_path = outside.path().join("host.txt");
    let external = super::invoke(
        &context,
        "external",
        "write",
        &serde_json::json!({"path": external_path, "contents": "direct"}),
    )
    .await;
    assert_eq!(external.failure, None, "{}", external.output);
    assert_eq!(std::fs::read_to_string(external_path).unwrap(), "direct");
    let denied = super::invoke(
        &context,
        "run",
        "run",
        &serde_json::json!({"command": "true", "explanation": "Not selected"}),
    )
    .await;
    assert_eq!(denied.failure, Some(ToolFailureKind::Authority));
}

#[tokio::test]
async fn host_skills_preserve_discovered_identity_and_reject_credentials() {
    let root = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let project_file = root.path().join(".agents/skills/local/SKILL.md");
    let global_file = global.path().join("global/SKILL.md");
    let text = "---\nname: test\ndescription: Test guidance\n---\nRead this body.\n";
    for file in [&project_file, &global_file] {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let fixture = HostFixture::new(root.path(), Some(global.path()));
    fixture.approve();
    let skills = crate::execution::discover_skills(None, &fixture.policy, None)
        .await
        .unwrap();
    assert_eq!(skills.len(), 2);
    let sources = skills
        .iter()
        .map(|skill| skill.source.clone())
        .collect::<Vec<_>>();
    let mut context = fixture.context();
    context.advertised_resources = &sources;
    for skill in &skills {
        let read = super::invoke(
            &context,
            "skill",
            "read",
            &serde_json::json!({"path": skill.read_path}),
        )
        .await;
        assert_eq!(read.failure, None, "{}", read.output);
        assert_eq!(read.resource.as_ref(), Some(&skill.source));
        assert!(read.output.contains("Read this body."));
    }
    std::fs::write(&global_file, text.replace("body", "changed body")).unwrap();
    let changed = super::invoke(
        &context,
        "changed",
        "read",
        &serde_json::json!({"path": global_file}),
    )
    .await;
    assert_eq!(changed.failure, Some(ToolFailureKind::Ordinary));
    assert!(!changed.output.contains("changed body"));
    std::fs::write(&project_file, text.replace("body", "sk-secret")).unwrap();
    context.advertised_resources = &[];
    let secret = super::invoke(
        &context,
        "secret",
        "read",
        &serde_json::json!({"path": project_file}),
    )
    .await;
    assert_eq!(secret.failure, Some(ToolFailureKind::Ordinary));
    assert!(secret.resource.is_none());
    assert!(!secret.output.contains("sk-secret"));
}
