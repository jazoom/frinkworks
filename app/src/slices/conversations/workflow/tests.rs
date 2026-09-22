use super::*;
use crate::agents::AccessMode;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use tower::ServiceExt;

fn directory_settings() -> crate::execution::ExecutionSettings {
    crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(
            crate::providers::ProviderKind::Xai,
            "grok-4.6".to_owned(),
            None,
        )
        .unwrap(),
        "Use the pinned instructions.".to_owned(),
        crate::agents::ToolId::ALL.to_vec(),
        crate::tests::test_environment_id(),
    )
    .unwrap()
}

#[test]
fn directory_launch_pins_non_git_roots_and_read_only_review_authority() {
    let root = tempfile::tempdir().unwrap();
    let mut grants = Vec::new();
    for name in ["first", "second"] {
        let path = root.path().join(name);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("same.txt"), name).unwrap();
        let mut grant = crate::execution::DirectoryGrant::from_selected(&path, &grants).unwrap();
        grant.access = crate::execution::DirectoryAccess::ReviewBeforeApply;
        grants.push(grant);
    }
    let mut settings = directory_settings().with_directories(grants).unwrap();
    let definition = workflows::seeds::implement_and_review_definition(settings.environment)
        .with_conversation_settings(&settings)
        .unwrap();
    let authority = crate::execution::ProjectFreeAuthority::from_settings(1, &settings).unwrap();
    let reviewer = &definition.steps()[1];
    let capabilities =
        workflows::capabilities::AttemptCapabilities::derive_project_free(reviewer, &authority)
            .unwrap();
    assert!(
        capabilities
            .directories
            .iter()
            .all(|directory| directory.access == AccessMode::ReadOnly)
    );
    assert!(matches!(&definition.steps().last().unwrap().action,
        workflows::definition::StepAction::SystemCommand(action)
            if action.command == workflows::commands::SystemCommandId::ApplyChanges));
    let state = connected_state();
    let source = workflows::artefacts::CandidateCapture::capture_set(
        &settings.directories,
        &root.path().join("data"),
        &state.workflow_artefacts,
    )
    .unwrap();
    assert_eq!(source.roots.len(), 2);
    let phases = phase_steps(&definition)
        .into_iter()
        .map(|step| PhaseModelSelection {
            step: step.key.clone(),
            selection: settings.model.clone(),
            instructions: settings.instructions.clone(),
            preset: None,
            settings: Some(settings.clone()),
        })
        .collect();
    let pinned = workflows::definition::PinnedWorkflowDefinition::pin(None, definition);
    let environments = crate::tests::test_environment_set(&pinned.definition);
    let mut run = WorkflowRun::create_source_free_for_conversation(
        workflows::RunId::generate().unwrap(),
        1,
        crate::conversations::ConversationId::generate().unwrap(),
        pinned,
        environments,
        phases,
    );
    run.kind = workflows::run::RunKind::Configured;
    run.launch_brief = "Update both files".to_owned();
    let bytes = source.manifest_bytes().unwrap();
    let kind = workflows::definition::ArtefactKind::CandidateRevision;
    let artefact = workflows::artefacts::ArtefactRecord {
        id: workflows::ArtefactId::generate().unwrap(),
        kind,
        artefact_hash: workflows::artefacts::artefact_hash_for(
            kind,
            workflows::artefacts::CANDIDATE_SCHEMA,
            &bytes,
        ),
        object_hash: state.workflow_artefacts.publish(&bytes).unwrap(),
        payload_bytes: bytes.len() as u64,
        created_at_ms: 1,
        provenance: workflows::artefacts::ArtefactProvenance {
            run_id: run.id,
            producer: workflows::artefacts::ArtefactProducer::RunSourceCapture,
            inputs: Vec::new(),
        },
        summary: workflows::artefacts::ArtefactSummary::Candidate {
            candidate: source.candidate_hash,
            entries: 2,
            bytes: 11,
            disposition: workflows::artefacts::ProductionDisposition::RequiredOutput,
        },
    };
    let reference = workflows::artefacts::ArtefactReference {
        id: artefact.id,
        kind,
        artefact_hash: artefact.artefact_hash,
    };
    run.record_initial_candidate(artefact).unwrap();
    let step = run.pinned.definition.steps()[0].clone();
    let capabilities =
        workflows::capabilities::AttemptCapabilities::derive_project_free(&step, &authority)
            .unwrap();
    run.start_attempt(
        workflows::AttemptId::generate().unwrap(),
        vec![workflows::run::AttemptArtefactInput {
            key: workflows::definition::InputKey::parse("candidate").unwrap(),
            artefact: reference,
        }],
        capabilities,
        workflows::run::AttemptSandboxRecord {
            kind: workflows::run::AttemptSandboxKind::IsolatedAttempt,
            snapshot_digest: run.environments.steps[0].snapshot_digest.clone(),
        },
        2,
    )
    .unwrap();
    settings.directories.clear();
    settings.instructions.clear();
    state.workflow_runs.create(run.clone()).unwrap();
    let loaded = state.workflow_runs.get(&run.id).unwrap();
    assert_eq!(loaded.directory_settings().unwrap().directories.len(), 2);
    assert_eq!(
        loaded.directory_settings().unwrap().instructions,
        "Use the pinned instructions."
    );
    assert!(loaded.agent_id.is_none());
    std::fs::rename(root.path().join("second"), root.path().join("old-second")).unwrap();
    std::fs::create_dir(root.path().join("second")).unwrap();
    assert!(
        crate::execution::ProjectFreeAuthority::from_settings(
            1,
            &loaded.directory_settings().unwrap()
        )
        .is_err()
    );
}

