use axum::http::StatusCode;
use tower::ServiceExt;

use crate::conversations::CheckpointId;
use crate::execution::{BudgetReason, BudgetSnapshot};
use crate::providers::ProviderKind;
use crate::slices::conversations::tests::{
    app, command, connected, document, session_id, test_state, text,
};

fn model_record(state: &crate::state::AppState) -> crate::conversations::ConversationRecord {
    let record = state.conversations.create("Pause".to_owned()).unwrap();
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(
            ProviderKind::Xai,
            "grok-4.6".to_owned(),
            Some(effort),
        )
        .unwrap(),
        String::new(),
        Vec::new(),
        crate::tests::test_environment_id(),
    )
    .unwrap();
    state
        .conversations
        .update_execution_settings(&record.id, record.revision, settings)
        .unwrap()
}

fn paused(
    state: &crate::state::AppState,
    token: &str,
) -> (
    crate::conversations::ConversationRecord,
    crate::conversations::CheckpointId,
) {
    let session = session_id(token);
    let record = model_record(state);
    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .unwrap();
    let record = state
        .conversations
        .begin_message_with_model(
            &record.id,
            record.revision,
            record.model.clone(),
            job.id(),
            "Keep going".to_owned(),
        )
        .unwrap();
    let checkpoint = CheckpointId::generate().unwrap();
    let boundary = record.messages[0].id;
    let stored = crate::conversations::ContinuationCheckpoint {
        id: checkpoint,
        boundary,
        pinned: record.model.as_ref().unwrap().settings.clone(),
        budget: BudgetSnapshot {
            model_requests: 2,
            model_request_limit: 2,
            tool_dispatches: 2,
            tool_dispatch_limit: 20,
            elapsed_ms: 1_000,
            elapsed_limit_ms: 1_800_000,
            reason: BudgetReason::ModelRequests,
        },
        run: None,
        attempt: None,
        step: None,
        drafts: Vec::new(),
        created_at_ms: 1,
    };
    let record = state
        .conversations
        .pause_for_budget(
            &record.id,
            job.id(),
            crate::providers::AssistantReply {
                text: "Working".to_owned(),
                ..crate::providers::AssistantReply::default()
            },
            stored,
        )
        .unwrap();
    job.finish(crate::sessions::JobStatus::Completed, None);
    state
        .sessions
        .finish_conversation_job(&session, record.id, job.id());
    (record, checkpoint)
}

fn paused_workflow(
    state: &crate::state::AppState,
    token: &str,
) -> (
    crate::conversations::ConversationRecord,
    crate::workflows::WorkflowRun,
) {
    use crate::workflows::run::{
        AttemptCleanupRecord, AttemptSandboxKind, AttemptSandboxRecord, PhaseModelSelection,
        RunKind,
    };
    let (record, checkpoint) = paused(state, token);
    let settings = record
        .model
        .as_ref()
        .unwrap()
        .settings
        .clone()
        .with_location(crate::execution::ToolLocation::Host);
    let pinned = crate::workflows::definition::PinnedWorkflowDefinition::pin(
        None,
        crate::workflows::seeds::implement_a_change_definition(settings.environment)
            .with_conversation_settings(&settings)
            .unwrap(),
    );
    let step = pinned.definition.first_step().clone();
    let environments = crate::tests::test_environment_set(&pinned.definition);
    let mut run = crate::workflows::WorkflowRun::create_source_free_for_conversation(
        crate::workflows::RunId::generate().unwrap(),
        1,
        record.id,
        pinned,
        environments,
        vec![PhaseModelSelection {
            step: step.clone(),
            selection: settings.model.clone(),
            instructions: String::new(),
            preset: None,
            settings: Some(settings.clone()),
        }],
        settings.clone(),
    );
    run.kind = RunKind::Configured;
    run.launch_brief = "Continue the assigned phase.".to_owned();
    let attempt = crate::workflows::AttemptId::generate().unwrap();
    run.start_attempt(
        attempt,
        Vec::new(),
        crate::workflows::capabilities::AttemptCapabilities::derive_project_free(
            run.pinned.definition.step(&step).unwrap(),
            &crate::execution::ProjectFreeAuthority::from_settings(record.revision, &settings)
                .unwrap(),
        )
        .unwrap(),
        AttemptSandboxRecord {
            kind: AttemptSandboxKind::HostExecution,
            snapshot_digest: crate::environments::SnapshotDigest::parse(&format!(
                "sha256:{}",
                "a".repeat(64)
            ))
            .unwrap(),
        },
        2,
    )
    .unwrap();
    run.record_cleanup(attempt, AttemptCleanupRecord::Complete)
        .unwrap();
    run.pause_attempt(attempt, checkpoint, Vec::new(), 3)
        .unwrap();
    state.workflow_runs.create(run.clone()).unwrap();
    let session = session_id(token);
    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .unwrap();
    state
        .conversations
        .claim_continuation(&record.id, record.revision, checkpoint, job.id())
        .unwrap();
    let mut stored = record.continuation.unwrap();
    stored.pinned = settings;
    stored.run = Some(run.id);
    stored.attempt = Some(attempt);
    stored.step = Some(step.as_str().to_owned());
    let record = state
        .conversations
        .pause_for_budget(&record.id, job.id(), Default::default(), stored)
        .unwrap();
    job.finish(crate::sessions::JobStatus::Completed, None);
    state
        .sessions
        .finish_conversation_job(&session, record.id, job.id());
    (record, run)
}

