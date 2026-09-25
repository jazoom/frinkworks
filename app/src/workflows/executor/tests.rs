use super::*;

#[tokio::test]
#[ignore = "Requires an isolated Microsandbox runtime and prepared environment data."]
async fn real_workflow_commands_enforce_mounts_and_approval_without_capture() {
    use crate::execution::{DirectoryAccess, DirectoryGrant, HostApprovalPolicy};
    use crate::providers::{
        ChatBackend, CompletionReason, ModelEvent, ModelSelection, ProviderKind,
    };
    use crate::workflows::definition::{PinnedWorkflowDefinition, WorkflowDefinition};

    let data = std::path::PathBuf::from(
        std::env::var_os("FRINKWORKS_TEST_DATA_DIR").expect("isolated data directory"),
    );
    assert!(
        std::env::var_os("MSB_HOME").is_some(),
        "Use an isolated MSB_HOME"
    );
    for (access, system_only) in [
        (DirectoryAccess::Read, false),
        (DirectoryAccess::Write, false),
        (DirectoryAccess::Read, true),
    ] {
        let mut state = crate::tests::test_state(crate::config::RuntimeConfig::development());
        state.environments = Arc::new(
            crate::environments::EnvironmentCatalogue::open(
                data.join("environments.json"),
                data.join("environment-preparation-logs"),
            )
            .unwrap(),
        );
        state.environment_snapshots = Arc::new(
            crate::environments::EnvironmentSnapshotRepository::open(
                data.join("environment-snapshots"),
            )
            .unwrap(),
        );
        state.sandboxes = Arc::new(crate::sandbox::SandboxFleet::prepare().await);
        assert!(state.sandboxes.missing().is_none());
        let environment = state
            .environments
            .list()
            .into_iter()
            .find(|record| record.name == "Alpine Git")
            .expect("ready Alpine Git environment");
        let root = tempfile::tempdir().unwrap();
        let evidence_dir = tempfile::tempdir().unwrap();
        state.workflow_evidence = Arc::new(
            crate::workflows::evidence::WorkflowEvidenceStore::open(
                evidence_dir.path().to_path_buf(),
            )
            .unwrap(),
        );
        std::fs::write(root.path().join("original"), "original").unwrap();
        std::fs::write(root.path().join("ignored"), "ignored content").unwrap();
        std::fs::write(root.path().join(".gitignore"), "ignored\nlarge\n").unwrap();
        std::fs::File::create(root.path().join("large"))
            .unwrap()
            .set_len(128 * 1024 * 1024)
            .unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(root.path())
                .status()
                .unwrap()
                .success()
        );
        let mut settings = crate::workflows::tests::settings();
        settings.environment = environment.id;
        settings.model = ModelSelection::new(
            ProviderKind::Xai,
            "grok-4.6".into(),
            state
                .models_dev
                .effective_effort(ProviderKind::Xai, "grok-4.6", None),
        )
        .unwrap();
        settings.host_approval = if system_only || access == DirectoryAccess::Write {
            HostApprovalPolicy::AskEachTime
        } else {
            HostApprovalPolicy::Automatic
        };
        let mut grant = DirectoryGrant::from_selected(root.path(), &[]).unwrap();
        grant.access = access;
        settings.directories = vec![grant];
        let connection =
            ProviderConnection::with_key(ProviderKind::Xai, "validation-secret", "grok-4.6");
        state.vault.put(connection.clone()).unwrap();
        let command = if access == DirectoryAccess::Read {
            "set -eu; cat ignored; test $(wc -c < large) -eq 134217728; if printf bad > original; then exit 41; fi; if touch created; then exit 42; fi; if rm original; then exit 43; fi; printf scratch > /workspace/probe; cat /workspace/probe; printf 'validation-%s' secret"
        } else {
            "set -eu; printf changed > original; printf created > created; rm ignored; printf scratch > /workspace/probe; printf 'validation-%s' secret"
        };
        let backend = crate::tests::ScriptedBackend::rounds(vec![
            vec![
                Ok(ModelEvent::ToolCall {
                    id: "run-once".into(),
                    name: "run".into(),
                    arguments: serde_json::json!({"command": command, "explanation": "Test the mount permissions"}),
                }),
                Ok(ModelEvent::Complete {
                    reason: CompletionReason::ToolCalls,
                }),
            ],
            vec![
                Ok(ModelEvent::Text("Complete".into())),
                Ok(ModelEvent::Complete {
                    reason: CompletionReason::Stop,
                }),
            ],
        ]);
        state.chat = Arc::new(ChatBackend::Scripted(backend.clone()));
        let mut definition = crate::workflows::seeds::implement_a_change_definition(environment.id)
            .with_conversation_settings(&settings)
            .unwrap();
        if system_only {
            let mut file = serde_json::to_value(definition.to_file()).unwrap();
            file["roles"] = serde_json::json!([]);
            file["steps"][0]["action"] = serde_json::json!({"type": "system-command", "command": "repository-status", "environment": {"source": "workflow-default"}, "required-outputs": []});
            definition =
                WorkflowDefinition::from_file(serde_json::from_value(file).unwrap()).unwrap();
        }
        let environments = crate::workflows::resolve_environments(
            &definition,
            &state.environments,
            &state.environment_snapshots,
        )
        .await
        .unwrap();
        let token = crate::sessions::generate_session_token().unwrap();
        let session = token.id();
        state.sessions.insert(session);
        let conversation = state
            .conversations
            .create_saved(
                ConversationId::generate().unwrap(),
                Some("Live mount test".into()),
                Some(crate::conversations::ConversationModelConfiguration {
                    settings: settings.clone(),
                    preset: None,
                }),
                vec![],
            )
            .unwrap();
        let phases = if system_only {
            vec![]
        } else {
            vec![crate::workflows::PhaseModelSelection {
                step: definition.first_step().clone(),
                selection: settings.model.clone(),
                instructions: settings.instructions.clone(),
                preset: None,
                settings: Some(settings.clone()),
            }]
        };
        let mut run = crate::workflows::WorkflowRun::create_source_free_for_conversation(
            RunId::generate().unwrap(),
            now_ms(),
            conversation.id,
            PinnedWorkflowDefinition::pin(None, definition),
            environments,
            phases,
            settings.clone(),
        );
        run.launch_brief = "Test the directory permissions".into();
        let approval = state
            .access_consent
            .request_launch(session, conversation.id, vec![settings.clone()])
            .unwrap();
        state
            .access_consent
            .approve_launch(
                &approval,
                run.id,
                session,
                conversation.id,
                vec![settings.clone()],
            )
            .unwrap();
        let job = state
            .sessions
            .begin_conversation_job(&session, conversation.id)
            .unwrap();
        state
            .conversations
            .begin_message_with_model(
                &conversation.id,
                conversation.revision,
                None,
                job.id(),
                "Test permissions".into(),
            )
            .unwrap();
        let authority =
            crate::execution::ProjectFreeAuthority::from_settings(conversation.revision, &settings)
                .unwrap();
        state.workflow_runs.create(run.clone()).unwrap();
        let work = WorkflowJob {
            run_id: run.id,
            session_id: session,
            agent_id: None,
            agent_revision: conversation.revision,
            conversation_id: Some(conversation.id),
            authority: None,
            project_free_authority: Some(authority.clone()),
            connection,
            phase_providers: vec![ProviderKind::Xai],
            active_connection: Arc::new(std::sync::Mutex::new(None)),
            host_policy: authority.policy,
            turns: vec![],
            job: job.clone(),
            eligible_reply: Arc::new(std::sync::Mutex::new(String::new())),
        };
        let task = tokio::spawn(execute_run(
            state.clone(),
            work,
            None,
            state.workflow_execution.acquire().unwrap(),
        ));
        if settings.host_approval == HostApprovalPolicy::AskEachTime {
            let request = tokio::time::timeout(Duration::from_secs(60), async {
                loop {
                    if let Some(request) =
                        state.host_approvals.pending_for(conversation.id, job.id())
                    {
                        break request;
                    }
                    assert!(
                        !task.is_finished(),
                        "workflow ended before approval: {:?}",
                        state.workflow_runs.get(&run.id)
                    );
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(
                std::fs::read_to_string(root.path().join("original")).unwrap(),
                "original"
            );
            state
                .host_approvals
                .decide(&request, crate::execution::HostCommandDecision::Approved)
                .unwrap();
            assert!(
                state
                    .host_approvals
                    .decide(&request, crate::execution::HostCommandDecision::Approved)
                    .is_err()
            );
        }
        tokio::time::timeout(Duration::from_secs(60), task)
            .await
            .unwrap()
            .unwrap();
        let settled = state.workflow_runs.get(&run.id).unwrap();
        assert_eq!(
            settled.state,
            crate::workflows::run::RunState::Completed,
            "{:?}",
            settled
                .attempts
                .last()
                .and_then(|attempt| state.workflow_evidence.get(&run.id, &attempt.id))
        );
        assert_eq!(backend.turn_count(), if system_only { 0 } else { 2 });
        assert_eq!(settled.attempts.len(), 1);
        assert_eq!(
            settled.attempts[0].cleanup,
            crate::workflows::run::AttemptCleanupRecord::Complete
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("original")).unwrap(),
            if access == DirectoryAccess::Write {
                "changed"
            } else {
                "original"
            }
        );
        assert_eq!(
            root.path().join("created").exists(),
            access == DirectoryAccess::Write
        );
        assert_eq!(
            root.path().join("ignored").exists(),
            access == DirectoryAccess::Read
        );
        let evidence = state
            .workflow_evidence
            .get(&run.id, &settled.attempts[0].id)
            .unwrap();
        assert!(
            !serde_json::to_string(&evidence)
                .unwrap()
                .contains("validation-secret")
        );
        let command_records: Vec<_> = std::fs::read_dir(evidence_dir.path().join("host"))
            .unwrap()
            .map(|entry| {
                let bytes = std::fs::read(entry.unwrap().path()).unwrap();
                assert!(!String::from_utf8_lossy(&bytes).contains("validation-secret"));
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
            })
            .collect();
        assert_eq!(command_records.len(), 1);
        let command_record = &command_records[0];
        let result: crate::execution::CommandResult =
            serde_json::from_value(command_record["result"].clone()).unwrap();
        assert!(result.is_success(), "{result:?}");
        let events = command_record["events"].as_array().unwrap();
        let statuses: Vec<_> = events
            .iter()
            .map(|event| event["status"].as_str().unwrap())
            .collect();
        assert_eq!(
            &statuses[..statuses.len() - 1],
            if settings.host_approval.automatic() {
                vec!["automatic", "dispatching"]
            } else {
                vec!["awaiting_approval", "approved", "dispatching"]
            }
        );
        assert!(
            events.windows(2).all(
                |pair| pair[0]["at_ms"].as_u64().unwrap() <= pair[1]["at_ms"].as_u64().unwrap()
            )
        );
        assert!(
            state
                .host_approvals
                .pending_for(conversation.id, job.id())
                .is_none()
        );
        assert!(state.sandboxes.orphans().is_empty());

        let environment = &run.environments.environments[0];
        let artifact = state
            .environment_snapshots
            .restore_path(&environment.snapshot.artifact_key)
            .unwrap();
        let sandbox = state
            .sandboxes
            .attempt_handle(RunId::generate().unwrap(), AttemptId::generate().unwrap());
        let mount = crate::sandbox::MountSpec {
            host: root.path().to_path_buf(),
            guest: "/access/duplicate".into(),
            read_only: true,
        };
        let duplicate = crate::sandbox::SandboxSpec {
            mounts: vec![mount.clone(), mount],
            workdir: "/workspace".into(),
            network: crate::agents::NetworkAccess::None,
        };
        assert!(matches!(
            sandbox
                .start_from_snapshot(
                    &artifact,
                    environment.snapshot.snapshot_digest.as_str(),
                    duplicate
                )
                .await,
            Err(crate::sandbox::SandboxError::StaleMount)
        ));
        sandbox.remove().await.unwrap();
    }
}

#[test]
fn live_mounts_intersect_permissions_without_copy_or_scratch_bind() {
    let data = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let large = std::fs::File::create(root.path().join("large-ignored-file")).unwrap();
    large.set_len(12 * 1024 * 1024 * 1024).unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(root.path(), &[]).unwrap();
    let workspaces =
        crate::workflows::workspace::WorkflowWorkspaces::open(data.path().join("attempts"))
            .unwrap();
    let workspace = workspaces
        .create_attempt(RunId::generate().unwrap(), AttemptId::generate().unwrap())
        .unwrap();
    for access in [
        crate::execution::DirectoryAccess::Read,
        crate::execution::DirectoryAccess::Write,
    ] {
        let mut settings = crate::workflows::tests::settings();
        settings.directories = vec![crate::execution::DirectoryGrant {
            access,
            ..grant.clone()
        }];
        let definition =
            crate::workflows::seeds::implement_a_change_definition(settings.environment)
                .with_conversation_settings(&settings)
                .unwrap();
        let authority =
            crate::execution::ProjectFreeAuthority::from_settings(1, &settings).unwrap();
        let caps = crate::workflows::capabilities::AttemptCapabilities::derive_project_free(
            &definition.steps()[0],
            &authority,
        )
        .unwrap();
        let spec = project_free_attempt_spec(&caps, &workspace, &authority, None).unwrap();
        assert_eq!(spec.mounts.len(), 1);
        assert_eq!(spec.mounts[0].host, grant.host_path);
        assert_eq!(
            spec.mounts[0].read_only,
            access == crate::execution::DirectoryAccess::Read
        );
        assert!(
            std::fs::read_dir(&workspace.project)
                .unwrap()
                .next()
                .is_none()
        );
        assert!(
            spec.mounts
                .iter()
                .all(|mount| mount.guest != crate::execution::GUEST_WORKSPACE)
        );
    }
}

#[test]
fn forged_write_capabilities_cannot_upgrade_read_mounts() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let root = tempfile::tempdir().unwrap();
    let mut settings = crate::workflows::tests::settings();
    settings.directories =
        vec![crate::execution::DirectoryGrant::from_selected(root.path(), &[]).unwrap()];
    let authority = crate::execution::ProjectFreeAuthority::from_settings(1, &settings).unwrap();
    let definition = crate::workflows::seeds::plan_a_change_definition(settings.environment)
        .with_conversation_settings(&settings)
        .unwrap();
    let mut caps = crate::workflows::capabilities::AttemptCapabilities::derive_project_free(
        &definition.steps()[0],
        &authority,
    )
    .unwrap();
    caps.directories[0].access = AccessMode::ReadWrite;
    let workspace = state
        .workflow_workspaces
        .create_attempt(RunId::generate().unwrap(), AttemptId::generate().unwrap())
        .unwrap();
    assert!(project_free_attempt_spec(&caps, &workspace, &authority, None).is_err());
}