#[test]
fn source_free_plans_have_no_candidate_and_missing_review_sources_fail_before_execution() {
    let settings = directory_settings();
    let definition = workflows::seeds::plan_a_change_definition(settings.environment)
        .with_conversation_settings(&settings)
        .unwrap();
    assert!(definition.steps().iter().all(|step| step.inputs.is_empty()));
    assert!(
        workflows::seeds::review_current_code_definition(settings.environment)
            .with_conversation_settings(&settings)
            .is_err()
    );
    assert!(
        workflows::seeds::implement_with_approval_definition(settings.environment)
            .with_conversation_settings(&settings)
            .is_err()
    );
    let authority = crate::execution::ProjectFreeAuthority::from_settings(1, &settings).unwrap();
    let capabilities = workflows::capabilities::AttemptCapabilities::derive_project_free(
        &definition.steps()[0],
        &authority,
    )
    .unwrap();
    assert!(authority.policy.grants().is_empty());
    assert_eq!(capabilities.primary().unwrap().guest_path, "/workspace");
    let phases = vec![PhaseModelSelection {
        step: definition.first_step().clone(),
        selection: settings.model.clone(),
        instructions: settings.instructions.clone(),
        preset: None,
        settings: Some(settings),
    }];
    let pinned = workflows::definition::PinnedWorkflowDefinition::pin(None, definition);
    let environments = crate::tests::test_environment_set(&pinned.definition);
    let mut run = WorkflowRun::create_source_free_for_conversation(
        workflows::RunId::generate().unwrap(),
        1,
        crate::conversations::ConversationId::generate().unwrap(),
        pinned,
        environments,
        phases,
    );
    run.kind = workflows::run::RunKind::Configured;
    run.launch_brief = "Prepare a plan".to_owned();
    let state = connected_state();
    state.workflow_runs.create(run.clone()).unwrap();
    let loaded = state.workflow_runs.get(&run.id).unwrap();
    assert!(matches!(loaded.source, workflows::RunSource::None));
    assert!(loaded.artefacts.is_empty());
}

