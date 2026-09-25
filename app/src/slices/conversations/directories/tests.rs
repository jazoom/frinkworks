use super::super::tests::*;
use crate::{
    conversations::{ConversationId, ConversationModelConfiguration},
    providers::{ModelSelection, ProviderKind},
};
use axum::http::StatusCode;
use tower::ServiceExt;

fn conversation(state: &crate::state::AppState) -> crate::conversations::ConversationRecord {
    state
        .conversations
        .create_saved(
            ConversationId::generate().unwrap(),
            Some("Directory test".to_owned()),
            Some(ConversationModelConfiguration::direct(
                ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None).unwrap(),
                crate::tests::test_environment_id(),
            )),
            Vec::new(),
        )
        .unwrap()
}

#[tokio::test]
async fn picker_commands_are_patch_only_and_revision_bound() {
    let state = test_state();
    let token = connected(&state);
    let record = conversation(&state);
    let directory = tempfile::tempdir().unwrap();
    state
        .directory_picker
        .queue(Some(directory.path().to_path_buf()));
    let path = format!("/conversations/{}/directories/pick", record.id);

    let mut native = command(&path, &token, "revision=1");
    native.headers_mut().remove("graft-request");
    assert_eq!(
        app(&state).oneshot(native).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    assert!(
        state
            .conversations
            .get(&record.id)
            .unwrap()
            .model
            .unwrap()
            .settings
            .directories
            .is_empty()
    );

    let response = app(&state)
        .oneshot(command(&path, &token, "revision=1"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("target=\"conversation-detail\""));
    let updated = state.conversations.get(&record.id).unwrap();
    let grant = &updated.model.as_ref().unwrap().settings.directories[0];
    assert_eq!(grant.host_path, directory.path().canonicalize().unwrap());

    let remove = format!(
        "/conversations/{}/directories/{}/remove",
        record.id,
        grant.id.as_hex()
    );
    assert_eq!(
        app(&state)
            .oneshot(command(&remove, &token, "revision=1"))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        state
            .conversations
            .get(&record.id)
            .unwrap()
            .model
            .unwrap()
            .settings
            .directories
            .len(),
        1
    );
}

#[tokio::test]
async fn direct_write_needs_destination_consent() {
    directory_consent_case(crate::execution::DirectoryAccess::DirectWrite).await;
}

async fn directory_consent_case(access: crate::execution::DirectoryAccess) {
    let state = test_state();
    let token = connected(&state);
    let record = conversation(&state);
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let first_grant = crate::execution::DirectoryGrant::from_selected(first.path(), &[]).unwrap();
    let first_id = first_grant.id;
    let current = state
        .conversations
        .add_directory(&record.id, 1, first_grant)
        .unwrap();
    let second_grant = crate::execution::DirectoryGrant::from_selected(
        second.path(),
        &current.model.as_ref().unwrap().settings.directories,
    )
    .unwrap();
    let second_id = second_grant.id;
    let current = state
        .conversations
        .add_directory(&record.id, current.revision, second_grant)
        .unwrap();

    let access_path = format!(
        "/conversations/{}/directories/{}/access",
        record.id,
        first_id.as_hex()
    );
    let preview = app(&state)
        .oneshot(command(
            &access_path,
            &token,
            &format!("revision={}&access={}", current.revision, access.as_str()),
        ))
        .await
        .unwrap();
    assert_eq!(preview.status(), StatusCode::OK);
    let body = text(preview).await;
    assert!(!hidden_value(&body, "consent_request").is_empty());
    assert_eq!(
        state
            .conversations
            .get(&record.id)
            .unwrap()
            .model
            .unwrap()
            .settings
            .directories[0]
            .access,
        crate::execution::DirectoryAccess::ReadOnly
    );
    let consent_body = format!(
        "revision={}&consent_request={}&pending_directory={}&existing=true",
        current.revision,
        form_value(&hidden_value(&body, "consent_request")),
        form_value(&hidden_value(&body, "pending_directory")),
    );
    let approved = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/directories/consent", record.id),
            &token,
            &consent_body,
        ))
        .await
        .unwrap();
    let approved_status = approved.status();
    let approved_body = text(approved).await;
    assert_eq!(approved_status, StatusCode::OK, "{approved_body}");
    let updated = state.conversations.get(&record.id).unwrap();
    assert_eq!(
        updated.model.as_ref().unwrap().settings.directories[0].access,
        access
    );

    let second_response = app(&state)
        .oneshot(command(
            &format!(
                "/conversations/{}/directories/{}/access",
                record.id,
                second_id.as_hex()
            ),
            &token,
            &format!("revision={}&access={}", updated.revision, access.as_str()),
        ))
        .await
        .unwrap();
    assert_eq!(second_response.status(), StatusCode::OK);
    let second_body = text(second_response).await;
    assert!(!hidden_value(&second_body, "consent_request").is_empty());
    assert_eq!(
        state
            .conversations
            .get(&record.id)
            .unwrap()
            .model
            .unwrap()
            .settings
            .directories[1]
            .access,
        crate::execution::DirectoryAccess::ReadOnly
    );
}

