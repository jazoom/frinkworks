use axum::http::StatusCode;
use tower::ServiceExt;

use crate::{
    agents::{NetworkAccess, ToolId},
    providers::{ModelSelection, ProviderKind},
};

use super::super::tests::{app, command, connected, document, test_state, text};

#[tokio::test]
async fn future_defaults_require_an_explicit_revision_bound_command_without_access_approval() {
    let state = test_state();
    let token = connected(&state);
    let record = state
        .conversations
        .create("Defaults source".to_owned())
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut grant = crate::execution::DirectoryGrant::from_selected(directory.path(), &[]).unwrap();
    grant.access = crate::execution::DirectoryAccess::DirectWrite;
    let mut settings = crate::execution::ExecutionSettings::new(
        ModelSelection::new(
            ProviderKind::Xai,
            "grok-4.6".to_owned(),
            state
                .models_dev
                .effective_effort(ProviderKind::Xai, "grok-4.6", None),
        )
        .unwrap(),
        "Use concise explanations.".to_owned(),
        vec![ToolId::Read],
        super::super::default_environment(&state).unwrap(),
    )
    .unwrap();
    settings.directories.push(grant.clone());
    let record = state
        .conversations
        .update_execution_settings(&record.id, record.revision, settings.clone())
        .unwrap();
    let record = state
        .conversations
        .record_directory_approval(
            &record.id,
            record.revision,
            crate::conversations::DirectoryApproval::for_grant(&settings, &grant),
        )
        .unwrap();
    let path = format!("/conversations/{}/settings/defaults", record.id);
    let fields = format!("revision={}", record.revision);
    let mut native = command(&path, &token, &fields);
    native.headers_mut().remove("graft-request");
    assert_eq!(
        app(&state).oneshot(native).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app(&state)
            .oneshot(command(&path, &token, "revision=0"))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert!(state.preferences.conversation_defaults().is_none());
    assert_eq!(
        app(&state)
            .oneshot(command(&path, &token, &fields))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        state.preferences.conversation_defaults(),
        Some(settings.clone())
    );
    assert_eq!(state.conversations.get(&record.id), Some(record.clone()));
    let body = text(
        app(&state)
            .oneshot(document("/conversations/new", &token))
            .await
            .unwrap(),
    )
    .await;
    assert!(body.contains("Use concise explanations."));
    assert!(body.contains("Pending approval"));
    assert_eq!(state.conversations.list().len(), 1);
    let mut replacement = settings.clone();
    replacement.instructions = "Local exception.".to_owned();
    state
        .conversations
        .update_execution_settings(&record.id, record.revision, replacement)
        .unwrap();
    assert_eq!(state.preferences.conversation_defaults(), Some(settings));
}

#[tokio::test]
async fn settings_reject_retired_replacements_but_keep_the_current_retired_model() {
    let state = test_state();
    let token = connected(&state);
    super::super::tests::ready_starter_environment(&state).await;
    let provider = ProviderKind::Deepseek;
    state
        .vault
        .put(crate::providers::ProviderConnection::with_key(
            provider,
            "test-key",
            "deepseek-flash",
        ))
        .unwrap();
    for current in ["deepseek-flash", "deepseek-v4-flash"] {
        let record = state.conversations.create("Saved".to_owned()).unwrap();
        let record = state
            .conversations
            .select_model(
                &record.id,
                record.revision,
                ModelSelection::new(
                    provider,
                    current.to_owned(),
                    crate::providers::ThinkingEffort::new("high".to_owned()),
                )
                .unwrap(),
                super::super::default_environment(&state).unwrap(),
            )
            .unwrap();
        let response = app(&state).oneshot(command(
            &format!("/conversations/{}/settings", record.id),
            &token,
            &format!("revision={}&provider=deepseek&model=deepseek-v4-flash&thinking=low&instructions=Updated&tool_read=read", record.revision),
        )).await.unwrap();
        let status = response.status();
        let body = text(response).await;
        let saved = state.conversations.get(&record.id).unwrap();
        assert_eq!(saved.model.as_ref().unwrap().settings.model.model, current);
        if current == "deepseek-flash" {
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
            assert!(body.contains("This model is deprecated."));
            assert_eq!(saved.revision, record.revision);
        } else {
            assert_eq!(status, StatusCode::OK, "{body}");
            let settings = saved.model.unwrap().settings;
            assert_eq!(settings.instructions, "Updated");
            assert_eq!(settings.model.thinking.unwrap().as_str(), "low");
        }
    }
}

#[tokio::test]
async fn settings_update_validates_the_complete_form_and_revision() {
    let state = test_state();
    let token = connected(&state);
    super::super::tests::ready_starter_environment(&state).await;
    let record = state.conversations.create("Saved".to_owned()).unwrap();
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    let (environment, _) = state
        .environments
        .create(crate::environments::EnvironmentDraft {
            name: "Rust tools".to_owned(),
            oci_image: "docker.io/library/alpine:3.20".to_owned(),
            setup_script: String::new(),
        })
        .unwrap();
    let path = format!("/conversations/{}/settings", record.id);
    let valid = format!(
        "revision={}&provider=xai&model=grok-4.6&thinking={}&instructions={}&tool_read=read&tool_list=list&environment={}&network=restricted&network_domains=example.com",
        record.revision,
        effort.as_str(),
        "Answer%20with%20concise%20evidence.",
        environment.id
    );
    let response = app(&state)
        .oneshot(command(&path, &token, &valid))
        .await
        .unwrap();
    let status = response.status();
    let body = text(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("data-settings-open=\"true\""));
    let updated = state.conversations.get(&record.id).unwrap();
    assert!(state.preferences.conversation_defaults().is_none());
    let settings = &updated.model.as_ref().unwrap().settings;
    assert_eq!(settings.instructions, "Answer with concise evidence.");
    assert_eq!(settings.environment, environment.id);
    assert_eq!(
        settings.tools,
        vec![crate::agents::ToolId::List, crate::agents::ToolId::Read]
    );
    assert_eq!(
        settings.network,
        crate::agents::NetworkAccess::Restricted(vec!["example.com".to_owned()])
    );
    state
        .environments
        .delete(&environment.id, environment.revision)
        .unwrap();

    for fields in [
        format!(
            "revision={}&provider=xai&model=grok-4.6&thinking={}&host_approval=unknown",
            updated.revision,
            effort.as_str()
        ),
        format!(
            "revision={}&provider=xai&model=grok-4.6&thinking={}&tool_read=unknown",
            updated.revision,
            effort.as_str()
        ),
        format!(
            "revision={}&provider=xai&model=grok-4.6&thinking=invalid",
            updated.revision
        ),
        format!(
            "revision={}&provider=xai&model=grok-4.6&thinking={}&network=restricted&network_domains=https%3A%2F%2Fexample.com",
            updated.revision,
            effort.as_str()
        ),
        format!(
            "revision={}&provider=xai&model=grok-4.6&thinking={}&environment={}",
            updated.revision,
            effort.as_str(),
            environment.id
        ),
    ] {
        let response = app(&state)
            .oneshot(command(&path, &token, &fields))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = text(response).await;
        assert!(body.contains("data-settings-open=\"true\""));
        if fields.contains("thinking=invalid") {
            assert!(body.contains("Unavailable · invalid"));
        }
        assert_eq!(state.conversations.get(&record.id).unwrap(), updated);
    }

    let stale = format!(
        "revision={}&provider=xai&model=grok-4.6&thinking={}&instructions=Retained&tool_read=read",
        record.revision,
        effort.as_str()
    );
    let response = app(&state)
        .oneshot(command(&path, &token, &stale))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = text(response).await;
    assert!(body.contains("Retained"));
    assert!(body.contains("data-settings-open=\"true\""));

    let owner = super::super::tests::session_id(&token);
    let job = state
        .sessions
        .begin_conversation_job(&owner, updated.id)
        .unwrap();
    state
        .conversations
        .begin_message(
            &updated.id,
            updated.revision,
            settings.model.clone(),
            job.id(),
            "Question".to_owned(),
        )
        .unwrap();
    let active = state.conversations.get(&updated.id).unwrap();
    let response = app(&state)
        .oneshot(command(
            &path,
            &token,
            &format!(
                "revision={}&provider=xai&model=grok-4.6&thinking={}&environment={}",
                active.revision,
                effort.as_str(),
                super::super::default_environment(&state).unwrap()
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("Stop task and switch"));
    assert_eq!(state.conversations.get(&active.id).unwrap(), active);

    let preview_path = format!("/conversations/{}/settings/environment", active.id);
    let response = app(&state)
        .oneshot(command(
            &preview_path,
            &token,
            &format!(
                "revision={}&environment={}",
                record.revision,
                super::super::default_environment(&state).unwrap()
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(text(response).await.contains("Reload the conversation"));

    let (unready, _) = state
        .environments
        .create(crate::environments::EnvironmentDraft {
            name: "Not prepared".to_owned(),
            oci_image: "docker.io/library/alpine:3.20".to_owned(),
            setup_script: String::new(),
        })
        .unwrap();
    let response = app(&state)
        .oneshot(command(
            &preview_path,
            &token,
            &format!("revision={}&environment={}", active.revision, unready.id),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(text(response).await.contains("not ready"));

    let stop_path = format!(
        "/conversations/{}/settings/environment/stop-and-switch",
        active.id
    );
    let other_job = crate::sessions::JobId::generate().unwrap();
    let response = app(&state)
        .oneshot(command(
            &stop_path,
            &token,
            &format!(
                "revision={}&environment={}&job={}",
                active.revision,
                super::super::default_environment(&state).unwrap(),
                other_job
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(!job.cancel_requested());

    let response = app(&state)
        .oneshot(command(
            &stop_path,
            &token,
            &format!(
                "revision={}&environment={}&job={}",
                active.revision,
                super::super::default_environment(&state).unwrap(),
                job.id()
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(text(response).await.contains("still stopping the task"));
    assert_eq!(state.conversations.get(&active.id).unwrap(), active);

    job.finish(crate::sessions::JobStatus::Failed, Some("Cleanup failed"));
    let response = app(&state)
        .oneshot(command(
            &stop_path,
            &token,
            &format!(
                "revision={}&environment={}&job={}",
                active.revision,
                super::super::default_environment(&state).unwrap(),
                job.id()
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(text(response).await.contains("could not clean up the task"));
    assert_eq!(state.conversations.get(&active.id).unwrap(), active);
    assert!(state.sessions.conversation_reserved(active.id));
}

#[tokio::test]
async fn draft_preset_replacement_retains_its_reference_without_creating_a_conversation() {
    let state = test_state();
    let token = connected(&state);
    let owner = super::super::tests::session_id(&token);
    let settings = crate::execution::ExecutionSettings::new(
        ModelSelection::new(ProviderKind::Deepseek, "deepseek-chat".to_owned(), None).unwrap(),
        "Keep these instructions.".to_owned(),
        Vec::new(),
        crate::environments::EnvironmentId::generate().unwrap(),
    )
    .unwrap();
    let preset = state
        .presets
        .create(
            "Unavailable resources",
            settings,
            crate::presets::PresetProvenance::Draft,
        )
        .unwrap();
    let form = super::super::new::NewForm {
        preset: preset.id.as_hex(),
        draft_nonce: "test-draft".to_owned(),
        ..Default::default()
    };
    let preview = state
        .presets
        .preview(
            owner,
            preset.id,
            crate::presets::PresetDestination::Draft(form.consent_nonce()),
        )
        .unwrap();
    let response = app(&state)
        .oneshot(command(
            "/conversations/new/settings/presets/apply",
            &token,
            &format!(
                "preset={}&draft_nonce=test-draft&preset_preview={}",
                preset.id, preview.token
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains(&preview.token));
    assert!(
        body.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains("Unavailable · DeepSeek")
    );
    assert!(body.contains("Keep these instructions."));
    assert!(state.conversations.list().is_empty());
    assert!(state.presets.applied_draft(owner, &preview.token).is_some());
}

#[tokio::test]
async fn preset_application_uses_the_preview_snapshot_and_requires_fresh_access_approval() {
    let state = test_state();
    let token = connected(&state);
    let record = state.conversations.create("Saved".to_owned()).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut grant = crate::execution::DirectoryGrant::from_selected(directory.path(), &[]).unwrap();
    grant.access = crate::execution::DirectoryAccess::DirectWrite;
    let settings = crate::execution::ExecutionSettings::new(
        ModelSelection::new(
            ProviderKind::Xai,
            "grok-4.6".to_owned(),
            state
                .models_dev
                .effective_effort(ProviderKind::Xai, "grok-4.6", None),
        )
        .unwrap(),
        "Pinned instructions".to_owned(),
        vec![ToolId::Read],
        crate::environments::EnvironmentId::generate().unwrap(),
    )
    .unwrap()
    .with_network(NetworkAccess::Public)
    .unwrap()
    .with_directories(vec![grant])
    .unwrap();
    let preset = state
        .presets
        .create(
            "Pinned",
            settings.clone(),
            crate::presets::PresetProvenance::Draft,
        )
        .unwrap();
    let owner = super::super::tests::session_id(&token);
    let grant = &settings.directories[0];
    let request = state
        .access_consent
        .request_conversation(owner, record.id, &settings, grant)
        .unwrap();
    state
        .access_consent
        .approve_conversation(&request, owner, record.id, &settings, grant)
        .unwrap();
    assert!(
        state
            .access_consent
            .authorised_conversation(owner, record.id, &settings, grant)
    );
    let preview = state
        .presets
        .preview(
            owner,
            preset.id,
            crate::presets::PresetDestination::Conversation(record.id, record.revision),
        )
        .unwrap();
    let path = format!("/conversations/{}/settings/presets/apply", record.id);

    let response = app(&state)
        .oneshot(command(
            &path,
            &token,
            &format!("revision={}&preset_preview=tampered", record.revision),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(state.conversations.get(&record.id).unwrap().model.is_none());

    let response = app(&state)
        .oneshot(command(
            &path,
            &token,
            &format!(
                "revision={}&preset_preview={}",
                record.revision, preview.token
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("Selected environment unavailable"));
    assert!(body.contains("Pending approval"));
    let applied = state.conversations.get(&record.id).unwrap();
    assert_eq!(applied.model.as_ref().unwrap().settings, settings);
    assert!(
        !state
            .access_consent
            .authorised_conversation(owner, record.id, &settings, grant)
    );
    assert_eq!(
        applied
            .model
            .as_ref()
            .unwrap()
            .preset
            .as_ref()
            .unwrap()
            .name,
        "Pinned"
    );
}

#[tokio::test]
async fn host_approval_policy_needs_fresh_consent_and_does_not_settle_pending_commands() {
    let state = test_state();
    let token = connected(&state);
    let session = super::super::tests::session_id(&token);
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    let settings = crate::execution::ExecutionSettings::new(
        ModelSelection::new(
            ProviderKind::Xai,
            "grok-4.6".to_owned(),
            Some(effort.clone()),
        )
        .unwrap(),
        String::new(),
        vec![ToolId::Run],
        super::super::default_environment(&state).unwrap(),
    )
    .unwrap()
    .with_location(crate::execution::ToolLocation::Host);
    let record = state
        .conversations
        .create("Host policy".to_owned())
        .unwrap();
    let record = state
        .conversations
        .update_execution_settings(&record.id, record.revision, settings.clone())
        .unwrap();
    state
        .access_consent
        .approve_host_conversation(
            &state
                .access_consent
                .request_host_conversation(session, record.id, &settings)
                .unwrap(),
            session,
            record.id,
            &settings,
        )
        .unwrap();
    assert!(
        state
            .access_consent
            .authorised_host_conversation(session, record.id, &settings)
    );

    let path = format!("/conversations/{}/settings", record.id);
    let automatic = format!(
        "revision={}&provider=xai&model=grok-4.6&thinking={}&location=host&tool_run=run&host_approval=automatic&environment={}",
        record.revision,
        effort.as_str(),
        settings.environment
    );
    let response = app(&state)
        .oneshot(command(&path, &token, &automatic))
        .await
        .unwrap();
    let status = response.status();
    let body = text(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        state.conversations.get(&record.id).unwrap().model,
        record.model
    );
    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/settings/environment", record.id),
            &token,
            &format!(
                "revision={}&environment={}&location=host&host_approval=automatic&confirm=true",
                record.revision, settings.environment
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("Pending approval"));
    assert!(body.contains("Run without approval"));
    let updated = state.conversations.get(&record.id).unwrap();
    let stored = updated.model.as_ref().unwrap().settings.clone();
    assert_eq!(
        stored.host_approval,
        crate::execution::HostApprovalPolicy::Automatic
    );
    assert!(
        !state
            .access_consent
            .authorised_host_conversation(session, record.id, &stored)
    );
    assert!(
        !state
            .access_consent
            .authorised_host_conversation(session, record.id, &settings)
    );

    let request = state
        .access_consent
        .request_host_conversation(session, updated.id, &stored)
        .unwrap();
    state
        .access_consent
        .approve_host_conversation(&request, session, updated.id, &stored)
        .unwrap();
    let job = state
        .sessions
        .begin_conversation_job(&session, updated.id)
        .unwrap();
    let current = state.conversations.get(&updated.id).unwrap();
    state
        .conversations
        .begin_message_with_model(
            &current.id,
            current.revision,
            current.model.clone(),
            job.id(),
            "Question".to_owned(),
        )
        .unwrap();
    let token_request = state
        .host_approvals
        .submit(crate::execution::HostCommandRequest {
            token: String::new(),
            session,
            job: job.id(),
            conversation: current.id,
            execution_revision: current.revision,
            command: "printf hi".to_owned(),
            directory: std::env::current_dir().unwrap(),
            explanation: "Print a greeting".to_owned(),
            run: None,
            step: None,
            attempt: None,
        })
        .unwrap();
    job.set_awaiting_decision();
    let active = state.conversations.get(&current.id).unwrap();
    let response = app(&state)
        .oneshot(command(
            &path,
            &token,
            &format!(
                "revision={}&provider=xai&model=grok-4.6&thinking={}&location=host&tool_run=run&host_approval=ask-each-time&environment={}",
                active.revision,
                effort.as_str(),
                stored.environment
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("Stop task and switch"));
    assert!(body.contains("Unrestricted host access · Run without approval"));
    assert!(!body.contains("Unrestricted host access · Ask each time"));
    assert!(body.contains("Host command approval changes from"));
    assert_eq!(
        state
            .conversations
            .get(&active.id)
            .unwrap()
            .model
            .as_ref()
            .unwrap()
            .settings
            .host_approval,
        crate::execution::HostApprovalPolicy::Automatic
    );
    assert_eq!(
        state
            .host_approvals
            .pending_for(active.id, job.id())
            .unwrap()
            .token,
        token_request
    );

    let stale_stop = format!(
        "revision={}&location=host&host_approval=ask-each-time&environment={}&job={}",
        record.revision,
        stored.environment,
        job.id()
    );
    let stop_path = format!(
        "/conversations/{}/settings/environment/stop-and-switch",
        active.id
    );
    let response = app(&state)
        .oneshot(command(&stop_path, &token, &stale_stop))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        state
            .host_approvals
            .pending_for(active.id, job.id())
            .unwrap()
            .token,
        token_request
    );
}

#[tokio::test]
async fn strategy_switch_binds_existing_roots_and_waits_for_cancelled_commands() {
    let mut state = test_state();
    let home = tempfile::tempdir().unwrap();
    let data = home.path().join("power-plant-data");
    std::fs::create_dir(&data).unwrap();
    state.local_data = crate::local_data::LocalDataReset::for_test(data);
    let token = connected(&state);
    let session = super::super::tests::session_id(&token);
    super::super::tests::ready_starter_environment(&state).await;
    let grant =
        crate::execution::DirectoryGrant::from_selected(state.local_data.root(), &[]).unwrap();
    let settings = crate::execution::ExecutionSettings::new(
        ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None).unwrap(),
        String::new(),
        vec![ToolId::Run],
        super::super::default_environment(&state).unwrap(),
    )
    .unwrap()
    .with_directories(vec![grant.clone()])
    .unwrap()
    .with_location(crate::execution::ToolLocation::Host);
    let record = state
        .conversations
        .create("Strategy switch".to_owned())
        .unwrap();
    let record = state
        .conversations
        .update_execution_settings(&record.id, record.revision, settings.clone())
        .unwrap();
    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .unwrap();
    state
        .conversations
        .begin_message_with_model(
            &record.id,
            record.revision,
            record.model.clone(),
            job.id(),
            "Question".to_owned(),
        )
        .unwrap();
    let active = state.conversations.get(&record.id).unwrap();
    let request = crate::execution::HostCommandRequest {
        token: String::new(),
        session,
        conversation: record.id,
        job: job.id(),
        execution_revision: active.revision,
        command: "printf harmless".to_owned(),
        directory: grant.host_path.clone(),
        explanation: "Show text".to_owned(),
        run: None,
        step: None,
        attempt: None,
    };
    let pending = state.host_approvals.submit(request).unwrap();
    let path = format!("/conversations/{}/settings/environment", record.id);
    let access = serde_json::to_string(&vec![(grant.id.as_hex(), "direct-write")]).unwrap();
    let fields = format!(
        "revision={}&environment={}&location=sandbox&directory_access={}",
        active.revision,
        settings.environment,
        url::form_urlencoded::byte_serialize(access.as_bytes()).collect::<String>()
    );
    let response = app(&state)
        .oneshot(command(&path, &token, &fields))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.conversations.get(&active.id).unwrap(), active);
    assert!(
        state
            .host_approvals
            .pending_for(record.id, job.id())
            .is_some()
    );

    for invalid in [
        format!(
            "[[\"{}\",\"direct-write\"],[\"{}\",\"read-only\"]]",
            grant.id.as_hex(),
            grant.id.as_hex()
        ),
        "[[\"unknown-root\",\"direct-write\"]]".to_owned(),
        format!("[[\"{}\",\"unknown-strategy\"]]", grant.id.as_hex()),
    ] {
        let response = app(&state)
            .oneshot(command(
                &path,
                &token,
                &format!(
                    "revision={}&environment={}&directory_access={}",
                    active.revision,
                    settings.environment,
                    url::form_urlencoded::byte_serialize(invalid.as_bytes()).collect::<String>(),
                ),
            ))
            .await
            .unwrap();
        assert!(!response.status().is_success());
        assert!(!job.cancel_requested());
    }

    let settling_state = state.clone();
    let settling_job = job.clone();
    let settlement = tokio::spawn(async move {
        settling_job.cancelled().await;
        settling_state
            .conversations
            .settle_message(
                &record.id,
                settling_job.id(),
                String::new(),
                crate::conversations::MessageStatus::Interrupted,
                None,
            )
            .unwrap();
        settling_job.finish(crate::sessions::JobStatus::Cancelled, None);
        settling_state
            .sessions
            .finish_conversation_job(&session, record.id, settling_job.id());
    });
    let stop_path = format!("{path}/stop-and-switch");
    let stop = format!("{fields}&job={}", job.id());
    let response = app(&state)
        .oneshot(command(&stop_path, &token, &stop))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    settlement.await.unwrap();
    assert!(state.host_approvals.wait(&pending, &job).await.is_err());
    let updated = state.conversations.get(&active.id).unwrap();
    let settings = &updated.model.as_ref().unwrap().settings;
    assert_eq!(settings.location, crate::execution::ToolLocation::Sandbox);
    assert_eq!(
        settings.directories[0].access,
        crate::execution::DirectoryAccess::DirectWrite
    );
    assert!(text(response).await.contains("Pending approval"));
    let response = app(&state)
        .oneshot(command(&stop_path, &token, &stop))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(state.conversations.get(&active.id).unwrap(), updated);
}

#[tokio::test]
async fn explicit_empty_tool_selection_persists_without_default_substitution() {
    let state = test_state();
    let token = connected(&state);
    let record = state.conversations.create("Saved".to_owned()).unwrap();
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    let path = format!("/conversations/{}/settings", record.id);
    let fields = format!(
        "revision={}&provider=xai&model=grok-4.6&thinking={}&instructions=Concise+replies",
        record.revision,
        effort.as_str()
    );
    let response = app(&state)
        .oneshot(command(&path, &token, &fields))
        .await
        .unwrap();
    let status = response.status();
    let body = text(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let updated = state.conversations.get(&record.id).unwrap();
    assert!(updated.model.as_ref().unwrap().settings.tools.is_empty());
    let invalid = format!(
        "revision={}&provider=xai&model=grok-4.6&thinking=invalid&instructions=Concise+replies",
        updated.revision
    );
    let response = app(&state)
        .oneshot(command(&path, &token, &invalid))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = text(response).await;
    assert!(body.contains("data-settings-open=\"true\""));
    // The submitted instructions survive validation so the user keeps their edits.
    assert!(body.contains("Concise replies"));
    assert!(
        state
            .conversations
            .get(&record.id)
            .unwrap()
            .model
            .as_ref()
            .unwrap()
            .settings
            .tools
            .is_empty()
    );
}

#[tokio::test]
async fn partial_tool_selection_survives_an_unrelated_setup_save() {
    // The How the agent works section offers five individually selectable
    // tool checkboxes plus a Select all tools shortcut. The submitted fields
    // must carry the effective selection so an unrelated save neither broadens
    // nor narrows it.
    let state = test_state();
    let token = connected(&state);
    super::super::tests::ready_starter_environment(&state).await;
    let record = state.conversations.create("Saved".to_owned()).unwrap();
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    let (environment, _) = state
        .environments
        .create(crate::environments::EnvironmentDraft {
            name: "Rust tools".to_owned(),
            oci_image: "docker.io/library/alpine:3.20".to_owned(),
            setup_script: String::new(),
        })
        .unwrap();
    let path = format!("/conversations/{}/settings", record.id);
    let partial = format!(
        "revision={}&provider=xai&model=grok-4.6&thinking={}&instructions=Partial&tool_read=read&environment={}",
        record.revision,
        effort.as_str(),
        environment.id
    );
    let response = app(&state)
        .oneshot(command(&path, &token, &partial))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        text(response).await
    );
    let saved = state.conversations.get(&record.id).unwrap();
    assert_eq!(
        saved.model.as_ref().unwrap().settings.tools,
        vec![ToolId::Read]
    );

    let response = app(&state)
        .oneshot(document(&format!("/conversations/{}", record.id), &token))
        .await
        .unwrap();
    let body = text(response).await;
    assert!(body.contains("data-enable-tools"));
    assert_eq!(body.matches("data-tool-field").count(), 5);
    for name in [
        "tool_list",
        "tool_read",
        "tool_edit",
        "tool_write",
        "tool_run",
    ] {
        let start = body
            .find(&format!("name=\"{name}\""))
            .expect("tool field renders");
        let end = body[start..]
            .find("data-tool-field")
            .expect("tool field marker");
        assert_eq!(
            body[start..start + end].contains("checked"),
            name == "tool_read",
            "{name} keeps its effective selection"
        );
    }

    // An unrelated instructions edit resubmits the rendered partial
    // selection, exactly as the browser form does.
    let unrelated = format!(
        "revision={}&provider=xai&model=grok-4.6&thinking={}&instructions=Unrelated+edit&tool_read=read&environment={}",
        saved.revision,
        effort.as_str(),
        environment.id
    );
    let response = app(&state)
        .oneshot(command(&path, &token, &unrelated))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        text(response).await
    );
    let updated = state.conversations.get(&record.id).unwrap();
    let settings = &updated.model.as_ref().unwrap().settings;
    assert_eq!(settings.instructions, "Unrelated edit");
    assert_eq!(settings.tools, vec![ToolId::Read]);
    state
        .environments
        .delete(&environment.id, environment.revision)
        .unwrap();
}

#[tokio::test]
async fn preset_application_clears_host_approval() {
    // A preset replaces the whole setup, so applying it must invalidate
    // host approval exactly like a manual Files and execution change.
    let state = test_state();
    let token = connected(&state);
    super::super::tests::ready_starter_environment(&state).await;
    let session = super::super::tests::session_id(&token);
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    let settings = crate::execution::ExecutionSettings::new(
        ModelSelection::new(
            ProviderKind::Xai,
            "grok-4.6".to_owned(),
            Some(effort.clone()),
        )
        .unwrap(),
        String::new(),
        vec![ToolId::Run],
        super::super::default_environment(&state).unwrap(),
    )
    .unwrap()
    .with_location(crate::execution::ToolLocation::Host);
    let record = state
        .conversations
        .create("Host approval".to_owned())
        .unwrap();
    let record = state
        .conversations
        .update_execution_settings(&record.id, record.revision, settings.clone())
        .unwrap();
    let request = state
        .access_consent
        .request_host_conversation(session, record.id, &settings)
        .unwrap();
    state
        .access_consent
        .approve_host_conversation(&request, session, record.id, &settings)
        .unwrap();
    assert!(
        state
            .access_consent
            .authorised_host_conversation(session, record.id, &settings)
    );
    let preset = state
        .presets
        .create(
            "Replacement",
            crate::execution::ExecutionSettings::new(
                ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), Some(effort))
                    .unwrap(),
                String::new(),
                Vec::new(),
                super::super::default_environment(&state).unwrap(),
            )
            .unwrap(),
            crate::presets::PresetProvenance::Draft,
        )
        .unwrap();
    let preview = state
        .presets
        .preview(
            session,
            preset.id,
            crate::presets::PresetDestination::Conversation(record.id, record.revision),
        )
        .unwrap();
    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/settings/presets/apply", record.id),
            &token,
            &format!(
                "revision={}&preset_preview={}",
                record.revision, preview.token
            ),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        text(response).await
    );
    let applied = state.conversations.get(&record.id).unwrap();
    let applied_settings = applied.model.as_ref().unwrap().settings.clone();
    assert!(!state.access_consent.authorised_host_conversation(
        session,
        applied.id,
        &applied_settings
    ));
    assert!(
        !state
            .access_consent
            .authorised_host_conversation(session, applied.id, &settings)
    );
}
