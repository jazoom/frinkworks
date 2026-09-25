use super::super::tests::*;
use crate::conversations::ConversationStore;
use crate::providers::{ProviderConnection, ProviderKind};
use axum::http::StatusCode;
use tower::ServiceExt;

#[tokio::test]
async fn retired_draft_defaults_change_only_the_unsaved_model() {
    let state = test_state();
    let token = connected(&state);
    let kind = ProviderKind::Deepseek;
    state
        .vault
        .put(ProviderConnection::with_key(
            kind,
            "test-key",
            "deepseek-v4-flash",
        ))
        .unwrap();
    state
        .preferences
        .select_settings(kind, "deepseek-v4-flash".to_owned(), None)
        .unwrap();
    let response = app(&state)
        .oneshot(document("/conversations/new", &token))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("This draft uses deepseek-flash from the same provider."));
    assert!(body.contains("value=\"deepseek-flash\""));
    assert!(state.conversations.list().is_empty());
    assert_eq!(
        state
            .preferences
            .desk_providers(&state.vault)
            .into_iter()
            .find(|provider| provider.kind == kind)
            .unwrap()
            .model,
        "deepseek-v4-flash"
    );

    let response = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            "action=send&provider=deepseek&model=deepseek-v4-flash&thinking=high&message=Hello",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(text(response).await.contains("This model is deprecated."));
    assert!(state.conversations.list().is_empty());
}

#[tokio::test]
async fn an_empty_provider_blocks_new_model_requests_without_switching_providers() {
    let mut state = test_state();
    let token = connected(&state);
    let provider = ProviderKind::Deepseek;
    state
        .vault
        .put(ProviderConnection::with_key(
            provider,
            "test-key",
            "deepseek-flash",
        ))
        .unwrap();
    state
        .preferences
        .select_settings(provider, "deepseek-flash".to_owned(), None)
        .unwrap();
    let mut snapshot: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../../catalogue/models-dev-v1.json")).unwrap();
    snapshot["providers"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["id"] == "deepseek")
        .unwrap()["models"] = serde_json::json!([]);
    snapshot["checked_at_unix_seconds"] = serde_json::json!(u64::MAX);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("models.json");
    crate::storage::write_private(&path, &serde_json::to_vec(&snapshot).unwrap()).unwrap();
    state.models_dev =
        std::sync::Arc::new(crate::models::models_dev::ModelsDevCatalogue::open(path).unwrap());
    let response = app(&state)
        .oneshot(document("/conversations/new", &token))
        .await
        .unwrap();
    let body = text(response).await;
    assert!(body.contains("This provider has no models for new conversations."));
    assert_eq!(hidden_named(&body, "provider"), "deepseek");
    assert_eq!(hidden_named(&body, "model"), "");
    assert!(!state.models_dev.models(ProviderKind::Xai).is_empty());
    let response = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            "action=send&provider=deepseek&model=deepseek-flash&thinking=high&message=Hello",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(state.conversations.list().is_empty());
}

#[tokio::test]
async fn prompt_links_prefill_only_a_qualified_global_command_without_execution() {
    let state = test_state();
    let token = connected(&state);
    state
        .prompts
        .create("review", "Review $1.".to_owned())
        .unwrap();
    for request in [
        document("/conversations/new?prompt=review", &token),
        navigation("/conversations/new?prompt=review", &token),
    ] {
        let response = app(&state).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(text(response).await.contains("/global/review "));
        assert!(state.conversations.list().is_empty());
    }
}