#[tokio::test]
async fn workflow_continuation_requires_fresh_consent_and_exact_references() {
    let state = test_state();
    let token = connected(&state);
    let (record, run) = paused_workflow(&state, &token);
    let stored = record.continuation.as_ref().unwrap();
    let path = format!("/conversations/{}/continue", record.id);
    let body = format!(
        "revision={}&checkpoint={}&run={}&attempt={}&step={}",
        record.revision,
        stored.id,
        run.id,
        stored.attempt.unwrap(),
        stored.step.as_ref().unwrap()
    );
    let missing = app(&state)
        .oneshot(command(&path, &token, &body))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let stale = body.replace(
        &stored.attempt.unwrap().as_hex(),
        &crate::workflows::AttemptId::generate().unwrap().as_hex(),
    );
    let rejected = app(&state)
        .oneshot(command(
            &path,
            &token,
            &format!("{stale}&approval=continue-run"),
        ))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    assert_eq!(state.workflow_runs.get(&run.id).unwrap().state, run.state);
    assert!(!state.access_consent.authorised_launch(
        run.id,
        session_id(&token),
        record.id,
        &stored.pinned
    ));
    let accepted = app(&state)
        .oneshot(command(
            &path,
            &token,
            &format!("{body}&approval=continue-run"),
        ))
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::OK);
    assert!(state.access_consent.authorised_launch(
        run.id,
        session_id(&token),
        record.id,
        &stored.pinned
    ));
    assert!(!state.access_consent.authorised_launch(
        crate::workflows::RunId::generate().unwrap(),
        session_id(&token),
        record.id,
        &stored.pinned
    ));
}

#[tokio::test]
async fn a_stale_end_pause_does_not_cancel_the_workflow() {
    let state = test_state();
    let token = connected(&state);
    let (record, run) = paused_workflow(&state, &token);
    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/continue/end", record.id),
            &token,
            &format!(
                "revision={}&checkpoint={}",
                record.revision - 1,
                record.continuation.as_ref().unwrap().id
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(state.workflow_runs.get(&run.id).unwrap().state, run.state);
    assert_eq!(
        state.conversations.get(&record.id).unwrap().continuation,
        record.continuation
    );
}

#[tokio::test]
async fn continue_commands_use_patch_representation() {
    let state = test_state();
    let token = connected(&state);
    let (record, checkpoint) = paused(&state, &token);
    let page = app(&state)
        .oneshot(document(&format!("/conversations/{}", record.id), &token))
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let body = text(page).await;
    assert!(body.contains("id=\"conversation-continuation\""));
    assert!(body.contains("id=\"conversation-continue\""));
    let path = format!("/conversations/{}/continue", record.id);
    let body = format!("revision={}&checkpoint={}", record.revision, checkpoint);
    let response = app(&state)
        .oneshot(command(&path, &token, &body))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        text(response).await
    );

    let mut navigation = document(&path, &token);
    *navigation.method_mut() = axum::http::Method::POST;
    let rejected = app(&state).oneshot(navigation).await.unwrap();
    assert_ne!(rejected.status(), StatusCode::OK);
}

#[tokio::test]
async fn stale_checkpoint_identity_is_rejected() {
    let state = test_state();
    let token = connected(&state);
    let (record, _) = paused(&state, &token);
    let path = format!("/conversations/{}/continue", record.id);
    let stale = format!(
        "revision={}&checkpoint={}",
        record.revision,
        CheckpointId::generate().unwrap()
    );
    let response = app(&state)
        .oneshot(command(&path, &token, &stale))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(
        state
            .conversations
            .get(&record.id)
            .unwrap()
            .continuation
            .is_some()
    );
}

#[tokio::test]
async fn tool_free_continuation_accepts_a_fresh_session() {
    let state = test_state();
    let owner = connected(&state);
    let other = connected(&state);
    let (record, checkpoint) = paused(&state, &owner);
    let path = format!("/conversations/{}/continue", record.id);
    let body = format!("revision={}&checkpoint={}", record.revision, checkpoint);
    let rejected = app(&state)
        .oneshot(command(&path, &other, &body))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::OK);
}

#[tokio::test]
async fn ordinary_checkpoint_rejects_unrelated_workflow_references() {
    let state = test_state();
    let token = connected(&state);
    let (record, checkpoint) = paused(&state, &token);
    let path = format!("/conversations/{}/continue", record.id);
    let body = format!(
        "revision={}&checkpoint={}&run=deadbeefdeadbeefdeadbeefdeadbeef&attempt=deadbeefdeadbeefdeadbeefdeadbeef&step=build",
        record.revision, checkpoint
    );
    let response = app(&state)
        .oneshot(command(&path, &token, &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let unchanged = state.conversations.get(&record.id).unwrap();
    assert_eq!(unchanged.continuation, record.continuation);
    assert_eq!(unchanged.active_job, None);
}