#[test]
fn phase_authority_replaces_defaults_and_allows_read_only_narrowing() {
    let root = tempfile::tempdir().unwrap();
    let mut grants = Vec::new();
    for name in ["first/shared", "second/shared", "unused"] {
        let path = root.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        grants.push(crate::execution::DirectoryGrant::from_selected(&path, &[]).unwrap());
    }
    let mut defaults = directory_settings();
    defaults.directories = vec![grants[2].clone()];
    let definition = workflows::seeds::implement_and_review_definition(defaults.environment);
    let mut worker = defaults.clone();
    worker.directories = vec![grants[0].clone()];
    worker.directories[0].access = crate::execution::DirectoryAccess::ReviewBeforeApply;
    worker.environment = crate::environments::EnvironmentId::generate().unwrap();
    worker.network = crate::agents::NetworkAccess::Public;
    let mut reviewer = defaults.clone();
    reviewer.directories = vec![grants[0].clone(), grants[1].clone()];
    reviewer.directories[1].alias = "reference".to_owned();
    reviewer.tools = vec![crate::agents::ToolId::Read];
    let mut phases: Vec<_> = phase_steps(&definition)
        .into_iter()
        .zip([worker, reviewer])
        .map(|(step, settings)| PhaseModelSelection {
            step: step.key.clone(),
            selection: settings.model.clone(),
            instructions: settings.instructions.clone(),
            preset: None,
            settings: Some(settings),
        })
        .collect();
    resolve_directory_phase_settings(&definition, &defaults, &mut phases).unwrap();
    let merged = merged_phase_settings(&defaults, &phases);
    assert_eq!(merged.directories.len(), 2);
    assert!(
        !merged
            .directories
            .iter()
            .any(|grant| grant.identity == grants[2].identity)
    );
    assert_eq!(
        merged.directories[0].access,
        crate::execution::DirectoryAccess::ReviewBeforeApply
    );
    let snapshots: Vec<_> = phases
        .iter()
        .map(|phase| (phase.step.clone(), phase.settings.clone().unwrap()))
        .collect();
    let pinned = definition
        .with_phase_settings(&defaults, &snapshots)
        .unwrap();
    assert_ne!(
        pinned.effective_environment(&pinned.steps()[0]),
        defaults.environment
    );
    assert_eq!(
        pinned.effective_environment(&pinned.steps()[1]),
        defaults.environment
    );
    let reviewer_authority = crate::execution::ProjectFreeAuthority::from_settings(
        1,
        phases[1].settings.as_ref().unwrap(),
    )
    .unwrap();
    let capabilities = workflows::capabilities::AttemptCapabilities::derive_project_free(
        &pinned.steps()[1],
        &reviewer_authority,
    )
    .unwrap();
    assert_eq!(capabilities.tools, vec![crate::agents::ToolId::Read]);
    assert!(
        capabilities
            .directories
            .iter()
            .all(|directory| directory.access == AccessMode::ReadOnly)
    );
    assert_eq!(
        capabilities.network,
        workflows::capabilities::NetworkCapability::None
    );
}

#[test]
fn sensitive_workflow_launch_needs_live_destination_consent() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let data = home.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let mut state = connected_state();
    state.local_data = crate::local_data::LocalDataReset::for_test(data);
    let grant = crate::execution::DirectoryGrant::from_selected(&home, &[]).unwrap();
    let settings = directory_settings()
        .with_directories(vec![grant.clone()])
        .unwrap();
    let record = state
        .conversations
        .create_saved(
            crate::conversations::ConversationId::generate().unwrap(),
            None,
            Some(crate::conversations::ConversationModelConfiguration {
                settings: settings.clone(),
                preset: None,
            }),
            Vec::new(),
        )
        .unwrap();
    let session = crate::sessions::generate_session_token().unwrap().id();
    state.sessions.insert(session);
    let run = workflows::RunId::generate().unwrap();
    let other_run = workflows::RunId::generate().unwrap();
    let consent = &state.access_consent;
    assert!(!consent.authorised_launch(run, session, record.id, &settings));
    let request = consent
        .request_launch(session, record.id, vec![settings.clone()])
        .unwrap();
    let mut changed = settings.clone();
    changed.network = crate::agents::NetworkAccess::Public;
    assert!(
        consent
            .approve_launch(&request, run, session, record.id, vec![changed])
            .is_err()
    );
    consent
        .approve_launch(&request, run, session, record.id, vec![settings.clone()])
        .unwrap();
    assert!(consent.authorised_launch(run, session, record.id, &settings));
    assert!(!consent.authorised_launch(other_run, session, record.id, &settings));
    assert!(
        consent
            .approve_launch(
                &request,
                other_run,
                session,
                record.id,
                vec![settings.clone()]
            )
            .is_err()
    );
    assert!(!consent.authorised_conversation(session, record.id, &settings, &grant));
    state.access_consent.retain_sessions(|_| false);
    assert!(!consent.authorised_launch(run, session, record.id, &settings));
}

