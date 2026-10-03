use axum::http::StatusCode;
use tower::ServiceExt;

use super::super::super::tests::*;
use super::super::tests::hidden_value;
use crate::execution::{DirectoryAccess, DirectoryGrant};

#[tokio::test]
async fn saved_recent_commands_preserve_other_conversations_and_do_not_restore_forgotten_shortcuts()
{
    let state = test_state();
    let token = connected(&state);
    let root = tempfile::tempdir().unwrap();
    let grant = DirectoryGrant::from_selected(root.path(), &[]).unwrap();
    let first = super::super::tests::conversation(&state);
    let first = state
        .conversations
        .add_directory(&first.id, first.revision, grant.clone())
        .unwrap();
    state.preferences.remember_directory(&grant, true).unwrap();
    let second = super::super::tests::conversation(&state);
    let select = format!(
        "/conversations/{}/directories/recent/{}/select",
        second.id,
        grant.id.as_hex()
    );
    let forget = select.replace("/select", "/forget");
    let fields = format!("revision={}", second.revision);
    let mut native = command(&select, &token, &fields);
    native.headers_mut().remove("graft-request");
    assert_eq!(
        app(&state).oneshot(native).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app(&state)
            .oneshot(command(&select, &token, "revision=0"))
            .await
            .unwrap()
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let response = app(&state)
        .oneshot(command(&select, &token, &fields))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        text(response)
            .await
            .contains("target=\"conversation-detail\"")
    );
    assert_eq!(
        app(&state)
            .oneshot(command(&select, &token, &fields))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let selected = state.conversations.get(&second.id).unwrap();
    let selected_grant = &selected.model.as_ref().unwrap().settings.directories[0];
    let fields = format!("revision={}", selected.revision);
    assert_eq!(
        app(&state)
            .oneshot(command(&forget, &token, &fields))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(state.preferences.recent_directories().len(), 1);
    let remove = format!(
        "/conversations/{}/directories/{}/remove",
        second.id,
        selected_grant.id.as_hex()
    );
    assert_eq!(
        app(&state)
            .oneshot(command(&remove, &token, &fields))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let second = state.conversations.get(&second.id).unwrap();
    let fields = format!("revision={}", second.revision);
    let mut native = command(&forget, &token, &fields);
    native.headers_mut().remove("graft-request");
    assert_eq!(
        app(&state).oneshot(native).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app(&state)
            .oneshot(command(&forget, &token, &fields))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(state.conversations.get(&first.id), Some(first.clone()));
    assert!(state.preferences.directory_approved(&grant));
    assert!(state.preferences.recent_directories().is_empty());
    let access = format!(
        "/conversations/{}/directories/{}/access",
        first.id,
        grant.id.as_hex()
    );
    assert_eq!(
        app(&state)
            .oneshot(command(
                &access,
                &token,
                &format!("revision={}&access=write", first.revision)
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert!(state.preferences.recent_directories().is_empty());
    let first = state.conversations.get(&first.id).unwrap();
    assert_eq!(
        first.model.as_ref().unwrap().settings.directories[0].access,
        DirectoryAccess::Write
    );
    state.preferences.remember_directory(&grant, false).unwrap();
    let job = state
        .sessions
        .begin_conversation_job(&session_id(&token), second.id)
        .unwrap();
    let active = state
        .conversations
        .begin_message_with_model(
            &second.id,
            second.revision,
            second.model.clone(),
            job.id(),
            "Work".to_owned(),
        )
        .unwrap();
    assert_eq!(
        app(&state)
            .oneshot(command(
                &select,
                &token,
                &format!("revision={}", active.revision)
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(state.conversations.get(&second.id), Some(active));
}

#[tokio::test]
async fn recent_commands_are_patch_only_and_restore_modes_without_loss_of_the_draft() {
    let state = test_state();
    let token = connected(&state);
    let root = tempfile::tempdir().unwrap();
    state
        .directory_picker
        .queue(Some(root.path().to_path_buf()));
    let response = app(&state)
        .oneshot(command(
            "/conversations/new/directories/pick",
            &token,
            "draft_nonce=draft&message=Keep+this+text",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    let grant = DirectoryGrant::parse_form(&hidden_value(&body, "directory_0")).unwrap();
    assert_eq!(grant.access, DirectoryAccess::Write);
    let access_path = format!(
        "/conversations/new/directories/{}/access?recent=true",
        grant.id.as_hex()
    );
    let response = app(&state)
        .oneshot(command(
            &access_path,
            &token,
            &format!(
                "draft_nonce=draft&action=read&directory_0={}&message=Keep+this+text",
                form_value(&grant.form_value())
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(text(response).await.contains("Keep this text"));
    let entry = &state.preferences.recent_directories()[0];
    assert_eq!(entry.access, DirectoryAccess::Read);
    state
        .directory_picker
        .queue(Some(root.path().to_path_buf()));
    let response = app(&state)
        .oneshot(command(
            "/conversations/new/directories/pick",
            &token,
            "draft_nonce=repicked",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    let repicked = DirectoryGrant::parse_form(&hidden_value(&body, "directory_0")).unwrap();
    assert_eq!(repicked.access, DirectoryAccess::Read);
    assert_eq!(repicked.host_path, grant.host_path);
    let select = format!("/conversations/new/directories/recent/{}/select", entry.id);
    let mut native = command(&select, &token, "draft_nonce=fresh");
    native.headers_mut().remove("graft-request");
    assert_eq!(
        app(&state).oneshot(native).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    let response = app(&state)
        .oneshot(command(
            &select,
            &token,
            "draft_nonce=fresh&message=Keep+this+text",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("target=\"conversation-detail\""));
    assert!(body.contains("Keep this text"));
    let selected = DirectoryGrant::parse_form(&hidden_value(&body, "directory_0")).unwrap();
    assert_eq!(selected.access, DirectoryAccess::Read);
    assert_eq!(selected.host_path, grant.host_path);
    let fields = format!(
        "draft_nonce=fresh&directory_0={}&message=Keep+this+text",
        form_value(&selected.form_value())
    );
    let forget = select.replace("/select", "/forget");
    let mut native = command(&forget, &token, &fields);
    native.headers_mut().remove("graft-request");
    assert_eq!(
        app(&state).oneshot(native).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    let response = app(&state)
        .oneshot(command(&forget, &token, &fields))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(state.preferences.recent_directories().len(), 1);
    let body = text(response).await;
    assert_eq!(
        DirectoryGrant::parse_form(&hidden_value(&body, "directory_0")),
        Some(selected.clone())
    );
    let remove = format!(
        "/conversations/new/directories/{}/remove",
        selected.id.as_hex()
    );
    let response = app(&state)
        .oneshot(command(&remove, &token, &fields))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app(&state)
        .oneshot(command(
            &forget,
            &token,
            "draft_nonce=fresh&message=Keep+this+text",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(state.preferences.recent_directories().is_empty());
    assert!(text(response).await.contains("Keep this text"));
    assert_eq!(
        app(&state)
            .oneshot(command(&select, &token, ""))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert!(state.conversations.list().is_empty());
}

#[tokio::test]
async fn sensitive_consent_survives_restart_but_never_grants_host_consent() {
    let mut state = test_state();
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    std::fs::create_dir(&data).unwrap();
    state.local_data = crate::local_data::LocalDataReset::for_test(data.clone());
    let path = data.join("preferences.json");
    state.preferences = std::sync::Arc::new(crate::preferences::Preferences::open(path.clone()));
    state.access_consent = std::sync::Arc::new(crate::execution::AccessConsentStore::new(
        state.preferences.clone(),
    ));
    let token = connected(&state);
    state
        .directory_picker
        .queue(Some(root.path().to_path_buf()));
    let response = app(&state)
        .oneshot(command(
            "/conversations/new/directories/pick",
            &token,
            "draft_nonce=original",
        ))
        .await
        .unwrap();
    let body = text(response).await;
    let fields = format!(
        "draft_nonce=original&consent_request={}&pending_directory={}",
        form_value(&hidden_value(&body, "consent_request")),
        form_value(&hidden_value(&body, "pending_directory"))
    );
    let response = app(&state)
        .oneshot(command(
            "/conversations/new/directories/consent",
            &token,
            &fields,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let grant =
        DirectoryGrant::parse_form(&hidden_value(&text(response).await, "directory_0")).unwrap();
    assert!(state.preferences.directory_approved(&grant));
    state.preferences = std::sync::Arc::new(crate::preferences::Preferences::open(path));
    state.access_consent = std::sync::Arc::new(crate::execution::AccessConsentStore::new(
        state.preferences.clone(),
    ));
    let token = connected(&state);
    let entry = &state.preferences.recent_directories()[0];
    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/new/directories/recent/{}/select", entry.id),
            &token,
            "draft_nonce=fresh",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(!body.contains("id=\"conversation-directory-consent\""));
    assert!(!body.contains("Pending approval"));
    let grant = DirectoryGrant::parse_form(&hidden_value(&body, "directory_0")).unwrap();
    let grants = vec![grant.clone()];
    let session = session_id(&token);
    assert!(
        state
            .access_consent
            .authorised_draft("", session, "fresh", &grants, &grant)
    );
    assert!(
        state
            .access_consent
            .authorised_draft_preview("", session, "fresh", &grant)
    );
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(
            crate::providers::ProviderKind::Xai,
            "grok-4.6".to_owned(),
            None,
        )
        .unwrap(),
        String::new(),
        Vec::new(),
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_directories(grants.clone())
    .unwrap();
    let conversation = crate::conversations::ConversationId::generate().unwrap();
    state
        .access_consent
        .consume_draft("", session, "fresh", &settings, conversation, &grants)
        .unwrap();
    assert!(
        state
            .access_consent
            .authorised_conversation(session, conversation, &settings, &grant)
    );
    assert!(
        !state.access_consent.authorised_host_draft(
            "",
            session,
            "fresh",
            &settings
                .clone()
                .with_location(crate::execution::ToolLocation::Host)
        )
    );
    let mut other = grant.clone();
    other.host_path = data;
    assert!(!state.preferences.directory_approved(&other));
}

#[cfg(unix)]
#[tokio::test]
async fn recent_selection_rejects_redirected_paths_and_overlapping_grants() {
    let state = test_state();
    let token = connected(&state);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("project");
    let other = root.path().join("other");
    std::fs::create_dir(&path).unwrap();
    std::fs::create_dir(&other).unwrap();
    let grant = DirectoryGrant::from_selected(&path, &[]).unwrap();
    state.preferences.remember_directory(&grant, true).unwrap();
    let entry = &state.preferences.recent_directories()[0];
    let select = format!("/conversations/new/directories/recent/{}/select", entry.id);
    let parent = DirectoryGrant::from_selected(root.path(), &[]).unwrap();
    let response = app(&state)
        .oneshot(command(
            &select,
            &token,
            &format!("directory_0={}", form_value(&parent.form_value())),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        DirectoryGrant::parse_form(&hidden_value(&text(response).await, "directory_0")),
        Some(parent)
    );
    std::fs::remove_dir(&path).unwrap();
    std::os::unix::fs::symlink(&other, &path).unwrap();
    let response = app(&state)
        .oneshot(command(&select, &token, ""))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!text(response).await.contains("name=\"directory_0\""));
    assert!(!state.preferences.directory_approved(&grant));
    let redirected = DirectoryGrant::from_selected(&path, &[]).unwrap();
    assert!(!state.preferences.directory_approved(&redirected));
}
