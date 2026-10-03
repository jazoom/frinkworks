use super::super::tests::*;
use crate::{
    conversations::{ConversationId, ConversationModelConfiguration},
    providers::{ModelSelection, ProviderKind},
};
use axum::http::StatusCode;
use tower::ServiceExt;

pub(super) fn conversation(
    state: &crate::state::AppState,
) -> crate::conversations::ConversationRecord {
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
    assert_eq!(grant.access, crate::execution::DirectoryAccess::Write);

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
async fn directory_access_changes_reach_the_next_model_request() {
    use crate::execution::{DirectoryAccess, DirectoryGrant};

    let mut state = test_state();
    ready_starter_environment(&state).await;
    let backend = crate::providers::tests::ScriptedBackend::accept();
    state.chat = std::sync::Arc::new(crate::providers::ChatBackend::Scripted(backend.clone()));
    let token = connected(&state);
    let mut record = conversation(&state);
    let directory = tempfile::tempdir().unwrap();
    let reference = tempfile::tempdir().unwrap();
    let grant = DirectoryGrant::from_selected(directory.path(), &[]).unwrap();
    let read_only =
        DirectoryGrant::from_selected(reference.path(), std::slice::from_ref(&grant)).unwrap();
    let mut settings = record.model.as_ref().unwrap().settings.clone();
    settings.model.thinking =
        state
            .models_dev
            .effective_effort(ProviderKind::Xai, "grok-4.6", None);
    settings.environment = super::super::default_environment(&state).unwrap();
    settings.tools = vec![crate::agents::ToolId::List];
    settings.directories = vec![grant.clone(), read_only.clone()];
    record = state
        .conversations
        .update_execution_settings(&record.id, record.revision, settings)
        .unwrap();
    let access_path = format!(
        "/conversations/{}/directories/{}/access",
        record.id,
        grant.id.as_hex()
    );
    let message_path = format!("/conversations/{}/messages", record.id);

    for access in [
        DirectoryAccess::Read,
        DirectoryAccess::Write,
        DirectoryAccess::Read,
    ] {
        let response = app(&state)
            .oneshot(command(
                &access_path,
                &token,
                &format!("revision={}&access={}", record.revision, access.as_str()),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{}",
            text(response).await
        );
        record = state.conversations.get(&record.id).unwrap();
        let response = app(&state)
            .oneshot(command(
                &message_path,
                &token,
                &format!(
                    "revision={}&message=Inspect%20the%20project",
                    record.revision
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
        record = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let current = state.conversations.get(&record.id).unwrap();
                if current.active_job.is_none() {
                    break current;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("reply settlement");
        let reply = record.messages.last().unwrap();
        assert_eq!(
            reply.status,
            crate::conversations::MessageStatus::Complete,
            "{reply:?}"
        );
        let preamble = backend.last_preamble().unwrap();
        let expected = match access {
            DirectoryAccess::Read => "Read",
            DirectoryAccess::Write => "Write: cancellation does not undo file changes.",
        };
        let prefix = format!("- {}: ", grant.guest_path());
        assert_eq!(
            preamble.lines().find_map(|line| line.strip_prefix(&prefix)),
            Some(expected),
        );
        assert!(
            preamble
                .lines()
                .any(|line| line == format!("- {}: Read", read_only.guest_path()))
        );
    }
    assert_eq!(backend.captured().len(), 3);
}

#[tokio::test]
async fn saved_access_selection_applies_permissions_and_rejects_native_stale_and_active_commands() {
    use crate::execution::{DirectoryAccess, DirectoryGrant};

    let state = test_state();
    let token = connected(&state);
    let record = conversation(&state);
    let directory = tempfile::tempdir().unwrap();
    let grant = DirectoryGrant::from_selected(directory.path(), &[]).unwrap();
    let mut current = state
        .conversations
        .add_directory(&record.id, record.revision, grant.clone())
        .unwrap();
    let path = format!(
        "/conversations/{}/directories/{}/access",
        record.id,
        grant.id.as_hex()
    );
    let mut native = command(
        &path,
        &token,
        &format!("revision={}&access=write", current.revision),
    );
    native.headers_mut().remove("graft-request");
    assert_eq!(
        app(&state).oneshot(native).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(state.conversations.get(&record.id), Some(current.clone()));

    for access in [DirectoryAccess::Write, DirectoryAccess::Read] {
        let response = app(&state)
            .oneshot(command(
                &path,
                &token,
                &format!("revision={}&access={}", current.revision, access.as_str()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            !text(response)
                .await
                .contains("id=\"conversation-directory-consent\"")
        );
        let updated = state.conversations.get(&record.id).unwrap();
        assert_eq!(
            updated.model.as_ref().unwrap().settings.directories[0].access,
            access
        );
        assert_eq!(
            app(&state)
                .oneshot(command(
                    &path,
                    &token,
                    &format!("revision={}&access=write", current.revision)
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(state.conversations.get(&record.id), Some(updated.clone()));
        current = updated;
    }
    let job = state
        .sessions
        .begin_conversation_job(&session_id(&token), record.id)
        .unwrap();
    let active = state
        .conversations
        .begin_message_with_model(
            &record.id,
            current.revision,
            current.model.clone(),
            job.id(),
            "Work".to_owned(),
        )
        .unwrap();
    assert_eq!(
        app(&state)
            .oneshot(command(
                &path,
                &token,
                &format!("revision={}&access=write", active.revision)
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(state.conversations.get(&record.id), Some(active));
}

#[tokio::test]
async fn ordinary_draft_access_applies_without_consent_and_rejects_native_commands() {
    use crate::execution::{DirectoryAccess, DirectoryGrant};

    let state = test_state();
    let token = connected(&state);
    let directory = tempfile::tempdir().unwrap();
    let grant = DirectoryGrant::from_selected(directory.path(), &[]).unwrap();
    let path = format!(
        "/conversations/new/directories/{}/access",
        grant.id.as_hex()
    );
    for access in [DirectoryAccess::Read, DirectoryAccess::Write] {
        let fields = format!(
            "action={}&directory_0={}",
            access.as_str(),
            form_value(&grant.form_value())
        );
        let mut native = command(&path, &token, &fields);
        native.headers_mut().remove("graft-request");
        assert_eq!(
            app(&state).oneshot(native).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
        let response = app(&state)
            .oneshot(command(&path, &token, &fields))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = text(response).await;
        assert!(hidden_value(&body, "consent_request").is_empty());
        assert!(hidden_value(&body, "pending_directory").is_empty());
        let updated = DirectoryGrant::parse_form(&hidden_value(&body, "directory_0")).unwrap();
        assert_eq!(updated.access, access);
    }
}

#[tokio::test]
async fn draft_access_changes_require_consent_only_for_sensitive_directories() {
    use crate::execution::{DirectoryAccess, DirectoryGrant};

    for sensitive in [false, true] {
        for access in [DirectoryAccess::Read, DirectoryAccess::Write] {
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
            assert_eq!(!previous_request.is_empty(), sensitive);
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
                .oneshot(command(&path, &token, &format!("{fields}&action=read")))
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
async fn credential_picker_and_access_require_consent_for_read_and_write() {
    use crate::execution::{DirectoryAccess, DirectoryGrant};

    const DIRECTORIES: [&str; 11] = [
        ".ssh",
        ".gnupg",
        ".aws",
        ".azure",
        ".kube",
        ".config",
        ".password-store",
        ".config/gcloud",
        ".config/gh",
        ".config/rclone",
        ".config/sops/age",
    ];
    const CHILD_HOME: &str = "FRINKWORKS_TEST_CREDENTIAL_CONSENT_HOME";
    let Some(home) = std::env::var_os(CHILD_HOME) else {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        for relative in DIRECTORIES {
            std::fs::create_dir_all(home.join(relative)).unwrap();
        }
        let name = concat!(
            module_path!(),
            "::credential_picker_and_access_require_consent_for_read_and_write"
        );
        // HOME belongs to the child process. Parallel tests never use the fixture or the user's credential directories.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name.split_once("::").unwrap().1])
            .env("HOME", &home)
            .env(CHILD_HOME, &home)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    };
    assert_eq!(std::env::var_os("HOME"), Some(home.clone()));
    let home = std::path::PathBuf::from(home);
    let state = test_state();
    let token = connected(&state);

    for relative in DIRECTORIES {
        let directory = home.join(relative);
        for saved in [false, true] {
            let record = saved.then(|| conversation(&state));
            state.directory_picker.queue(Some(directory.clone()));
            let path = record.as_ref().map_or_else(
                || "/conversations/new/directories/pick".to_owned(),
                |record| format!("/conversations/{}/directories/pick", record.id),
            );
            let response = app(&state)
                .oneshot(command(&path, &token, "revision=1"))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = text(response).await;
            assert!(body.contains("id=\"conversation-directory-consent\""));
            assert!(!hidden_value(&body, "consent_request").is_empty());
            let pending =
                DirectoryGrant::parse_form(&hidden_value(&body, "pending_directory")).unwrap();
            assert_eq!(pending.host_path, directory.canonicalize().unwrap());
            assert_eq!(pending.access, DirectoryAccess::Write);
            if let Some(record) = record {
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
            } else {
                assert!(!body.contains("name=\"directory_0\""));
            }
        }

        for access in [DirectoryAccess::Read, DirectoryAccess::Write] {
            let state = test_state();
            let token = connected(&state);
            let session = session_id(&token);
            let mut grant = DirectoryGrant::from_selected(&directory, &[]).unwrap();
            grant.access = access;
            assert!(grant.requires_access_consent(state.local_data.root()));
            let path = format!(
                "/conversations/new/directories/{}/access",
                grant.id.as_hex()
            );
            let response = app(&state)
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
            assert_eq!(response.status(), StatusCode::OK);
            assert!(
                text(response)
                    .await
                    .contains("id=\"conversation-directory-consent\"")
            );

            let record = conversation(&state);
            let mut settings = record.model.as_ref().unwrap().settings.clone();
            settings.tools.clear();
            settings.directories = vec![grant.clone()];
            let record = state
                .conversations
                .update_execution_settings(&record.id, record.revision, settings)
                .unwrap();
            assert!(matches!(
                super::super::preflight_execution(
                    &state,
                    session,
                    Some(record.id),
                    record.model.as_ref().unwrap()
                )
                .await,
                Err(super::super::StartMessageError::User(
                    hypergraft::PatchStatus::UnprocessableEntity,
                    _
                ))
            ));
            let path = format!(
                "/conversations/{}/directories/{}/access",
                record.id,
                grant.id.as_hex()
            );
            let response = app(&state)
                .oneshot(command(
                    &path,
                    &token,
                    &format!("revision={}&access={}", record.revision, access.as_str()),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = text(response).await;
            assert!(body.contains("id=\"conversation-directory-consent\""));
            let approval = format!(
                "revision={}&existing=true&consent_request={}&pending_directory={}",
                record.revision,
                form_value(&hidden_value(&body, "consent_request")),
                form_value(&hidden_value(&body, "pending_directory")),
            );
            let path = format!("/conversations/{}/directories/consent", record.id);
            let response = app(&state)
                .oneshot(command(&path, &token, &approval))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let approved = state.conversations.get(&record.id).unwrap();
            assert!(
                super::super::preflight_execution(
                    &state,
                    session,
                    Some(approved.id),
                    approved.model.as_ref().unwrap()
                )
                .await
                .is_ok()
            );
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
        .oneshot(document(&format!("/conversations/{}", record.id), &token))
        .await
        .unwrap();
    assert_eq!(restored.status(), StatusCode::OK);
    assert!(state.sessions.contains_live(&session));
    assert!(!text(restored).await.contains("Pending approval"));
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
    one.access = DirectoryAccess::Write;
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
    one.access = DirectoryAccess::Write;
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

pub(super) fn hidden_value(body: &str, name: &str) -> String {
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