#[tokio::test]
async fn non_sensitive_review_access_needs_no_consent() {
    let state = test_state();
    let token = connected(&state);
    let directory = tempfile::tempdir().unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(directory.path(), &[]).unwrap();
    let path = format!(
        "/conversations/new/directories/{}/access",
        grant.id.as_hex()
    );
    let preview = app(&state)
        .oneshot(command(
            &path,
            &token,
            &format!(
                "action=review-before-apply&directory_0={}",
                form_value(&grant.form_value())
            ),
        ))
        .await
        .unwrap();
    assert_eq!(preview.status(), StatusCode::OK);
    let body = text(preview).await;
    assert!(hidden_value(&body, "consent_request").is_empty());
    assert!(hidden_value(&body, "pending_directory").is_empty());
    let updated =
        crate::execution::DirectoryGrant::parse_form(&hidden_value(&body, "directory_0")).unwrap();
    assert_eq!(
        updated.access,
        crate::execution::DirectoryAccess::ReviewBeforeApply
    );
}

#[tokio::test]
async fn draft_read_only_replaces_stale_write_consent_without_bypassing_sensitive_consent() {
    use crate::execution::{DirectoryAccess, DirectoryGrant};

    for sensitive in [false, true] {
        for access in [
            DirectoryAccess::ReviewBeforeApply,
            DirectoryAccess::DirectWrite,
        ] {
            let mut state = test_state();
            let token = connected(&state);
            let directory = tempfile::tempdir().unwrap();
            if sensitive {
                let data = directory.path().join("frinkworks-data");
                std::fs::create_dir(&data).unwrap();
                state.local_data = crate::local_data::LocalDataReset::for_test(data);
            }
            let grant = DirectoryGrant::from_selected(directory.path(), &[]).unwrap();
            let path = format!(
                "/conversations/new/directories/{}/access",
                grant.id.as_hex()
            );
            let preview = app(&state)
                .oneshot(command(
                    &path,
                    &token,
                    &format!(
                        "action={}&directory_0={}",
                        access.as_str(),
                        form_value(&grant.form_value())
                    ),
                ))
                .await
                .unwrap();
            assert_eq!(preview.status(), StatusCode::OK);
            let preview = text(preview).await;
            let previous_request = hidden_value(&preview, "consent_request");
            assert_eq!(
                !previous_request.is_empty(),
                sensitive || access == DirectoryAccess::DirectWrite
            );
            let fields = [
                "draft_nonce",
                "directory_0",
                "pending_directory",
                "consent_request",
                "consent_existing",
            ]
            .map(|name| format!("{name}={}", form_value(&hidden_value(&preview, name))))
            .join("&");
            let response = app(&state)
                .oneshot(command(
                    &path,
                    &token,
                    &format!("{fields}&action=read-only"),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = text(response).await;
            let current = DirectoryGrant::parse_form(&hidden_value(&body, "directory_0")).unwrap();
            assert_eq!(current, grant);
            assert_eq!(
                body.contains("id=\"conversation-directory-consent\""),
                sensitive
            );
            let request = hidden_value(&body, "consent_request");
            let pending = hidden_value(&body, "pending_directory");
            if sensitive {
                assert!(!request.is_empty());
                assert_ne!(request, previous_request);
                assert_eq!(DirectoryGrant::parse_form(&pending), Some(grant));
                assert_eq!(hidden_value(&body, "consent_existing"), "true");
            } else {
                assert!(request.is_empty());
                assert!(pending.is_empty());
                assert_eq!(hidden_value(&body, "consent_existing"), "false");
            }
            assert!(state.conversations.list().is_empty());
        }
    }
}

#[tokio::test]
async fn sensitive_saved_grant_needs_exact_single_use_consent() {
    let mut state = test_state();
    let home = tempfile::tempdir().unwrap();
    let data = home.path().join("frinkworks-data");
    std::fs::create_dir(&data).unwrap();
    state.local_data = crate::local_data::LocalDataReset::for_test(data.clone());
    let token = connected(&state);
    let record = conversation(&state);
    state
        .directory_picker
        .queue(Some(home.path().to_path_buf()));
    let path = format!("/conversations/{}/directories/pick", record.id);

    let preview = app(&state)
        .oneshot(command(&path, &token, "revision=1"))
        .await
        .unwrap();
    assert_eq!(preview.status(), StatusCode::OK);
    assert!(
        state
            .conversations
            .get(&record.id)
            .unwrap()
            .model
            .unwrap()
            .settings
            .directories
            .is_empty()
    );
    let body = text(preview).await;
    assert!(body.contains(&data.to_string_lossy().replace('&', "&amp;")));
    let request = hidden_value(&body, "consent_request");
    let grant = hidden_value(&body, "pending_directory");
    let consent_path = format!("/conversations/{}/directories/consent", record.id);
    let consent_body = format!(
        "revision=1&consent_request={}&pending_directory={}",
        form_value(&request),
        form_value(&grant),
    );
    let approved = app(&state)
        .oneshot(command(&consent_path, &token, &consent_body))
        .await
        .unwrap();
    let approved_status = approved.status();
    let approved_body = text(approved).await;
    assert_eq!(approved_status, StatusCode::OK, "{approved_body}");
    let updated = state.conversations.get(&record.id).unwrap();
    assert_eq!(updated.model.unwrap().settings.directories.len(), 1);

    let replay = app(&state)
        .oneshot(command(&consent_path, &token, &consent_body))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let current = state.conversations.get(&record.id).unwrap();
    let settings = &current.model.as_ref().unwrap().settings;
    let grant = &settings.directories[0];
    let session = session_id(&token);
    assert!(
        state
            .access_consent
            .authorised_conversation(session, record.id, settings, grant)
    );
    state
        .sessions
        .advance_clock(crate::sessions::SESSION_LIFETIME + std::time::Duration::from_secs(1));
    let restored = app(&state)
        .oneshot(command(
            &format!(
                "/conversations/{}/directories/{}/consent",
                record.id,
                grant.id.as_hex()
            ),
            &token,
            &format!("revision={}", current.revision),
        ))
        .await
        .unwrap();
    assert_eq!(restored.status(), StatusCode::OK);
    assert!(state.sessions.contains_live(&session));
    assert!(
        !state
            .access_consent
            .authorised_conversation(session, record.id, settings, grant)
    );
    let preview = text(restored).await;
    let request = hidden_value(&preview, "consent_request");
    let pending = hidden_value(&preview, "pending_directory");
    let renamed = state
        .conversations
        .rename(&record.id, current.revision, "Renamed".to_owned())
        .unwrap();
    let body = format!(
        "revision={}&existing=true&consent_request={}&pending_directory={}",
        current.revision,
        form_value(&request),
        form_value(&pending)
    );
    let stale = app(&state)
        .oneshot(command(&consent_path, &token, &body))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert!(
        !state
            .access_consent
            .authorised_conversation(session, record.id, settings, grant)
    );
    let body = body.replacen(
        &format!("revision={}", current.revision),
        &format!("revision={}", renamed.revision),
        1,
    );
    let approved = app(&state)
        .oneshot(command(&consent_path, &token, &body))
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);
    assert!(
        state
            .access_consent
            .authorised_conversation(session, record.id, settings, grant)
    );
}

#[tokio::test]
async fn draft_command_directory_preserves_grants_but_not_consent() {
    use crate::execution::{DirectoryAccess, DirectoryGrant};

    let state = test_state();
    let token = connected(&state);
    let session = session_id(&token);
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let mut one = DirectoryGrant::from_selected(first.path(), &[]).unwrap();
    one.access = DirectoryAccess::DirectWrite;
    let two = DirectoryGrant::from_selected(second.path(), std::slice::from_ref(&one)).unwrap();
    let grants = vec![one.clone(), two.clone()];
    let mut form = super::super::new::NewForm {
        draft_nonce: "draft".to_owned(),
        ..Default::default()
    };
    form.set_directories(&grants);
    let nonce = form.consent_nonce();
    let request = state
        .access_consent
        .request_draft(session, &nonce, &grants, &one)
        .unwrap();
    let reference = state
        .access_consent
        .approve_draft(&request, session, &nonce, &grants, &one)
        .unwrap();
    let fields = format!(
        "draft_nonce=draft&message=Unsent+text&directory_0={}&directory_1={}&consent_reference={}",
        form_value(&one.form_value()),
        form_value(&two.form_value()),
        reference
    );
    let path = "/conversations/new/directories/start";
    let mut native = command(
        path,
        &token,
        &format!("{fields}&start_directory={}", two.id.as_hex()),
    );
    native.headers_mut().remove("graft-request");
    assert_eq!(
        app(&state).oneshot(native).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    for selected in [
        "invalid".to_owned(),
        crate::execution::DirectoryGrantId::generate()
            .unwrap()
            .as_hex(),
    ] {
        let response = app(&state)
            .oneshot(command(
                path,
                &token,
                &format!("{fields}&start_directory={selected}"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            DirectoryGrant::parse_form(&hidden_value(&text(response).await, "directory_0")),
            Some(one.clone())
        );
    }
    let response = app(&state)
        .oneshot(command(
            path,
            &token,
            &format!("{fields}&start_directory={}", two.id.as_hex()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("target=\"conversation-detail\""));
    assert!(body.contains("Unsent text"));
    assert_eq!(
        DirectoryGrant::parse_form(&hidden_value(&body, "directory_0")),
        Some(two.clone())
    );
    assert_eq!(
        DirectoryGrant::parse_form(&hidden_value(&body, "directory_1")),
        Some(one.clone())
    );
    assert!(hidden_value(&body, "consent_reference").is_empty());
    assert!(!state.access_consent.authorised_draft(
        &reference,
        session,
        &nonce,
        &[two.clone(), one.clone()],
        &one
    ));
    assert!(state.conversations.list().is_empty());

    drop(second);
    let response = app(&state)
        .oneshot(command(
            path,
            &token,
            &format!("{fields}&start_directory={}", two.id.as_hex()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn saved_command_directory_is_revision_bound_and_invalidates_authority() {
    use crate::execution::{DirectoryAccess, DirectoryGrant, ToolLocation};

    let state = test_state();
    let token = connected(&state);
    let session = session_id(&token);
    let record = conversation(&state);
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let mut one = DirectoryGrant::from_selected(first.path(), &[]).unwrap();
    one.access = DirectoryAccess::DirectWrite;
    let two = DirectoryGrant::from_selected(second.path(), std::slice::from_ref(&one)).unwrap();
    let settings = record
        .model
        .unwrap()
        .settings
        .with_directories(vec![one.clone(), two.clone()])
        .unwrap()
        .with_location(ToolLocation::Host);
    let current = state
        .conversations
        .update_execution_settings(&record.id, record.revision, settings.clone())
        .unwrap();
    let current = state
        .conversations
        .record_directory_approval(
            &record.id,
            current.revision,
            crate::conversations::DirectoryApproval::for_grant(&settings, &one),
        )
        .unwrap();
    let request = state
        .access_consent
        .request_host_conversation(session, record.id, &settings)
        .unwrap();
    state
        .access_consent
        .approve_host_conversation(&request, session, record.id, &settings)
        .unwrap();
    let path = format!("/conversations/{}/directories/start", record.id);
    let body = format!(
        "revision={}&start_directory={}",
        current.revision,
        two.id.as_hex()
    );
    let mut native = command(&path, &token, &body);
    native.headers_mut().remove("graft-request");
    assert_eq!(
        app(&state).oneshot(native).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    for (body, status) in [
        (
            format!("revision=1&start_directory={}", two.id.as_hex()),
            StatusCode::CONFLICT,
        ),
        (
            format!(
                "revision={}&start_directory={}",
                current.revision,
                crate::execution::DirectoryGrantId::generate()
                    .unwrap()
                    .as_hex()
            ),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        assert_eq!(
            app(&state)
                .oneshot(command(&path, &token, &body))
                .await
                .unwrap()
                .status(),
            status
        );
        assert_eq!(
            state
                .conversations
                .get(&record.id)
                .unwrap()
                .model
                .unwrap()
                .settings,
            settings
        );
    }
    let unchanged = app(&state)
        .oneshot(command(
            &path,
            &token,
            &format!(
                "revision={}&start_directory={}",
                current.revision,
                one.id.as_hex()
            ),
        ))
        .await
        .unwrap();
    assert_eq!(unchanged.status(), StatusCode::OK);
    assert_eq!(
        state.conversations.get(&record.id).unwrap().revision,
        current.revision
    );
    assert!(
        state
            .access_consent
            .authorised_host_conversation(session, record.id, &settings)
    );

    let response = app(&state)
        .oneshot(command(&path, &token, &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let updated = state.conversations.get(&record.id).unwrap();
    let mut expected = settings.clone();
    expected.directories = vec![two.clone(), one.clone()];
    assert_eq!(updated.model.as_ref().unwrap().settings, expected);
    assert_eq!(
        crate::execution::command_directory(&expected.directories),
        two.host_path
    );
    assert!(updated.directory_approvals.is_empty());
    assert!(
        !state
            .access_consent
            .authorised_host_conversation(session, record.id, &settings)
    );
    assert!(
        !state
            .access_consent
            .authorised_host_conversation(session, record.id, &expected)
    );
    assert!(
        !state
            .conversations
            .directory_approved(&record.id, &expected, &one)
    );

    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .unwrap();
    let active = state
        .conversations
        .begin_message_with_model(
            &record.id,
            updated.revision,
            updated.model.clone(),
            job.id(),
            "No model request".to_owned(),
        )
        .unwrap();
    let response = app(&state)
        .oneshot(command(
            &path,
            &token,
            &format!(
                "revision={}&start_directory={}",
                active.revision,
                one.id.as_hex()
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        state
            .conversations
            .get(&record.id)
            .unwrap()
            .model
            .unwrap()
            .settings,
        expected
    );
}

fn hidden_value(body: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\"");
    let tail = &body[body.find(&marker).expect("hidden field") + marker.len()..];
    let value = &tail[tail.find("value=\"").expect("value") + 7..];
    value[..value.find('"').expect("value end")]
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&amp;", "&")
        .replace("&#x2F;", "/")
        .replace("&#x3D;", "=")
}

#[tokio::test]
async fn picker_cancellation_creates_no_grant() {
    let state = test_state();
    let token = connected(&state);
    let record = conversation(&state);
    state.directory_picker.queue(None);

    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/directories/pick", record.id),
            &token,
            "revision=1",
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        state
            .conversations
            .get(&record.id)
            .unwrap()
            .model
            .unwrap()
            .settings
            .directories
            .is_empty()
    );
}