#[test]
fn direct_workflow_consent_binds_run_and_exact_phase_strategy() {
    use crate::execution::{DirectoryAccess, DirectoryGrant};
    let root = tempfile::tempdir().unwrap();
    let mut grant = DirectoryGrant::from_selected(root.path(), &[]).unwrap();
    grant.access = DirectoryAccess::DirectWrite;
    let settings = directory_settings()
        .with_directories(vec![grant.clone()])
        .unwrap();
    let definition = workflows::seeds::plan_a_change_definition(settings.environment)
        .with_conversation_settings(&settings)
        .unwrap();
    let authority = crate::execution::ProjectFreeAuthority::from_settings(1, &settings).unwrap();
    let capabilities = workflows::capabilities::AttemptCapabilities::derive_project_free(
        &definition.steps()[0],
        &authority,
    )
    .unwrap();
    assert_eq!(capabilities.directories[0].access, AccessMode::ReadWrite);
    assert!(definition.steps()[0].inputs.is_empty());
    let consent = crate::execution::AccessConsentStore::default();
    let session = crate::sessions::generate_session_token().unwrap().id();
    let conversation = crate::conversations::ConversationId::generate().unwrap();
    let run = workflows::RunId::generate().unwrap();
    let preview = consent
        .request_launch(session, conversation, vec![settings.clone()])
        .unwrap();
    let mut changed = settings.clone();
    changed.directories[0].access = DirectoryAccess::ReviewBeforeApply;
    assert!(
        consent
            .approve_launch(&preview, run, session, conversation, vec![changed.clone()])
            .is_err()
    );
    consent
        .approve_launch(&preview, run, session, conversation, vec![settings.clone()])
        .unwrap();
    assert!(consent.authorised_launch(run, session, conversation, &settings));
    assert!(!consent.authorised_launch(run, session, conversation, &changed));
    assert!(!consent.authorised_launch(
        workflows::RunId::generate().unwrap(),
        session,
        conversation,
        &settings
    ));
    assert!(!consent.authorised_conversation(session, conversation, &settings, &grant));
    assert!(
        consent
            .approve_launch(&preview, run, session, conversation, vec![settings.clone()])
            .is_err()
    );
    let parent_preview = consent
        .request_conversation(session, conversation, &settings, &grant)
        .unwrap();
    consent
        .approve_conversation(&parent_preview, session, conversation, &settings, &grant)
        .unwrap();
    assert!(consent.authorised_conversation(session, conversation, &settings, &grant));
    assert!(!consent.authorised_launch(
        workflows::RunId::generate().unwrap(),
        session,
        conversation,
        &settings
    ));
    consent.retain_sessions(|_| false);
    assert!(!consent.authorised_launch(run, session, conversation, &settings));
}

fn connected_state() -> AppState {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    state
        .vault
        .put(crate::providers::ProviderConnection::with_key(
            crate::providers::ProviderKind::Xai,
            "test-key",
            "grok-4.6",
        ))
        .expect("provider");
    state
}

