use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use hypergraft::live::{DecodedLivePatch, HarnessSession, LiveHarness};
use std::time::Duration;
use tower::ServiceExt;

use crate::{
    sessions::{JobStatus, LiveSessionGuard, ResolvedSession, SessionId},
    state::AppState,
};

use crate::slices::conversations::tests::{
    app, awaiting_gate, connected, document, navigation, session_id, test_state, text,
};

#[tokio::test]
async fn recent_projection_is_bounded_escaped_and_uses_the_canonical_catalogue_route() {
    let state = test_state();
    let token = connected(&state);
    for _ in 0..13 {
        state
            .conversations
            .create_saved(
                crate::conversations::ConversationId::generate().expect("id"),
                Some("<script>title</script>".to_owned()),
                None,
                vec![],
            )
            .expect("record");
    }
    let request = Request::builder()
        .uri("/conversations?index=true")
        .header(header::COOKIE, format!("frinkworks_session={token}"))
        .header("Graft-Request", "patch")
        .header(header::ACCEPT, "text/vnd.hypergraft.patches+html")
        .body(Body::empty())
        .expect("request");
    let response = app(&state).oneshot(request).await.expect("projection");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("target=\"recent-conversations\""));
    assert!(body.contains("target=\"attention-count\""));
    assert_eq!(body.matches("data-recent-conversation").count(), 12);
    assert!(!body.contains("<script>title</script>"));
    for request in [
        document("/conversations?index=true", &token),
        navigation("/conversations?index=true", &token),
    ] {
        let response = app(&state).oneshot(request).await.expect("canonical page");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            text(response)
                .await
                .contains("data-section=\"conversations\"")
        );
    }
}

#[tokio::test]
async fn sidebar_projection_counts_the_authoritative_gate() {
    let state = test_state();
    let token = connected(&state);
    awaiting_gate(&state);
    let request = Request::builder()
        .uri("/conversations?index=true")
        .header(header::COOKIE, format!("frinkworks_session={token}"))
        .header("Graft-Request", "patch")
        .header(header::ACCEPT, "text/vnd.hypergraft.patches+html")
        .body(Body::empty())
        .expect("request");
    let response = app(&state).oneshot(request).await.expect("projection");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("target=\"attention-count\""));
    assert!(body.contains(">1</template>"));
    let response = app(&state)
        .oneshot(document("/conversations", &token))
        .await
        .expect("catalogue");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("id=\"attention-count\""));
    assert!(body.contains("id=\"recent-conversations-projection\""));
    assert_eq!(state.workflow_runs.active_runs().len(), 1);
}

async fn subscribe(state: &AppState, session: SessionId) -> HarnessSession<LiveSessionGuard> {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(ResolvedSession::Present(session));
    LiveHarness::new(super::super::live_router(), state.clone())
        .subscribe_with(
            "/conversations?index=true",
            LiveSessionGuard::new(state.sessions.clone()),
            extensions,
        )
        .await
        .expect("sidebar subscription")
}

async fn next(subscription: &mut HarnessSession<LiveSessionGuard>) -> DecodedLivePatch {
    tokio::time::timeout(Duration::from_secs(1), subscription.next_patch())
        .await
        .expect("store change must reach the sidebar without a timer")
        .expect("live patch")
}

fn target<'a>(patch: &'a DecodedLivePatch, id: &str) -> &'a str {
    assert_eq!(patch.targets.len(), 2);
    &patch
        .targets
        .iter()
        .find(|patch| patch.target == id)
        .unwrap()
        .html
}

#[tokio::test]
async fn live_sidebar_is_idle_until_a_change_and_reconnects_with_current_truth() {
    let state = test_state();
    let session = session_id(&connected(&state));
    let mut live = subscribe(&state, session).await;
    assert_eq!(target(live.first_patch(), "attention-count"), "");
    assert!(
        !target(live.first_patch(), "recent-conversations").contains("data-recent-conversation")
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(2200), live.next_patch())
            .await
            .is_err()
    );

    let record = state.conversations.create("First title".into()).unwrap();
    assert!(target(&next(&mut live).await, "recent-conversations").contains("First title"));
    let record = state
        .conversations
        .rename(&record.id, record.revision, "Renamed".into())
        .unwrap();
    assert!(target(&next(&mut live).await, "recent-conversations").contains("Renamed"));
    state
        .conversations
        .save_automatic_title(&record.id, record.revision, "Automatic title".into())
        .unwrap();
    assert!(target(&next(&mut live).await, "recent-conversations").contains("Automatic title"));

    // Overflow the broadcast buffer before its listener can run. The next patch must use current truth.
    let mut record = record;
    for index in 0..32 {
        record = state
            .conversations
            .rename(&record.id, record.revision, format!("Title {index}"))
            .unwrap();
    }
    assert!(target(&next(&mut live).await, "recent-conversations").contains("Title 31"));
    drop(live);
    let record = state
        .conversations
        .rename(&record.id, record.revision, "While disconnected".into())
        .unwrap();
    let mut live = subscribe(&state, session).await;
    assert!(target(live.first_patch(), "recent-conversations").contains("While disconnected"));
    state
        .conversations
        .delete(&record.id, record.revision)
        .unwrap();
    assert!(
        !target(&next(&mut live).await, "recent-conversations")
            .contains("data-recent-conversation")
    );
}

