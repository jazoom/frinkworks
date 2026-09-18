use std::sync::Arc;

use axum::http::StatusCode;
use tower::ServiceExt;

use super::super::tests::{
    app, command, connected, document, form_value, hidden_named, navigation, session_id,
    test_state, text,
};
use crate::{
    conversations::{
        ConversationId, ConversationModelConfiguration, ConversationRecord, MessageStatus,
    },
    providers::{ChatBackend, ModelEvent, ModelSelection, ProviderKind, tests::ScriptedBackend},
    sessions::JobId,
    state::AppState,
};

async fn prepared_draft(
    state: &AppState,
    token: &str,
    record: &ConversationRecord,
    run: &crate::workflows::WorkflowRun,
) -> String {
    let response = app(state).oneshot(command(&format!("/conversations/{}/handoff/prepare", record.id), token, &format!("revision={}&choice=continue-prepared&run_fingerprint={}&prompt=Continue+these+exact+changes", record.revision, run.handoff_fingerprint()))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    let model = record.model.as_ref().unwrap();
    format!(
        "action=send&provider=xai&model=grok-4.6&thinking={}&environment={}&message=Continue+these+exact+changes&draft_nonce={}&prepared_run={}",
        state
            .models_dev
            .effective_effort(
                ProviderKind::Xai,
                "grok-4.6",
                model.settings.model.thinking.as_ref()
            )
            .unwrap()
            .as_str(),
        model.settings.environment,
        hidden_named(&body, "draft_nonce"),
        run.id
    )
}

#[tokio::test]
async fn prepared_handoff_needs_run_only_consent_and_rejects_stale_source_decisions() {
    let state = test_state();
    let token = connected(&state);
    let source = source(&state);
    let (run, directory) = crate::workflows::handoff::tests::prepared_run(&state, &source);
    state.workflow_runs.create(run.clone()).unwrap();
    let context_only = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/handoff/prepare", source.id),
            &token,
            &format!(
                "revision={}&choice=context-only&prompt=Use+current+host+files",
                source.revision
            ),
        ))
        .await
        .unwrap();
    assert_eq!(context_only.status(), StatusCode::OK);
    assert_eq!(state.workflow_runs.get(&run.id), Some(run.clone()));
    assert_eq!(state.conversations.get(&source.id), Some(source.clone()));
    assert_eq!(state.conversations.list().len(), 1);
    let draft = prepared_draft(&state, &token, &source, &run).await;
    assert_eq!(state.conversations.list().len(), 1);
    let response = app(&state)
        .oneshot(command("/conversations/new", &token, &draft))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(state.conversations.list().len(), 1);
    let response = app(&state)
        .oneshot(command(
            "/conversations/new",
            &token,
            &format!("{draft}&handoff_approval=continue-prepared"),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        text(response).await
    );
    let transferred = state.workflow_runs.get(&run.id).unwrap();
    let owner = transferred.conversation_id.unwrap();
    assert_ne!(owner, source.id);
    assert_eq!(transferred.source, run.source);
    assert_eq!(transferred.gates, run.gates);
    assert_eq!(transferred.artefacts, run.artefacts);
    assert_eq!(transferred.phase_models, run.phase_models);
    assert!(state.workflow_runs.for_conversation(&source.id).is_empty());
    assert_eq!(
        std::fs::read_to_string(directory.path().join("file.txt")).unwrap(),
        "original\n"
    );
    assert!(
        state
            .gate_continuations
            .available(&run.id, &session_id(&token))
    );
    let destination = state.conversations.get(&owner).unwrap();
    assert!(destination.directory_approvals.is_empty());
    assert_eq!(destination.messages.len(), 2);
    assert_eq!(destination.messages[0].text, "Continue these exact changes");
    let phase_settings = run
        .model_phases()
        .next()
        .unwrap()
        .settings
        .as_ref()
        .unwrap();
    assert!(state.access_consent.authorised_launch(
        run.id,
        session_id(&token),
        owner,
        phase_settings
    ));
    assert!(!state.access_consent.authorised_conversation(
        session_id(&token),
        owner,
        phase_settings,
        &phase_settings.directories[0]
    ));
    let gate = &run.gates[0];
    let candidate = run
        .artefact(&gate.candidate.id)
        .unwrap()
        .candidate_hash()
        .unwrap();
    let path = format!("/runs/{}/gates/{}/approve", run.id, gate.id);
    let stale = format!(
        "gate-revision={}&candidate={}&surface=conversation",
        gate.revision.get(),
        candidate.as_str()
    );
    let response = app(&state)
        .oneshot(command(&path, &token, &stale))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(state.workflow_runs.get(&run.id), Some(transferred.clone()));
    let current = format!(
        "gate-revision={}&candidate={}&surface=conversation",
        transferred.decision_revision(gate).get(),
        candidate.as_str()
    );
    let response = app(&state)
        .oneshot(command(&path, &token, &current))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        text(response).await
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !state.workflow_runs.get(&run.id).unwrap().is_terminal() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let applied = state.workflow_runs.get(&run.id).unwrap();
    assert_eq!(applied.state, crate::workflows::run::RunState::Completed);
    let persisted = tempfile::tempdir().unwrap();
    let runs = crate::workflows::WorkflowRunStore::open(persisted.path().to_owned()).unwrap();
    runs.create(applied.clone()).unwrap();
    assert_eq!(
        crate::workflows::WorkflowRunStore::open(persisted.path().to_owned())
            .unwrap()
            .get(&run.id),
        Some(applied.clone())
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("file.txt")).unwrap(),
        "prepared\n"
    );
    assert!(
        applied
            .attempts
            .iter()
            .all(|attempt| attempt.commit_result.is_none())
    );
}