#[tokio::test]
async fn chooser_lists_starters_in_approved_order_with_custom_workflows_last() {
    let state = connected_state();
    let environment = crate::tests::test_environment_id();
    let mut seeds = workflows::seeds::production_seeds(environment);
    seeds.reverse();
    for seed in seeds {
        state.workflows.create(seed.definition).expect("seed");
    }
    let custom = workflows::seeds::one_agent_definition(environment);
    state.workflows.create(custom).expect("custom");
    let conversation = state
        .conversations
        .create("Ordering".to_owned())
        .expect("conversation");
    let view = launch_view(&state, &conversation, None, "", "", &[], "").await;
    let names: Vec<_> = view
        .workflows
        .iter()
        .map(|workflow| workflow.name.clone())
        .collect();
    assert_eq!(
        names,
        vec![
            "Plan a change".to_owned(),
            "Review current code".to_owned(),
            "Implement with approval".to_owned(),
            "Implement and review".to_owned(),
            "Plan then implement".to_owned(),
            "One agent".to_owned(),
        ]
    );
    assert!(
        !view
            .workflows
            .iter()
            .any(|workflow| workflow.summary.is_empty())
    );
    assert!(
        view.workflows
            .iter()
            .all(|workflow| !workflow.process_phases.is_empty())
    );
}

#[tokio::test]
async fn host_preview_does_not_describe_sandbox_boundaries() {
    let state = connected_state();
    let record = state
        .conversations
        .create("Host preview".to_owned())
        .unwrap();
    let settings = directory_settings()
        .with_location(crate::execution::ToolLocation::Host)
        .with_host_approval(crate::execution::HostApprovalPolicy::Automatic);
    let record = state
        .conversations
        .update_execution_settings(&record.id, record.revision, settings)
        .unwrap();
    let workflow = state
        .workflows
        .create(workflows::seeds::plan_a_change_definition(
            crate::tests::test_environment_id(),
        ))
        .unwrap();
    let selection = WorkflowSelection {
        workflow_id: workflow.id,
        definition_version: workflow.definition_version,
    }
    .as_token();
    let (_, access, _) = launch_readiness(&state, &record, &selection).await;
    assert!(access.contains("Unrestricted host access"));
    assert!(access.contains("Run without approval"));
    assert!(!access.contains("/workspace"));
    assert!(!access.contains("Sandbox network"));
}

#[tokio::test]
async fn launch_rejects_stale_definitions_without_reserving_the_conversation() {
    let state = connected_state();
    let conversation = state
        .conversations
        .create("Workflow".to_owned())
        .expect("conversation");
    let mut seeds =
        workflows::seeds::production_seeds(crate::tests::test_environment_id()).into_iter();
    let workflow = state
        .workflows
        .create(seeds.next().expect("seed").definition)
        .expect("workflow");
    let selection = WorkflowSelection {
        workflow_id: workflow.id,
        definition_version: workflow.definition_version,
    }
    .as_token();
    state
        .workflows
        .update(
            &workflow.id,
            workflow.revision,
            seeds.next().expect("seed").definition,
        )
        .expect("update");
    let token = crate::sessions::generate_session_token().expect("session");
    state.sessions.insert(token.id());
    let app = crate::slices::router()
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::sessions::resolve_session,
        ))
        .layer(axum::middleware::from_fn(hypergraft::middleware::classify))
        .with_state(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/conversations/{}/workflow",
                    conversation.id.as_hex()
                ))
                .header(
                    header::COOKIE,
                    format!("powerplant_session={}", token.raw().as_str()),
                )
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from(format!(
                    "revision={}&workflow={selection}&brief=Inspect&phase=first&phase=second",
                    conversation.revision
                )))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("body")
            .to_vec(),
    )
    .expect("text");
    assert!(body.contains(ResolveWorkflowError::Changed.message()));
    assert_eq!(
        state
            .conversations
            .get(&conversation.id)
            .expect("conversation"),
        conversation
    );
    assert!(!state.sessions.busy(&token.id()));
    assert!(
        state
            .workflow_runs
            .for_conversation(&conversation.id)
            .is_empty()
    );
}