#[tokio::test]
async fn live_sidebar_tracks_attention_and_job_status_without_reply_updates() {
    use crate::conversations::{PendingQuestion, QuestionAnswer};
    use crate::execution::{HostCommandDecision, HostCommandRequest};

    let state = test_state();
    let session = session_id(&connected(&state));
    let record = state
        .conversations
        .create("Active conversation".into())
        .unwrap();
    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .unwrap();
    let record = state
        .conversations
        .begin_message(
            &record.id,
            record.revision,
            crate::providers::ModelSelection::new(
                crate::providers::ProviderKind::Xai,
                "model".into(),
                None,
            )
            .unwrap(),
            job.id(),
            "A message".into(),
        )
        .unwrap();
    let mut live = subscribe(&state, session).await;
    assert!(target(live.first_patch(), "recent-conversations").contains("In progress"));

    state
        .conversations
        .append_output(&record.id, job.id(), "Partial reply")
        .unwrap();
    state
        .conversations
        .checkpoint_output(&record.id, job.id(), "Longer partial reply")
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), live.next_patch())
            .await
            .is_err()
    );

    let question = PendingQuestion {
        conversation: record.id,
        job: job.id(),
        session,
        tool_call: "question".into(),
        prompt: "Which directory?".into(),
        options: vec![],
        allow_free_text: true,
    };
    state
        .conversations
        .submit_question(question.clone())
        .unwrap();
    let patch = next(&mut live).await;
    assert_eq!(target(&patch, "attention-count"), "1");
    assert!(target(&patch, "recent-conversations").contains("Needs an answer"));
    state
        .conversations
        .answer_question(record.id, &job, "question", QuestionAnswer::Cancelled)
        .unwrap();
    assert_eq!(target(&next(&mut live).await, "attention-count"), "");
    state
        .conversations
        .wait_question(record.id, &job, "question")
        .await
        .unwrap();
    state.conversations.submit_question(question).unwrap();
    assert_eq!(target(&next(&mut live).await, "attention-count"), "1");
    state.conversations.retain_question_sessions(|_| false);
    assert_eq!(target(&next(&mut live).await, "attention-count"), "");

    let request = HostCommandRequest {
        token: String::new(),
        session,
        job: job.id(),
        conversation: record.id,
        execution_revision: record.revision,
        command: "pwd".into(),
        directory: "/tmp".into(),
        explanation: String::new(),
        run: None,
        step: None,
        attempt: None,
    };
    let token = state.host_approvals.submit(request.clone()).unwrap();
    let patch = next(&mut live).await;
    assert_eq!(target(&patch, "attention-count"), "1");
    assert!(target(&patch, "recent-conversations").contains("Needs command approval"));
    let pending = state
        .host_approvals
        .pending_for(record.id, job.id())
        .unwrap();
    state
        .host_approvals
        .decide(&pending, HostCommandDecision::Rejected)
        .unwrap();
    assert_eq!(target(&next(&mut live).await, "attention-count"), "");
    state.host_approvals.wait(&token, &job).await.unwrap();
    state.host_approvals.submit(request).unwrap();
    assert_eq!(target(&next(&mut live).await, "attention-count"), "1");
    state.host_approvals.invalidate_job(job.id());
    assert_eq!(target(&next(&mut live).await, "attention-count"), "");

    job.finish(JobStatus::Failed, Some("Reply failed"));
    assert!(target(&next(&mut live).await, "recent-conversations").contains("Needs recovery"));
    state
        .conversations
        .settle_message(
            &record.id,
            job.id(),
            "",
            crate::conversations::MessageStatus::Failed,
            Some("Reply failed".into()),
        )
        .unwrap();
    assert!(target(&next(&mut live).await, "recent-conversations").contains("Response failed"));
}

#[tokio::test]
async fn live_sidebar_tracks_workflow_gate_changes() {
    let state = test_state();
    let session = session_id(&connected(&state));
    let record = state
        .conversations
        .create_saved(
            crate::conversations::ConversationId::generate().unwrap(),
            Some("Workflow conversation".into()),
            Some(crate::conversations::ConversationModelConfiguration {
                settings: crate::workflows::tests::settings(),
                preset: None,
            }),
            vec![],
        )
        .unwrap();
    let (run, directory) = crate::workflows::handoff::tests::prepared_run(&state, &record);
    state.keep_temp_dir(directory);
    let mut live = subscribe(&state, session).await;
    state.workflow_runs.create(run.clone()).unwrap();
    let patch = next(&mut live).await;
    assert_eq!(target(&patch, "attention-count"), "1");
    assert!(target(&patch, "recent-conversations").contains("Needs your review"));
    let gate = run.gates.last().unwrap();
    state
        .workflow_runs
        .mutate(&run.id, |run| {
            run.cancel_gate(gate.id, gate.revision, gate.opened_at_ms + 1)
        })
        .unwrap();
    let patch = next(&mut live).await;
    assert_eq!(target(&patch, "attention-count"), "");
    assert!(target(&patch, "recent-conversations").contains("Cancelled"));
}