#[tokio::test]
async fn invalid_prompt_links_never_become_composer_commands() {
    let state = test_state();
    let token = connected(&state);
    std::fs::write(
        state.prompts.host_dir().join("broken.md"),
        "---\nUnfinished",
    )
    .unwrap();
    for name in ["../outside", "!pwd", "missing", "broken"] {
        let response = app(&state)
            .oneshot(document(
                &format!("/conversations/new?prompt={}", form_value(name)),
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = text(response).await;
        let message = body
            .split("id=\"composer-message\"")
            .nth(1)
            .unwrap()
            .split_once('>')
            .unwrap()
            .1
            .split("</textarea>")
            .next()
            .unwrap();
        assert!(message.trim().is_empty(), "{name}: {message}");
        assert!(state.conversations.list().is_empty());
    }
}

#[tokio::test]
async fn provider_free_draft_validates_before_it_consumes_consent() {
    let state = test_state();
    let token = connected(&state);
    state
        .vault
        .forget(ProviderKind::Xai)
        .expect("forget provider");
    assert!(!state.vault.has_providers());
    let fields = "action=send&provider=xai&model=grok-4.6&location=host&tool_run=run&host_approval=automatic";
    let preview = app(&state)
        .oneshot(command(
            "/conversations/new/settings/host-consent",
            &token,
            fields,
        ))
        .await
        .expect("consent preview");
    let request = host_consent_request(&text(preview).await);
    let approved = app(&state)
        .oneshot(command(
            "/conversations/new/settings/host-consent/approve",
            &token,
            &format!("{fields}&host_consent_request={request}"),
        ))
        .await
        .expect("consent approval");
    assert_eq!(approved.status(), StatusCode::OK);
    let body = text(approved).await;
    let fields = format!(
        "{fields}&consent_reference={}&draft_nonce={}",
        hidden_named(&body, "consent_reference"),
        hidden_named(&body, "draft_nonce")
    );
    let rejected = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            &format!("{fields}&message=%21"),
        ))
        .await
        .expect("empty command");
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(state.conversations.list().is_empty());
    let accepted = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            &format!("{fields}&message=%21printf%20draft-sentinel"),
        ))
        .await
        .expect("direct command");
    assert_eq!(
        accepted.status(),
        StatusCode::OK,
        "{}",
        text(accepted).await
    );
    assert_eq!(state.conversations.list().len(), 1);
}

#[tokio::test]
async fn copied_draft_is_independent_and_navigation_creates_no_record() {
    let state = test_state();
    let token = connected(&state);
    let source = state
        .conversations
        .create("Private source title".to_owned())
        .unwrap();
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None)
            .unwrap(),
        "Copied instructions".to_owned(),
        Vec::new(),
        super::super::default_environment(&state).unwrap(),
    )
    .unwrap();
    let source = state
        .conversations
        .update_execution_settings(&source.id, source.revision, settings.clone())
        .unwrap();
    let job = crate::sessions::JobId::generate().unwrap();
    let source = state
        .conversations
        .begin_message_with_model(
            &source.id,
            source.revision,
            None,
            job,
            "Unfinished source work".to_owned(),
        )
        .unwrap();
    let path = format!("/conversations/new?source={}", source.id);
    for request in [document(&path, &token), navigation(&path, &token)] {
        let response = app(&state).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = text(response).await;
        let body = body.split("id=\"chat-main\"").last().unwrap();
        assert!(body.contains("Copied instructions"));
        assert!(!body.contains("Private source title"));
        assert!(!body.contains("Unfinished source work"));
        assert!(body.contains("data-conversation-state=\"new\""));
        assert_eq!(state.conversations.list().len(), 1);
        assert_eq!(
            state.conversations.get(&source.id).unwrap().revision,
            source.revision
        );
    }
    assert_eq!(
        state.conversations.get(&source.id).unwrap().active_job,
        Some(job)
    );
    state
        .conversations
        .settle_message(
            &source.id,
            job,
            String::new(),
            crate::conversations::MessageStatus::Interrupted,
            None,
        )
        .unwrap();
    let settled = state.conversations.get(&source.id).unwrap();
    state
        .conversations
        .delete(&source.id, settled.revision)
        .unwrap();
    let response = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            &format!(
                "action=send&provider={}&model={}&environment={}&instructions={}&thinking={}&message=",
                settings.model.provider.as_str(),
                settings.model.model,
                settings.environment,
                form_value("Independent instructions"),
                state.models_dev.effective_effort(ProviderKind::Xai, "grok-4.6", None).unwrap().as_str()
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = text(response).await;
    assert!(body.contains("Independent instructions"));
    assert!(body.contains(crate::conversations::ConversationError::Message.message()));
    assert!(state.conversations.list().is_empty());
}