#[test]
fn initial_planner_needs_no_source_capture_or_existing_plan() {
    let mut run = crate::workflows::tests::run(
        crate::workflows::seeds::plan_then_implement_definition(crate::tests::test_environment_id()),
    );
    let planner = &run.pinned.definition.steps()[0];
    assert!(resolve_inputs(&run, planner).unwrap().is_empty());
    let store = crate::workflows::WorkflowArtefactRepository::in_memory();
    assert!(crate::workflows::input_context::verify_inputs(&run, planner, &[], &store).is_ok());
    let plan = crate::workflows::tests::complete_plan(&mut run, &store, "Plan", 2);
    let gate = &run.pinned.definition.steps()[1];
    assert_eq!(resolve_inputs(&run, gate).unwrap()[0].artefact, plan);
}

#[test]
fn pre_attempt_failure_is_durable_without_a_fake_attempt() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let run = crate::workflows::tests::run(crate::tests::test_named_definition("Failure"));
    state.workflow_runs.create(run.clone()).unwrap();
    state.workflow_runs.fail_next_mutation();
    assert!(persist_fail(&state, &run.id, None, FailureCategory::Authority).is_err());
    assert_eq!(state.workflow_runs.get(&run.id).unwrap(), run);
    persist_fail(&state, &run.id, None, FailureCategory::Authority).unwrap();
    let failed = state.workflow_runs.get(&run.id).unwrap();
    assert_eq!(failed.state, crate::workflows::run::RunState::Failed);
    assert!(failed.attempts.is_empty());
    let dir = tempfile::tempdir().unwrap();
    let store = crate::workflows::WorkflowRunStore::open(dir.path().to_owned()).unwrap();
    store.create(failed.clone()).unwrap();
    assert_eq!(
        crate::workflows::WorkflowRunStore::open(dir.path().to_owned())
            .unwrap()
            .get(&run.id),
        Some(failed)
    );
}