#[test]
fn host_workflow_consent_binds_the_destination_and_approval_policy() {
    let mut settings = directory_settings()
        .with_location(crate::execution::ToolLocation::Host)
        .with_host_approval(crate::execution::HostApprovalPolicy::Automatic);
    settings.tools = vec![crate::agents::ToolId::Run];
    let consent = crate::execution::AccessConsentStore::default();
    let session = crate::sessions::generate_session_token().unwrap().id();
    let conversation = crate::conversations::ConversationId::generate().unwrap();
    let run = workflows::RunId::generate().unwrap();
    let other = workflows::RunId::generate().unwrap();
    consent
        .approve_host_conversation(
            &consent
                .request_host_conversation(session, conversation, &settings)
                .unwrap(),
            session,
            conversation,
            &settings,
        )
        .unwrap();
    assert!(consent.authorised_host_conversation(session, conversation, &settings));
    assert!(!consent.authorised_launch(run, session, conversation, &settings));
    let preview = consent
        .request_launch(session, conversation, vec![settings.clone()])
        .unwrap();
    consent
        .approve_launch(&preview, run, session, conversation, vec![settings.clone()])
        .unwrap();
    assert!(consent.authorised_launch(run, session, conversation, &settings));
    assert!(!consent.authorised_launch(other, session, conversation, &settings));
    let mut ask = settings.clone();
    ask.host_approval = crate::execution::HostApprovalPolicy::AskEachTime;
    assert!(!consent.authorised_launch(run, session, conversation, &ask));
}

#[tokio::test]
async fn unavailable_environments_block_sandbox_launch_without_substitution() {
    let state = connected_state();
    let scratch = tempfile::tempdir().expect("scratch");
    let mut grant =
        crate::execution::DirectoryGrant::from_selected(scratch.path(), &[]).expect("grant");
    grant.access = crate::execution::DirectoryAccess::ReviewBeforeApply;
    let mut settings = directory_settings()
        .with_directories(vec![grant])
        .expect("directories");
    settings.model.thinking =
        state
            .models_dev
            .effective_effort(settings.model.provider, &settings.model.model, None);
    let conversation = state
        .conversations
        .create("Sandbox block".to_owned())
        .expect("conversation");
    let conversation = state
        .conversations
        .update_execution_settings(&conversation.id, conversation.revision, settings.clone())
        .expect("settings");
    let workflow = state
        .workflows
        .create(workflows::seeds::implement_and_review_definition(
            crate::tests::test_environment_id(),
        ))
        .expect("workflow");
    let selection = WorkflowSelection {
        workflow_id: workflow.id,
        definition_version: workflow.definition_version,
    }
    .as_token();
    let (_, _, environment_summary) = launch_readiness(&state, &conversation, &selection).await;
    assert_eq!(
        environment_summary,
        "That environment is no longer in the catalogue."
    );
    let definition = state
        .workflows
        .resolve(&WorkflowSelection::parse(&selection).expect("selection"))
        .expect("resolved")
        .pinned
        .definition
        .clone();
    let phases: Vec<String> = super::phase_steps(&definition)
        .into_iter()
        .map(|step| super::phase_choice_token(step.key.as_str(), &settings.model, None))
        .collect();
    let token = crate::sessions::generate_session_token().expect("session");
    state.sessions.insert(token.id());
    let mut serialiser = url::form_urlencoded::Serializer::new(String::new());
    serialiser.append_pair("revision", &conversation.revision.to_string());
    serialiser.append_pair("workflow", &selection);
    serialiser.append_pair("preview_workflow", &selection);
    serialiser.append_pair("brief", "Inspect the code");
    for phase in &phases {
        serialiser.append_pair("phase", phase);
    }
    let body = serialiser.finish();
    let app = crate::slices::router()
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::sessions::resolve_session,
        ))
        .layer(axum::middleware::from_fn(hypergraft::middleware::classify))
        .with_state(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/conversations/{}/workflow",
                    conversation.id.as_hex()
                ))
                .header(
                    header::COOKIE,
                    format!("powerplant_session={}", token.raw().as_str()),
                )
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from(body))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("body")
            .to_vec(),
    )
    .expect("text");
    assert!(body.contains("That environment is no longer in the catalogue."));
    assert!(body.contains("href=\"/environments\""));
    assert!(
        state
            .workflow_runs
            .for_conversation(&conversation.id)
            .is_empty()
    );
    assert_eq!(
        state
            .conversations
            .get(&conversation.id)
            .expect("conversation")
            .model
            .expect("model")
            .settings
            .environment,
        crate::tests::test_environment_id()
    );
}