#[tokio::test]
async fn copied_sensitive_access_requires_new_consent() {
    let mut state = test_state();
    let home = tempfile::tempdir().unwrap();
    let data = home.path().join("data");
    std::fs::create_dir(&data).unwrap();
    state.local_data = crate::local_data::LocalDataReset::for_test(data);
    let token = connected(&state);
    let session = session_id(&token);
    let grant = crate::execution::DirectoryGrant::from_selected(home.path(), &[]).unwrap();
    let source = state
        .conversations
        .create("Sensitive source".to_owned())
        .unwrap();
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None)
            .unwrap(),
        String::new(),
        Vec::new(),
        super::super::default_environment(&state).unwrap(),
    )
    .unwrap()
    .with_directories(vec![grant.clone()])
    .unwrap();
    let source = state
        .conversations
        .update_execution_settings(&source.id, source.revision, settings.clone())
        .unwrap();
    let request = state
        .access_consent
        .request_conversation(session, source.id, &settings, &grant)
        .unwrap();
    state
        .access_consent
        .approve_conversation(&request, session, source.id, &settings, &grant)
        .unwrap();

    let response = app(&state)
        .oneshot(document(
            &format!("/conversations/new?source={}", source.id),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("Pending approval"));
    assert!(
        body.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains("name=\"consent_reference\" value=\"\"")
    );
    assert!(body.contains("name=\"directory_0\""));

    let response = app(&state).oneshot(command(
        "/conversations/new", &token,
        &format!("action=send&provider=xai&model=grok-4.6&environment={}&directory_0={}&thinking={}&message=Hello",
            settings.environment, form_value(&grant.form_value()),
            state.models_dev.effective_effort(ProviderKind::Xai, "grok-4.6", None).unwrap().as_str()),
    )).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        text(response)
            .await
            .contains("Directory access needs explicit approval.")
    );
    assert_eq!(state.conversations.list().len(), 1);
    assert!(
        state
            .access_consent
            .authorised_conversation(session, source.id, &settings, &grant)
    );
}

#[tokio::test]
async fn first_message_keeps_the_model_choice_local() {
    let state = test_state();
    state
        .vault
        .put(ProviderConnection::with_key(
            ProviderKind::Deepseek,
            "test-key",
            "deepseek-v4-flash",
        ))
        .unwrap();
    let token = connected(&state);
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Deepseek, "deepseek-v4-pro", None)
        .unwrap();
    let response = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            &format!(
                "provider=deepseek&model=deepseek-v4-pro&thinking={}&action=send&message=Hello",
                effort.as_str()
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let record = state.conversations.list().pop().unwrap();
    assert_eq!(
        record.model.unwrap().settings.model.model,
        "deepseek-v4-pro"
    );
    let selected = state.preferences.selected_provider(&state.vault).unwrap();
    assert_eq!(selected.kind, ProviderKind::Xai);
    assert_eq!(selected.model, "grok-4.6");
    assert!(state.preferences.conversation_defaults().is_none());
}