#[tokio::test]
async fn expired_runtime_requires_fresh_owner_consent_without_execution_or_baseline_capture() {
    let state = test_state();
    let token = connected(&state);
    let record = source(&state);
    let (run, directory) = crate::workflows::handoff::tests::prepared_run(&state, &record);
    state.workflow_runs.create(run.clone()).unwrap();
    for (endpoint, fields) in [
        ("delete", format!("revision={}", record.revision)),
        (
            "messages",
            format!(
                "revision={}&message=Do+not+replace+the+pending+context",
                record.revision
            ),
        ),
    ] {
        let response = app(&state)
            .oneshot(command(
                &format!("/conversations/{}/{endpoint}", record.id),
                &token,
                &fields,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(state.conversations.get(&record.id), Some(record.clone()));
    }
    let path = format!("/conversations/{}/prepared/restore", record.id);
    let fields = format!(
        "revision={}&run={}&fingerprint={}",
        record.revision,
        run.id,
        run.handoff_fingerprint()
    );
    for request in [
        command(&path, &token, &fields),
        command(&path, &token, &format!("{fields}&approval=wrong")),
    ] {
        assert_eq!(
            app(&state).oneshot(request).await.unwrap().status(),
            StatusCode::CONFLICT
        );
        assert_eq!(state.workflow_runs.get(&run.id), Some(run.clone()));
    }
    let mut native = command(
        &path,
        &token,
        &format!("{fields}&approval=restore-prepared"),
    );
    native.headers_mut().remove("graft-request");
    assert_eq!(
        app(&state).oneshot(native).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    std::fs::write(directory.path().join("file.txt"), "later host edit\n").unwrap();
    let response = app(&state)
        .oneshot(command(
            &path,
            &token,
            &format!("{fields}&approval=restore-prepared"),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        text(response).await
    );
    assert_eq!(state.workflow_runs.get(&run.id), Some(run.clone()));
    assert!(
        state
            .gate_continuations
            .available(&run.id, &session_id(&token))
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("file.txt")).unwrap(),
        "later host edit\n"
    );
    assert_eq!(
        state.conversations.get(&record.id).unwrap().messages.len(),
        record.messages.len()
    );
    crate::workflows::interrupt_session_continuations(&state, session_id(&token)).unwrap();
    assert_eq!(state.workflow_runs.get(&run.id), Some(run));
}

#[tokio::test]
async fn source_edits_and_other_sessions_cannot_consume_a_prepared_draft() {
    let state = test_state();
    let token = connected(&state);
    let other = connected(&state);
    let record = source(&state);
    let (run, _directory) = crate::workflows::handoff::tests::prepared_run(&state, &record);
    state.workflow_runs.create(run.clone()).unwrap();
    let draft = prepared_draft(&state, &token, &record, &run).await;
    let request = format!("{draft}&handoff_approval=continue-prepared");
    assert_eq!(
        app(&state)
            .oneshot(command("/conversations/new", &other, &request))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    state
        .conversations
        .rename(&record.id, record.revision, "Changed context".to_owned())
        .unwrap();
    assert_eq!(
        app(&state)
            .oneshot(command("/conversations/new", &token, &request))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(state.workflow_runs.get(&run.id), Some(run));
    assert_eq!(state.conversations.list().len(), 1);
}

fn source(state: &AppState) -> ConversationRecord {
    let record = state
        .conversations
        .create_saved(
            ConversationId::generate().unwrap(),
            Some("CSV export".to_owned()),
            Some(ConversationModelConfiguration::direct(
                ModelSelection {
                    provider: ProviderKind::Xai,
                    model: "grok-4.6".to_owned(),
                    thinking: state.models_dev.effective_effort(
                        ProviderKind::Xai,
                        "grok-4.6",
                        None,
                    ),
                },
                super::super::default_environment(state).unwrap(),
            )),
            Vec::new(),
        )
        .unwrap();
    let job = JobId::generate().unwrap();
    state
        .conversations
        .begin_message_with_model(
            &record.id,
            record.revision,
            None,
            job,
            "Add CSV export. A secret appeared in the context: test-key".to_owned(),
        )
        .unwrap();
    state
        .conversations
        .settle_message(
            &record.id,
            job,
            "The export still needs tests.".to_owned(),
            MessageStatus::Complete,
            None,
        )
        .unwrap();
    state.conversations.get(&record.id).unwrap()
}

#[tokio::test]
async fn handoff_navigation_is_read_only_and_commands_require_patches() {
    let state = test_state();
    let token = connected(&state);
    let record = source(&state);
    let path = format!("/conversations/{}/handoff", record.id);
    for request in [document(&path, &token), navigation(&path, &token)] {
        assert_eq!(
            app(&state).oneshot(request).await.unwrap().status(),
            StatusCode::OK
        );
    }
    let mut targeted = document(&path, &token);
    targeted
        .headers_mut()
        .insert("graft-request", "patch".parse().unwrap());
    assert_eq!(
        app(&state).oneshot(targeted).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    for endpoint in ["generate", "prepare"] {
        let mut request = command(
            &format!("{path}/{endpoint}"),
            &token,
            &format!("revision={}", record.revision),
        );
        request.headers_mut().remove("graft-request");
        assert_eq!(
            app(&state).oneshot(request).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(state.conversations.get(&record.id), Some(record));
    assert_eq!(state.conversations.list().len(), 1);
}

#[tokio::test]
async fn generation_redacts_secrets_and_cannot_execute_tools_or_create_records() {
    let mut state = test_state();
    let token = connected(&state);
    let record = source(&state);
    let backend = ScriptedBackend::chunks([Ok(
        "Continue the export. test-key </textarea><script>bad()</script>".to_owned(),
    )]);
    state.chat = Arc::new(ChatBackend::Scripted(backend.clone()));
    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/handoff/generate", record.id),
            &token,
            &format!("revision={}&focus=Keep+the+test+failure", record.revision),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(!body.contains("test-key"));
    assert!(!body.contains("<script>bad()"));
    assert!(backend.last_tools().is_empty());
    assert!(!backend.last_history()[0].text.contains("test-key"));
    assert!(
        backend.last_history()[0]
            .text
            .contains("Keep the test failure")
    );
    assert_eq!(state.conversations.get(&record.id), Some(record.clone()));
    assert_eq!(state.conversations.list().len(), 1);
    assert!(state.workflow_runs.for_conversation(&record.id).is_empty());
    assert!(!state.sessions.busy(&session_id(&token)));
}

#[tokio::test]
async fn edited_prompt_opens_an_unsent_draft_without_consent_or_history() {
    let state = test_state();
    let token = connected(&state);
    let record = source(&state);
    let directory = tempfile::tempdir().unwrap();
    let mut grant = crate::execution::DirectoryGrant::from_selected(directory.path(), &[]).unwrap();
    grant.access = crate::execution::DirectoryAccess::DirectWrite;
    let mut settings = record.model.as_ref().unwrap().settings.clone();
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
    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/handoff/prepare", record.id),
            &token,
            &format!(
                "revision={}&prompt={}",
                record.revision,
                form_value("My edited prompt. Keep the active filters.")
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("location=\"/conversations/new\""));
    assert!(body.contains("My edited prompt. Keep the active filters."));
    assert!(body.contains("Pending approval"));
    assert!(!body.contains("A secret appeared in the context"));
    assert_eq!(state.conversations.get(&record.id), Some(record));
    assert_eq!(state.conversations.list().len(), 1);
    assert!(!state.sessions.busy(&session_id(&token)));
}

#[tokio::test]
async fn stale_sources_and_unbounded_prompts_do_not_create_drafts_or_call_models() {
    let mut state = test_state();
    let token = connected(&state);
    let record = source(&state);
    let backend = ScriptedBackend::accept();
    state.chat = Arc::new(ChatBackend::Scripted(backend.clone()));
    for (endpoint, fields, expected) in [
        ("generate", "revision=0".to_owned(), StatusCode::CONFLICT),
        (
            "prepare",
            "revision=0&prompt=Continue".to_owned(),
            StatusCode::CONFLICT,
        ),
        (
            "prepare",
            format!("revision={}&prompt=", record.revision),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "prepare",
            format!(
                "revision={}&prompt={}",
                record.revision,
                "x".repeat(crate::conversations::MAXIMUM_MESSAGE_BYTES + 1)
            ),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "prepare",
            format!(
                "revision={}&prompt={}",
                record.revision,
                "test-key".repeat(crate::conversations::MAXIMUM_MESSAGE_BYTES / 8)
            ),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "generate",
            format!("revision={}&focus=%00", record.revision),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let response = app(&state)
            .oneshot(command(
                &format!("/conversations/{}/handoff/{endpoint}", record.id),
                &token,
                &fields,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert!(
            !text(response)
                .await
                .contains("location=\"/conversations/new\"")
        );
    }
    assert!(backend.last_history().is_empty());
    assert_eq!(state.conversations.get(&record.id), Some(record));
}

#[tokio::test]
async fn an_aborted_generation_releases_its_conversation_reservation() {
    let mut state = test_state();
    let token = connected(&state);
    let record = source(&state);
    let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    state.chat = Arc::new(ChatBackend::Scripted(ScriptedBackend::hang_watched(
        started.clone(),
        dropped.clone(),
    )));
    let request = command(
        &format!("/conversations/{}/handoff/generate", record.id),
        &token,
        &format!("revision={}", record.revision),
    );
    let router = app(&state);
    let running = tokio::spawn(async move { router.oneshot(request).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !started.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        state
            .sessions
            .command_reserved(&session_id(&token), &record.id)
    );
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    assert!(
        !state
            .sessions
            .command_reserved(&session_id(&token), &record.id)
    );
    assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(state.conversations.get(&record.id), Some(record));
}

#[tokio::test]
async fn generated_prompts_obey_the_message_byte_limit() {
    for (length, expected) in [
        (crate::conversations::MAXIMUM_MESSAGE_BYTES, StatusCode::OK),
        (
            crate::conversations::MAXIMUM_MESSAGE_BYTES + 1,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let mut state = test_state();
        let token = connected(&state);
        let record = source(&state);
        state.chat = Arc::new(ChatBackend::Scripted(ScriptedBackend::chunks([Ok(
            "x".repeat(length)
        )])));
        let response = app(&state)
            .oneshot(command(
                &format!("/conversations/{}/handoff/generate", record.id),
                &token,
                &format!("revision={}", record.revision),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(state.conversations.get(&record.id), Some(record));
        assert!(!state.sessions.busy(&session_id(&token)));
    }
}

#[tokio::test]
async fn model_tool_calls_are_rejected_and_release_the_command_reservation() {
    let mut state = test_state();
    let token = connected(&state);
    let record = source(&state);
    state.chat = Arc::new(ChatBackend::Scripted(ScriptedBackend::events(vec![Ok(
        ModelEvent::ToolCall {
            id: "call".to_owned(),
            name: "run".to_owned(),
            arguments: serde_json::json!({"command": "touch forbidden"}),
        },
    )])));
    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/handoff/generate", record.id),
            &token,
            &format!("revision={}", record.revision),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!state.sessions.busy(&session_id(&token)));
    assert_eq!(state.conversations.get(&record.id), Some(record));
}