#[tokio::test]
async fn brief_defaults_to_the_conversation_request_not_sample_text() {
    let state = connected_state();
    let mut conversation = state
        .conversations
        .create("Ordering".to_owned())
        .expect("conversation");
    conversation.messages = vec![crate::conversations::ConversationMessage {
        parent: None,
        id: crate::conversations::MessageId::generate().expect("message id"),
        response: None,
        final_phase: false,
        continuation: Vec::new(),
        role: crate::conversations::MessageRole::User,
        text: "  Make errors helpful.  ".to_owned(),
        input: None,
        attachments: Vec::new(),
        activity: Vec::new(),
        status: crate::conversations::MessageStatus::Complete,
        error: None,
        request: None,
        completion: None,
        requests: Vec::new(),
    }];
    let brief = async |brief: &str, record: &ConversationRecord| {
        launch_view(&state, record, None, brief, "", &[], "")
            .await
            .brief
            .clone()
    };
    assert_eq!(brief("", &conversation).await, "Make errors helpful.");
    assert_eq!(
        brief("Explicit direction", &conversation).await,
        "Explicit direction"
    );
    let empty = state
        .conversations
        .create("Empty".to_owned())
        .expect("conversation");
    assert!(brief("", &empty).await.is_empty());
}

#[tokio::test]
async fn review_reports_automatic_commit_without_promising_a_human_decision() {
    let state = connected_state();
    let environment = crate::tests::test_environment_id();
    let source = workflows::seeds::implement_and_review_definition(environment);
    let mut steps = source.steps().to_vec();
    for step in &mut steps {
        if let workflows::definition::StepAction::SystemCommand(action) = &mut step.action
            && action.command == workflows::commands::SystemCommandId::ApplyChanges
        {
            action.command = workflows::commands::SystemCommandId::CommitCandidate;
        }
    }
    let definition = workflows::definition::WorkflowDefinition::from_parts(
        source.name().to_owned(),
        environment,
        source.roles().to_vec(),
        steps,
    )
    .unwrap();
    let workflow = state.workflows.create(definition).unwrap();
    let selection = WorkflowSelection {
        workflow_id: workflow.id,
        definition_version: workflow.definition_version,
    }
    .as_token();
    let conversation = state
        .conversations
        .create("Approval policy".to_owned())
        .unwrap();
    let mut view = launch_view(
        &state,
        &conversation,
        Some(&selection),
        "Implement the change",
        "automatic-after-review",
        &[],
        "",
    )
    .await;
    view.stage = "review";
    let body = view.contents().render().unwrap();
    assert!(body.contains("Automatic commit after approved review"));
    assert!(!body.contains("Your approval precedes file application"));
    assert!(body.contains("Direct"));
    assert!(body.contains("writes and host commands can change files before a"));
}

#[tokio::test]
async fn launch_preview_mismatch_starts_no_work() {
    let state = connected_state();
    let conversation = state
        .conversations
        .create("Workflow".to_owned())
        .expect("conversation");
    let workflow = state
        .workflows
        .create(workflows::seeds::implement_and_review_definition(
            crate::tests::test_environment_id(),
        ))
        .expect("workflow");
    let selection = WorkflowSelection {
        workflow_id: workflow.id,
        definition_version: workflow.definition_version,
    }
    .as_token();
    let token = crate::sessions::generate_session_token().expect("session");
    state.sessions.insert(token.id());
    let app = crate::slices::router()
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::sessions::resolve_session,
        ))
        .layer(axum::middleware::from_fn(hypergraft::middleware::classify))
        .with_state(state.clone());
    // Previews across every stage start no work.
    for stage in ["choose", "inputs", "review"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/conversations/{}/workflow?stage={stage}&workflow={selection}&brief=Implement&revision=1&preview_workflow=stale&preview_commit_policy=&confirm_additional_access=forged",
                        conversation.id.as_hex()
                    ))
                    .header(
                        header::COOKIE,
                        format!("powerplant_session={}", token.raw().as_str()),
                    )
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
    }
    assert!(
        state
            .workflow_runs
            .for_conversation(&conversation.id)
            .is_empty()
    );
    // A launch whose preview no longer matches starts nothing.
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/conversations/{}/workflow",
                    conversation.id.as_hex()
                ))
                .header(
                    header::COOKIE,
                    format!("powerplant_session={}", token.raw().as_str()),
                )
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from(format!(
                    "revision={}&workflow={selection}&preview_workflow=stale&brief=Implement",
                    conversation.revision
                )))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("body")
            .to_vec(),
    )
    .expect("text");
    assert!(body.contains("The selection changed."));
    assert!(
        state
            .workflow_runs
            .for_conversation(&conversation.id)
            .is_empty()
    );
}