#[tokio::test]
async fn first_send_does_not_write_preferences() {
    let mut state = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.preferences = std::sync::Arc::new(crate::preferences::Preferences::open(
        dir.path().to_path_buf(),
    ));
    let token = connected(&state);
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    let fields = format!("provider=xai&model=grok-4.6&thinking={}", effort.as_str());
    let response = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            &format!("{fields}&action=send&message=Hello"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let record = state.conversations.list().pop().unwrap();
    let body = text(response).await;
    assert!(body.contains(&format!("location=\"/conversations/{}\"", record.id)));
    assert!(!body.contains("Frinkworks cannot store the model preference."));
    assert_eq!(record.messages[0].text, "Hello");
}

#[tokio::test]
async fn new_navigation_and_invalid_submissions_leave_no_record_or_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = test_state();
    state.conversations =
        std::sync::Arc::new(ConversationStore::open(dir.path().to_path_buf()).unwrap());
    let token = connected(&state);
    for request in [
        document("/conversations/new", &token),
        navigation("/conversations/new", &token),
    ] {
        let response = app(&state).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(state.conversations.list().is_empty());
    }
    for body in [
        "action=send&message=",
        "action=save",
        "action=send&project=missing",
        "action=send&title=%20",
        "action=send&provider=missing&model=bad",
        "action=send&preset=missing",
        "action=unknown",
    ] {
        let response = app(&state)
            .oneshot(command("/conversations/new", &token, body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            text(response)
                .await
                .contains("target=\"conversation-detail\"")
        );
        assert!(state.conversations.list().is_empty());
        assert!(!dir.path().join("catalogue.json").exists());
    }
    assert_eq!(
        app(&state)
            .oneshot(document("/conversations", &token))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert!(state.conversations.list().is_empty());
}

#[tokio::test]
async fn invalid_first_submissions_preserve_all_local_choices_and_unsent_text() {
    let state = test_state();
    let token = connected(&state);
    let preset = state
        .agents
        .create(crate::agents::AgentDraft {
            name: "Local preset".to_owned(),
            instructions: String::new(),
            selection: None,
            tools: Vec::new(),
            network: crate::agents::NetworkAccess::None,
            directories: Vec::new(),
            primary_directory: String::new(),
        })
        .unwrap();
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    for (action, title) in [("save", "Local title"), ("send", " ")] {
        let response = app(&state)
            .oneshot(command(
                "/conversations/new",
                &token,
                &format!(
                    "action={action}&title={title}&message=Unsent%20text&provider=xai&model=grok-4.6&thinking={}&preset={}",
                    effort.as_str(), preset.id
                ),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = text(response).await;
        assert!(body.contains("Unsent text</textarea>"));
        assert!(body.contains(&format!("value=\"{title}\"")));
        assert!(body.contains("value=\"grok-4.6\""));
        assert_eq!(hidden_named(&body, "provider"), "xai");
        let option = body
            .split(&format!("value=\"{}\"", effort.as_str()))
            .nth(1)
            .unwrap()
            .split('>')
            .next()
            .unwrap();
        assert!(option.contains("selected"), "{}", effort.as_str());
        let envelope = body.split_once('>').unwrap().0;
        assert!(!envelope.contains(" location="));
        assert!(state.conversations.list().is_empty());
    }
}

#[tokio::test]
async fn template_first_send_rejects_invalid_arguments_before_creation() {
    let (state, _directory) = state_with_prompts();
    ready_starter_environment(&state).await;
    let token = connected(&state);
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    let fields = format!(
        "action=send&provider=xai&model=grok-4.6&thinking={}",
        effort.as_str()
    );
    let response = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            &format!("{fields}&message={}", form_value("/review \"open")),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(state.conversations.list().is_empty());

    let typed = "/review \"a b\"";
    let response = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            &format!("{fields}&message={}", form_value(typed)),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let record = state.conversations.list().pop().unwrap();
    let message = &record.messages[0];
    assert_eq!(message.text, "Review a b at HEAD.");
    let provenance = message.input.as_ref().expect("frozen input");
    assert_eq!(provenance.typed, typed);
    assert_eq!(provenance.expanded, message.text);
}

#[tokio::test]
async fn invalid_instructions_preserve_an_unsaved_draft_without_tool_defaults() {
    let state = test_state();
    let token = connected(&state);
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    for instructions in ["é".repeat(16385), "Unsupported\0control".to_owned()] {
        let response = app(&state).oneshot(command("/conversations/new", &token, &format!(
            "action=send&message=Unsent+draft&provider=xai&model=grok-4.6&thinking={}&instructions={}",
            effort.as_str(), form_value(&instructions),
        ))).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = text(response).await;
        assert!(body.contains("32 KiB"));
        assert!(body.contains(&instructions));
        assert!(body.contains("Unsent draft</textarea>"));
        for tool in ["list", "read", "edit", "write", "run"] {
            let field = body.split(&format!("name=\"tool_{tool}\"")).nth(1).unwrap();
            assert!(
                !field
                    .split("data-tool-field")
                    .next()
                    .unwrap()
                    .contains("checked")
            );
        }
        assert!(state.conversations.list().is_empty());
        assert!(!state.sessions.busy(&session_id(&token)));
    }
}

#[tokio::test]
async fn first_send_persists_message_and_model_then_replaces_location() {
    let state = test_state();
    ready_starter_environment(&state).await;
    let token = connected(&state);
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    let fields = format!(
        "action=send&provider=xai&model=grok-4.6&thinking={}&instructions={}&tool_read=read&tool_list=list",
        effort.as_str(),
        "Use%20the%20supplied%20context."
    );
    for message in [
        "%20",
        "%00",
        &"x".repeat(crate::conversations::MAXIMUM_MESSAGE_BYTES + 1),
    ] {
        let response = app(&state)
            .oneshot(command(
                "/conversations/new",
                &token,
                &format!("{fields}&message={message}"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(state.conversations.list().is_empty());
        assert!(!state.sessions.busy(&session_id(&token)));
    }
    let response = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            &format!(
                "{}&message=Explain%20this",
                fields.replace("tool_read=read", "tool_read=unknown")
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(state.conversations.list().is_empty());

    let response = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            &format!("{fields}&message=Explain%20this"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    let record = state.conversations.list().pop().unwrap();
    assert!(body.contains(&format!("location=\"/conversations/{}\"", record.id)));
    assert!(body.contains("data-conversation-state=\"saved\""));
    assert!(body.contains("id=\"conversation-settings-form\""));
    assert!(body.contains("id=\"conversation-model-search\""));
    assert_eq!(record.messages[0].text, "Explain this");
    let settings = record.model.unwrap().settings;
    assert_eq!(settings.model.model, "grok-4.6");
    assert_eq!(settings.instructions, "Use the supplied context.");
    assert_eq!(
        settings.tools,
        vec![crate::agents::ToolId::List, crate::agents::ToolId::Read]
    );
}

#[tokio::test]
async fn sensitive_draft_consent_is_consumed_by_one_valid_first_message() {
    for access in [
        crate::execution::DirectoryAccess::ReadOnly,
        crate::execution::DirectoryAccess::DirectWrite,
    ] {
        sensitive_first_message_case(access).await;
    }
}

async fn sensitive_first_message_case(access: crate::execution::DirectoryAccess) {
    let mut state = test_state();
    let home = tempfile::tempdir().unwrap();
    let data = home.path().join("frinkworks-data");
    std::fs::create_dir(&data).unwrap();
    state.local_data = crate::local_data::LocalDataReset::for_test(data);
    let token = connected(&state);
    let session = session_id(&token);
    let mut grant = crate::execution::DirectoryGrant::from_selected(home.path(), &[]).unwrap();
    grant.access = access;
    let directories = vec![grant.clone()];
    let nonce = crate::execution::draft_nonce().unwrap();
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    let draft = super::NewForm {
        provider: "xai".to_owned(),
        model: "grok-4.6".to_owned(),
        thinking: effort.as_str().to_owned(),
        draft_nonce: nonce.clone(),
        ..super::NewForm::default()
    };
    let bound_nonce = draft.consent_nonce();
    let request = state
        .access_consent
        .request_draft(session, &bound_nonce, &directories, &grant)
        .unwrap();
    let reference = state
        .access_consent
        .approve_draft(&request, session, &bound_nonce, &directories, &grant)
        .unwrap();
    let body = format!(
        "action=send&provider=xai&model=grok-4.6&thinking={}&message=Hello&draft_nonce={}&consent_reference={}&directory_0={}",
        effort.as_str(),
        form_value(&nonce),
        form_value(&reference),
        form_value(&grant.form_value()),
    );

    for rejected in [
        body.replace("message=Hello", "message="),
        format!("{body}&instructions=Changed"),
        format!("{body}&network=public"),
        body.replace(&reference, "forged"),
    ] {
        let response = app(&state)
            .oneshot(command("/conversations/new", &token, &rejected))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(state.conversations.list().is_empty());
    }
    let first = app(&state)
        .oneshot(command("/conversations/new", &token, &body))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let records = state.conversations.list();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    let settings = &record.model.as_ref().unwrap().settings;
    assert!(
        state
            .conversations
            .directory_approved(&record.id, settings, &grant)
    );
    state.access_consent.invalidate_conversation(record.id);
    assert!(
        state
            .conversations
            .directory_approved(&record.id, settings, &grant)
    );

    let replay = app(&state)
        .oneshot(command("/conversations/new", &token, &body))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(state.conversations.list().len(), 1);
}

#[tokio::test]
async fn first_message_waits_for_the_host_path_permit_before_it_stores_grants() {
    let state = test_state();
    let token = connected(&state);
    let directory = tempfile::tempdir().unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(directory.path(), &[]).unwrap();
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    let permit = state.local_data.begin_host_path_mutation().await.unwrap();
    let request = command(
        "/conversations/new",
        &token,
        &format!(
            "action=send&provider=xai&model=grok-4.6&thinking={}&message=Hello&directory_0={}",
            effort.as_str(),
            form_value(&grant.form_value()),
        ),
    );
    let mut response = tokio::spawn(app(&state).oneshot(request));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), &mut response)
            .await
            .is_err()
    );
    assert!(state.conversations.list().is_empty());
    drop(permit);
    assert_eq!(response.await.unwrap().unwrap().status(), StatusCode::OK);
    assert_eq!(
        state.conversations.list()[0]
            .model
            .as_ref()
            .unwrap()
            .settings
            .directories,
        vec![grant]
    );
}

#[tokio::test]
async fn first_message_persists_an_independent_directory_grant() {
    let state = test_state();
    ready_starter_environment(&state).await;
    let token = connected(&state);
    let directory = tempfile::tempdir().unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(directory.path(), &[]).unwrap();
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    let response = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            &format!(
                "action=send&provider=xai&model=grok-4.6&thinking={}&message=Remember%20this&tool_list=list&directory_0={}",
                effort.as_str(),
                form_value(&grant.form_value())
            ),
        ))
        .await
        .unwrap();

    let status = response.status();
    let body = text(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let record = state.conversations.list().pop().unwrap();
    assert_eq!(
        record.model.as_ref().unwrap().settings.directories,
        vec![grant.clone()]
    );
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while state
            .conversations
            .get(&record.id)
            .unwrap()
            .active_job
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("directory-backed reply");
    assert!(state.workflow_runs.summaries().is_empty());
    let settled = state.conversations.get(&record.id).unwrap();
    assert_eq!(
        settled.model.as_ref().unwrap().settings.directories,
        vec![grant]
    );
    assert!(!state.conversation_runtime.unsettled(record.id));
}

#[tokio::test]
async fn network_tool_reply_uses_private_workspace_without_catalogue_identity() {
    let mut state = test_state();
    ready_starter_environment(&state).await;
    let (environment, _queued) = state
        .environments
        .create(crate::environments::EnvironmentDraft {
            name: "Network tools".to_owned(),
            oci_image: "docker.io/library/alpine:3.20".to_owned(),
            setup_script: String::new(),
        })
        .unwrap();
    let preparation = state
        .environments
        .claim_oldest_queued()
        .unwrap()
        .expect("custom preparation");
    let snapshot = crate::tests::sample_snapshot(preparation.id);
    state.environment_snapshots.mark(
        snapshot.artifact_key.clone(),
        crate::environments::snapshot::SnapshotAvailability::Available,
    );
    state
        .environments
        .finish_ready(
            &preparation.id,
            snapshot,
            crate::environments::PreparationLogRecord::empty(),
        )
        .unwrap();
    use crate::providers::ModelEvent;
    let backend = crate::providers::tests::ScriptedBackend::rounds(vec![
        vec![
            Ok(ModelEvent::Text("I will inspect the site.".to_owned())),
            Ok(ModelEvent::Thinking(
                "Inspect the response first.".to_owned(),
            )),
            Ok(ModelEvent::Text("The tool can read it.".to_owned())),
            Ok(ModelEvent::ToolCall {
                id: "call-1".to_owned(),
                name: "run".to_owned(),
                arguments: serde_json::json!({"command": "wget -qO- https://example.com"}),
            }),
            Ok(ModelEvent::Complete {
                reason: crate::providers::CompletionReason::ToolCalls,
            }),
        ],
        vec![
            Ok(ModelEvent::Thinking(String::new())),
            Ok(ModelEvent::Text("The network check completed.".to_owned())),
            Ok(ModelEvent::Complete {
                reason: crate::providers::CompletionReason::Stop,
            }),
        ],
    ]);
    state.chat = std::sync::Arc::new(crate::providers::ChatBackend::Scripted(backend.clone()));
    let token = connected(&state);
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    let response = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            &format!(
                "action=send&provider=xai&model=grok-4.6&thinking={}&message=Check%20the%20site&tool_run=run&environment={}&network=restricted&network_domains=example.com",
                effort.as_str(),
                environment.id
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let conversation = state.conversations.list().pop().expect("conversation");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while state
            .conversations
            .get(&conversation.id)
            .expect("conversation")
            .active_job
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("tool reply");
    assert!(state.workflow_runs.summaries().is_empty());
    assert!(!state.conversation_runtime.unsettled(conversation.id));
    let preamble = backend.last_preamble().unwrap_or_default();
    assert!(preamble.contains("/workspace"));
    assert!(!preamble.contains("# Role"));
    let settled = state.conversations.get(&conversation.id).unwrap();
    assert_eq!(
        settled
            .messages
            .iter()
            .filter(|message| message.text == "Check the site")
            .count(),
        1
    );
    let history = crate::conversations::history::project(&settled.messages, None).unwrap();
    assert_eq!(history.last().unwrap().text, "The network check completed.");
    assert_eq!(
        history
            .iter()
            .flat_map(|turn| &turn.calls)
            .filter(|call| call.result.is_some())
            .count(),
        1
    );
    assert_eq!(
        history
            .iter()
            .filter(|turn| turn.text.contains("I will inspect the site"))
            .count(),
        1
    );
}

#[tokio::test]
async fn first_message_preflight_requires_runtime_only_for_tools() {
    let state = test_state();
    let token = connected(&state);
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    let base = format!(
        "action=send&provider=xai&model=grok-4.6&thinking={}&message=Hello",
        effort.as_str()
    );
    let tool_fields = format!("{base}&tool_run=run");
    let invalid_domains =
        format!("{base}&network=restricted&network_domains=https%3A%2F%2Fexample.com");
    for fields in [&tool_fields, &invalid_domains] {
        let response = app(&state)
            .oneshot(command("/conversations/new", &token, fields))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(state.conversations.list().is_empty());
        assert!(!state.sessions.busy(&session_id(&token)));
    }
    state
        .sandboxes
        .set_missing_runtime(crate::sandbox::MissingRuntime::Both);
    let response = app(&state)
        .oneshot(command("/conversations/new", &token, &tool_fields))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(state.conversations.list().is_empty());
    let response = app(&state)
        .oneshot(command("/conversations/new", &token, &base))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let record = state.conversations.list().pop().unwrap();
    assert_eq!(record.messages[0].text, "Hello");
    let selected = record.model.unwrap().settings.environment;
    let selected_record = state.environments.get(&selected).unwrap();
    assert!(selected_record.ready_preparation.is_none());
    assert!(state.workflow_runs.summaries().is_empty());
}

#[tokio::test]
async fn host_first_message_runs_without_a_sandbox_runtime() {
    let mut state = test_state();
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("approved-command");
    let shell = format!(
        "printf approved > '{}'\nprintf 'finished\\n'",
        marker.display()
    );
    state.chat = std::sync::Arc::new(crate::providers::ChatBackend::Scripted(
        crate::providers::tests::ScriptedBackend::tool_then(
            "run",
            serde_json::json!({
                "command": shell,
                "explanation": "Write an approval marker in the temporary test directory",
            }),
            "Done",
        ),
    ));
    state.workflow_evidence = std::sync::Arc::new(
        crate::workflows::WorkflowEvidenceStore::open(directory.path().join("evidence")).unwrap(),
    );
    let token = connected(&state);
    state
        .sandboxes
        .set_missing_runtime(crate::sandbox::MissingRuntime::Both);
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    let fields = format!(
        "action=send&provider=xai&model=grok-4.6&thinking={}&message=Hello&location=host&tool_run=run",
        effort.as_str()
    );
    let preview_settings = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            &fields.replacen("action=send", "action=settings", 1),
        ))
        .await
        .unwrap();
    assert_eq!(preview_settings.status(), StatusCode::OK);
    assert!(state.conversations.list().is_empty());
    assert!(!state.sessions.busy(&session_id(&token)));
    let denied = app(&state)
        .oneshot(command("/conversations/new", &token, &fields))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(state.conversations.list().is_empty());

    let preview = app(&state)
        .oneshot(command(
            "/conversations/new/settings/host-consent",
            &token,
            &fields,
        ))
        .await
        .unwrap();
    assert_eq!(preview.status(), StatusCode::OK);
    let body = text(preview).await;
    assert!(body.contains("Allow this computer"));
    assert!(body.contains("Each Run command requires separate approval"));
    let request = host_consent_request(&body);
    let approved = app(&state)
        .oneshot(command(
            "/conversations/new/settings/host-consent/approve",
            &token,
            &format!("{fields}&host_consent_request={request}"),
        ))
        .await
        .unwrap();
    assert_eq!(
        approved.status(),
        StatusCode::OK,
        "{}",
        text(approved).await
    );
    let body = text(approved).await;
    let reference = hidden_named(&body, "consent_reference");
    let nonce = hidden_named(&body, "draft_nonce");
    let created = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            &format!("{fields}&consent_reference={reference}&draft_nonce={nonce}"),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK, "{}", text(created).await);
    assert_eq!(state.conversations.list().len(), 1);
    assert!(state.workflow_runs.summaries().is_empty());
    let settings = state.conversations.list()[0]
        .model
        .as_ref()
        .unwrap()
        .settings
        .clone();
    assert_eq!(settings.location, crate::execution::ToolLocation::Host);
    assert_eq!(settings.tools, vec![crate::agents::ToolId::Run]);
    let record = state.conversations.list()[0].clone();
    let job_id = record.active_job.unwrap();
    let request = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if let Some(request) = state.host_approvals.pending_for(record.id, job_id) {
                break request;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!marker.exists());
    let reloaded = app(&state)
        .oneshot(super::super::tests::document(
            &format!("/conversations/{}", record.id),
            &token,
        ))
        .await
        .unwrap();
    assert!(text(reloaded).await.contains(&request.token));
    let job = state.sessions.conversation_job(record.id, job_id).unwrap();
    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/host-command/approve", record.id),
            &token,
            &format!(
                "revision={}&job={}&request={}&command={}",
                request.execution_revision,
                job_id,
                request.token,
                super::super::tests::form_value(&serde_json::to_string(&shell).unwrap())
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        job.wait_for_terminal(std::time::Duration::from_secs(2))
            .await
    );
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "approved");
    let evidence: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            directory
                .path()
                .join("evidence/host")
                .join(format!("{}.json", request.token)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(evidence["status"], "finished");
    assert_eq!(evidence["command"], shell);
}

#[tokio::test]
async fn copied_host_policy_requires_new_consent() {
    let state = test_state();
    let token = connected(&state);
    let session = session_id(&token);
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None)
            .unwrap(),
        String::new(),
        vec![crate::agents::ToolId::Run],
        super::super::default_environment(&state).unwrap(),
    )
    .unwrap()
    .with_location(crate::execution::ToolLocation::Host)
    .with_host_approval(crate::execution::HostApprovalPolicy::Automatic);
    let source = state
        .conversations
        .create("Host source".to_owned())
        .unwrap();
    let source = state
        .conversations
        .update_execution_settings(&source.id, source.revision, settings.clone())
        .unwrap();
    state
        .access_consent
        .approve_host_conversation(
            &state
                .access_consent
                .request_host_conversation(session, source.id, &settings)
                .unwrap(),
            session,
            source.id,
            &settings,
        )
        .unwrap();
    let response = app(&state)
        .oneshot(document(
            &format!("/conversations/new?source={}", source.id),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("Not approved"));
    assert!(body.contains("Run without approval"));
    assert!(
        body.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains("name=\"consent_reference\" value=\"\"")
    );
    assert!(
        state
            .access_consent
            .authorised_host_conversation(session, source.id, &settings)
    );
}

#[tokio::test]
async fn automatic_host_first_message_runs_without_command_approval() {
    let mut state = test_state();
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("automatic-command");
    let shell = format!(
        "printf approved > '{}'\nprintf 'finished\\n'",
        marker.display()
    );
    state.chat = std::sync::Arc::new(crate::providers::ChatBackend::Scripted(
        crate::providers::tests::ScriptedBackend::tool_then(
            "run",
            serde_json::json!({
                "command": shell,
                "explanation": "Write an approval marker in the temporary test directory",
            }),
            "Done",
        ),
    ));
    state.workflow_evidence = std::sync::Arc::new(
        crate::workflows::WorkflowEvidenceStore::open(directory.path().join("evidence")).unwrap(),
    );
    let token = connected(&state);
    state
        .sandboxes
        .set_missing_runtime(crate::sandbox::MissingRuntime::Both);
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    let fields = format!(
        "action=send&provider=xai&model=grok-4.6&thinking={}&message=Hello&location=host&tool_run=run&host_approval=automatic",
        effort.as_str()
    );
    let denied = app(&state)
        .oneshot(command("/conversations/new", &token, &fields))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(state.conversations.list().is_empty());

    let preview = app(&state)
        .oneshot(command(
            "/conversations/new/settings/host-consent",
            &token,
            &fields,
        ))
        .await
        .unwrap();
    assert_eq!(preview.status(), StatusCode::OK);
    let body = text(preview).await;
    assert!(body.contains("Allow this computer"));
    assert!(
        body.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains("The model can run commands without an individual decision")
    );
    let request = host_consent_request(&body);
    let approved = app(&state)
        .oneshot(command(
            "/conversations/new/settings/host-consent/approve",
            &token,
            &format!("{fields}&host_consent_request={request}"),
        ))
        .await
        .unwrap();
    assert_eq!(
        approved.status(),
        StatusCode::OK,
        "{}",
        text(approved).await
    );
    let body = text(approved).await;
    let reference = hidden_named(&body, "consent_reference");
    let nonce = hidden_named(&body, "draft_nonce");
    let created = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            &format!("{fields}&consent_reference={reference}&draft_nonce={nonce}"),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK, "{}", text(created).await);
    assert_eq!(state.conversations.list().len(), 1);
    let record = state.conversations.list()[0].clone();
    assert_eq!(
        record.model.as_ref().unwrap().settings.host_approval,
        crate::execution::HostApprovalPolicy::Automatic
    );
    let job_id = record.active_job.unwrap();
    let job = state.sessions.conversation_job(record.id, job_id).unwrap();
    assert!(
        job.wait_for_terminal(std::time::Duration::from_secs(2))
            .await
    );
    assert!(
        state
            .host_approvals
            .pending_for(record.id, job_id)
            .is_none()
    );
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "approved");
    let evidence_dir = directory.path().join("evidence/host");
    let evidence_file = std::fs::read_dir(&evidence_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let evidence: serde_json::Value =
        serde_json::from_slice(&std::fs::read(evidence_file).unwrap()).unwrap();
    assert_eq!(evidence["status"], "finished");
    assert_eq!(evidence["command"], shell);
}

fn host_consent_request(body: &str) -> String {
    hidden_named(body, "host_consent_request")
}