#[test]
fn commit_workflow_settings_do_not_select_a_global_repository() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("work");
    std::fs::create_dir(&path).unwrap();
    let mut grant = crate::execution::DirectoryGrant::from_selected(&path, &[]).unwrap();
    grant.access = crate::execution::DirectoryAccess::ReviewBeforeApply;
    let settings = directory_settings()
        .with_directories(vec![grant.clone()])
        .unwrap();
    let definition = commit_capable_definition();
    let phases: Vec<_> = phase_steps(&definition)
        .into_iter()
        .map(|step| (step.key.clone(), settings.clone()))
        .collect();
    let pinned = definition.with_phase_settings(&settings, &phases).unwrap();
    assert!(pinned.steps().iter().any(|step| {
        matches!(
            &step.action,
            workflows::definition::StepAction::SystemCommand(action)
                if action.command == workflows::commands::SystemCommandId::CommitCandidate
        )
    }));
}

fn commit_capable_definition() -> workflows::definition::WorkflowDefinition {
    let source =
        workflows::seeds::implement_with_approval_definition(crate::tests::test_environment_id());
    let mut steps = source.steps().to_vec();
    for step in &mut steps {
        if let workflows::definition::StepAction::SystemCommand(action) = &mut step.action
            && action.command == workflows::commands::SystemCommandId::ApplyChanges
        {
            action.command = workflows::commands::SystemCommandId::CommitCandidate;
        }
    }
    workflows::definition::WorkflowDefinition::from_parts(
        source.name().to_owned(),
        crate::tests::test_environment_id(),
        source.roles().to_vec(),
        steps,
    )
    .unwrap()
}

#[tokio::test]
async fn launch_refuses_a_commit_workflow_with_only_read_access() {
    let state = connected_state();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("work");
    std::fs::create_dir(&path).unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(&path, &[]).unwrap();
    let settings = directory_settings().with_directories(vec![grant]).unwrap();
    let conversation = state
        .conversations
        .create("Commit launch".to_owned())
        .expect("conversation");
    let conversation = state
        .conversations
        .update_execution_settings(&conversation.id, conversation.revision, settings)
        .expect("settings");
    let workflow = state
        .workflows
        .create(commit_capable_definition())
        .expect("workflow");
    let selection = WorkflowSelection {
        workflow_id: workflow.id,
        definition_version: workflow.definition_version,
    }
    .as_token();
    let token = crate::sessions::generate_session_token().expect("session");
    state.sessions.insert(token.id());
    let app = crate::slices::router()
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::sessions::resolve_session,
        ))
        .layer(axum::middleware::from_fn(hypergraft::middleware::classify))
        .with_state(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/conversations/{}/workflow",
                    conversation.id.as_hex()
                ))
                .header(
                    header::COOKIE,
                    format!("powerplant_session={}", token.raw().as_str()),
                )
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .body(Body::from(format!(
                    "revision={}&workflow={selection}&preview_workflow={selection}&brief=Implement",
                    conversation.revision
                )))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("body")
            .to_vec(),
    )
    .expect("text");
    assert!(body.contains("The workflow needs tools or directory access outside these settings."));
    assert!(
        state
            .workflow_runs
            .for_conversation(&conversation.id)
            .is_empty()
    );
}
