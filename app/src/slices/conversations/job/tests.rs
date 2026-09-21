use crate::{
    conversations::{ConversationMessage, ConversationRecord, MessageRole, MessageStatus},
    sessions::JobId,
};

fn history(record: &ConversationRecord) -> Vec<crate::providers::ChatTurn> {
    crate::conversations::history::project(&record.messages, None).expect("valid history")
}
use crate::{
    config::RuntimeConfig,
    providers::{
        ChatBackend, ModelSelection, ProviderConnection, ProviderError, ProviderKind,
        tests::ScriptedBackend,
    },
    sessions::{JobStatus, generate_session_token},
};
use std::sync::Arc;

#[tokio::test]
async fn untrusted_activity_keeps_its_order_and_stays_secret_safe_in_each_representation() {
    use crate::providers::{AssistantActivity, ModelEvent};
    let mut state = crate::tests::test_state(RuntimeConfig::development());
    state.chat = Arc::new(ChatBackend::Scripted(ScriptedBackend::events(vec![
        Ok(ModelEvent::Text("First response.".to_owned())),
        Ok(ModelEvent::Thinking("Thought test-".to_owned())),
        Ok(ModelEvent::Thinking(
            "key <script>alert(1)</script>".to_owned(),
        )),
        Ok(ModelEvent::Text("Next response.".to_owned())),
        Ok(ModelEvent::ToolCall {
            id: "call-1".to_owned(),
            name: "<script>untrusted</script>".to_owned(),
            arguments: serde_json::json!({}),
        }),
    ])));
    let token = generate_session_token().unwrap();
    state.sessions.insert(token.id());
    let connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    state.vault.put(connection.clone()).unwrap();
    let record = state.conversations.create("Activity".to_owned()).unwrap();
    let job = state
        .sessions
        .begin_conversation_job(&token.id(), record.id)
        .unwrap();
    let record = state
        .conversations
        .begin_message_with_model(
            &record.id,
            record.revision,
            None,
            job.id(),
            "Question".to_owned(),
        )
        .unwrap();
    super::run(
        state.clone(),
        token.id(),
        record.id,
        record.clone(),
        connection,
        job.clone(),
    )
    .await;
    let saved = state.conversations.get(&record.id).unwrap();
    let message = &saved.messages[1];
    assert_eq!(message.status, MessageStatus::Failed);
    assert!(matches!(
        message.activity.as_slice(),
        [
            AssistantActivity::Response(_),
            AssistantActivity::Thinking(_),
            AssistantActivity::Response(_),
            AssistantActivity::ToolCall { result: None, .. }
        ]
    ));
    assert_eq!(message.activity, job.snapshot().output.activity);
    assert_eq!(
        crate::conversations::history::project(&saved.messages, None),
        Err(crate::conversations::history::HistoryError::Unsettled)
    );
    for body in [
        super::super::page::message_view(&record.id, message).html,
        String::from_utf8(
            super::progress_frame(
                &record.id,
                &job.id().as_hex(),
                message.id,
                job.latest_seq(),
                &job.snapshot().output,
                false,
                &mut hypergraft::StreamBudget::new(),
            )
            .unwrap()
            .into_bytes(),
        )
        .unwrap(),
        String::from_utf8(
            super::final_frame(&state, &record.id, token.id(), &job, job.latest_seq()).into_bytes(),
        )
        .unwrap(),
    ] {
        assert!(!body.contains("test-key"));
        assert!(!body.contains("<script>alert(1)</script>"));
        assert!(!body.contains("<script>untrusted</script>"));
        assert!(body.contains("data-thinking-content"));
        assert!(body.find("First response.").unwrap() < body.find("Thought [redacted]").unwrap());
        assert!(body.find("Thought [redacted]").unwrap() < body.find("Next response.").unwrap());
        assert!(body.contains("Tool call"));
    }
}

#[tokio::test]
async fn conversation_instructions_apply_before_and_after_the_first_exchange() {
    let mut state = crate::tests::test_state(RuntimeConfig::development());
    let backend = ScriptedBackend::chunks([Ok("Reply".to_owned())]);
    state.chat = Arc::new(ChatBackend::Scripted(backend.clone()));
    let token = generate_session_token().unwrap();
    state.sessions.insert(token.id());
    let connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    state.vault.put(connection.clone()).unwrap();
    let record = state
        .conversations
        .create("Discussion".to_owned())
        .expect("conversation");
    let settings = crate::execution::ExecutionSettings::new(
        ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None).unwrap(),
        "Answer from the supplied evidence.".to_owned(),
        Vec::new(),
        crate::tests::test_environment_id(),
    )
    .unwrap();
    let mut record = state
        .conversations
        .update_execution_settings(&record.id, record.revision, settings)
        .unwrap();
    for message in ["First question", "Second question"] {
        let job = state
            .sessions
            .begin_conversation_job(&token.id(), record.id)
            .unwrap();
        record = state
            .conversations
            .begin_message_with_model(
                &record.id,
                record.revision,
                None,
                job.id(),
                message.to_owned(),
            )
            .unwrap();
        super::run(
            state.clone(),
            token.id(),
            record.id,
            record.clone(),
            connection.clone(),
            job,
        )
        .await;
        record = state.conversations.get(&record.id).unwrap();
        assert!(
            backend
                .last_preamble()
                .unwrap()
                .starts_with("Answer from the supplied evidence.")
        );
        assert!(record.active_job.is_none());
    }
}

#[tokio::test]
async fn bounded_partial_reply_settles_and_observation_restores_commands() {
    let mut state = crate::tests::test_state(RuntimeConfig::development());
    let backend =
        ScriptedBackend::chunks([Ok("Partial reply".to_owned()), Ok("x".repeat(128 * 1024))]);
    state.chat = Arc::new(ChatBackend::Scripted(backend.clone()));
    let token = generate_session_token().expect("session");
    state.sessions.insert(token.id());
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::ACCEPT_LANGUAGE,
        "en-US".parse().unwrap(),
    );
    let language = crate::sessions::BrowserLanguage::from_headers(&headers).unwrap();
    state.sessions.set_language(&token.id(), language.clone());
    let connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    state.vault.put(connection.clone()).expect("provider");
    let record = state
        .conversations
        .create("Discussion".to_owned())
        .expect("conversation");
    let job = state
        .sessions
        .begin_conversation_job(&token.id(), record.id)
        .expect("job");
    let selection =
        ModelSelection::new(connection.kind, connection.model.clone(), None).expect("selection");
    let record = state
        .conversations
        .begin_message(
            &record.id,
            record.revision,
            selection,
            job.id(),
            "Question".to_owned(),
        )
        .expect("begin");
    super::run(
        state.clone(),
        token.id(),
        record.id,
        record.clone(),
        connection,
        job.clone(),
    )
    .await;
    let saved = state.conversations.get(&record.id).expect("saved");
    assert!(saved.messages[1].text.starts_with("Partial reply"));
    assert_eq!(saved.messages[1].status, MessageStatus::Failed);
    assert_eq!(
        saved.messages[1].completion,
        Some(crate::providers::CompletionReason::Length)
    );
    assert!(saved.active_job.is_none());
    assert!(!state.sessions.busy(&token.id()));
    assert_eq!(backend.last_tools(), vec![crate::tools::ASK_USER]);
    let mut instructions = super::instructions(&state, &record);
    language.append_instructions(&mut instructions);
    assert_eq!(
        backend.last_preamble().as_deref(),
        Some(instructions.as_str())
    );
    let frame = super::final_frame(&state, &record.id, token.id(), &job, job.latest_seq());
    let body = String::from_utf8(frame.into_bytes()).expect("frame");
    assert!(body.contains("target=\"conversation-detail\""));
    assert!(
        body.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains(&format!("name=\"revision\" value=\"{}\"", saved.revision))
    );
    assert!(!body.contains("data-observe-active"));
}

#[tokio::test]
async fn cancellation_and_stale_settlement_cannot_release_another_command() {
    let state = crate::tests::test_state(RuntimeConfig::development());
    let token = generate_session_token().expect("session");
    let other = generate_session_token().expect("other session");
    state.sessions.insert(token.id());
    state.sessions.insert(other.id());
    let record = state
        .conversations
        .create("Discussion".to_owned())
        .expect("conversation");
    let another = state
        .conversations
        .create("Another".to_owned())
        .expect("conversation");
    let job = state
        .sessions
        .begin_conversation_job(&token.id(), record.id)
        .expect("job");
    let concurrent = state
        .sessions
        .begin_conversation_job(&token.id(), another.id)
        .expect("concurrent job");
    assert!(!state.sessions.busy(&token.id()));
    assert!(state.sessions.conversation_reserved(record.id));
    assert!(state.sessions.conversation_reserved(another.id));
    assert!(
        state
            .sessions
            .begin_conversation_job(&other.id(), record.id)
            .is_err()
    );
    assert!(
        !state
            .sessions
            .finish_conversation_job(&other.id(), record.id, job.id())
    );
    assert!(
        state
            .sessions
            .finish_conversation_job(&token.id(), another.id, concurrent.id())
    );
    let connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    let selection =
        ModelSelection::new(connection.kind, connection.model.clone(), None).expect("selection");
    let record = state
        .conversations
        .begin_message(
            &record.id,
            record.revision,
            selection,
            job.id(),
            "Question".to_owned(),
        )
        .expect("begin");
    job.request_cancel();
    super::run(
        state.clone(),
        token.id(),
        record.id,
        record.clone(),
        connection,
        job.clone(),
    )
    .await;
    assert_eq!(job.snapshot().status, JobStatus::Cancelled);
    assert_eq!(
        state
            .conversations
            .get(&record.id)
            .expect("record")
            .messages[1]
            .status,
        MessageStatus::Interrupted
    );
    let next = state
        .sessions
        .begin_conversation_job(&token.id(), another.id)
        .expect("next job");
    assert!(
        !state
            .sessions
            .finish_conversation_job(&token.id(), record.id, job.id())
    );
    assert!(!state.sessions.busy(&token.id()));
    assert!(state.sessions.conversation_reserved(another.id));
    state.sessions.remove(&token.id());
    assert!(next.cancel_requested());
}

#[tokio::test]
async fn provider_failure_retains_partial_output_and_safe_error_details() {
    let mut state = crate::tests::test_state(RuntimeConfig::development());
    let dir = tempfile::tempdir().expect("directory");
    state.conversations = Arc::new(
        crate::conversations::ConversationStore::open(dir.path().to_path_buf()).expect("store"),
    );
    state.chat = Arc::new(ChatBackend::Scripted(ScriptedBackend::chunks([
        Ok("Partial".to_owned()),
        Err(ProviderError::Detail(
            "Request test-key failed: <script>alert(1)</script>\n".to_owned(),
        )),
    ])));
    let token = generate_session_token().expect("session");
    state.sessions.insert(token.id());
    let record = state
        .conversations
        .create("Discussion".to_owned())
        .expect("conversation");
    let job = state
        .sessions
        .begin_conversation_job(&token.id(), record.id)
        .expect("job");
    let connection = ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6");
    let selection =
        ModelSelection::new(connection.kind, connection.model.clone(), None).expect("model");
    let record = state
        .conversations
        .begin_message(
            &record.id,
            record.revision,
            selection,
            job.id(),
            "Question".to_owned(),
        )
        .expect("begin");
    super::run(
        state.clone(),
        token.id(),
        record.id,
        record.clone(),
        connection,
        job.clone(),
    )
    .await;
    let record = state.conversations.get(&record.id).expect("record");
    assert_eq!(record.messages[1].text, "Partial");
    assert_eq!(record.messages[1].status, MessageStatus::Failed);
    assert_eq!(
        record.messages[1].error.as_deref(),
        Some("Request [redacted] failed: <script>alert(1)</script>")
    );
    assert!(record.active_job.is_none());
    assert!(!state.sessions.busy(&token.id()));
    assert_eq!(history(&record).last().unwrap().text, "Question");
    let frame = super::final_frame(&state, &record.id, token.id(), &job, job.latest_seq());
    let body = String::from_utf8(frame.into_bytes()).expect("frame");
    assert!(body.contains("[redacted]"));
    assert!(!body.contains("test-key"));
    assert!(!body.contains("<script>"));
    assert!(body.contains("&#60;script&#62;"));
    let reopened =
        crate::conversations::ConversationStore::open(dir.path().to_path_buf()).expect("reopened");
    assert_eq!(reopened.get(&record.id).unwrap().messages, record.messages);
    let bytes = std::fs::read_to_string(dir.path().join(format!("{}.json", record.id.as_hex())))
        .expect("stored record");
    assert!(!bytes.contains("test-key"));
}

#[test]
fn pending_assistant_output_stays_out_of_the_next_request_history() {
    let request = JobId::generate().expect("request");
    let record = ConversationRecord {
        id: crate::conversations::ConversationId::generate().expect("conversation"),
        revision: 1,
        title: "Discussion".to_owned(),
        title_pending: false,
        network: crate::agents::NetworkAccess::None,
        model: None,
        directory_approvals: Vec::new(),

        source_candidate_review: None,
        candidate_reviews: Vec::new(),
        candidate_review_context: None,
        continuation: None,
        compaction: None,
        summary_requests: Vec::new(),
        messages: vec![
            ConversationMessage {
                id: crate::conversations::MessageId::generate().expect("message id"),
                continuation: Vec::new(),
                role: MessageRole::User,
                text: "First question".to_owned(),
                activity: Vec::new(),
                status: MessageStatus::Complete,
                error: None,
                request: None,
                completion: None,
                requests: Vec::new(),
            },
            ConversationMessage {
                id: crate::conversations::MessageId::generate().expect("message id"),
                continuation: Vec::new(),
                role: MessageRole::Assistant,
                text: "First reply".to_owned(),
                activity: Vec::new(),
                status: MessageStatus::Complete,
                error: None,
                request: Some(JobId::generate().expect("previous request")),
                completion: None,
                requests: Vec::new(),
            },
            ConversationMessage {
                id: crate::conversations::MessageId::generate().expect("message id"),
                continuation: Vec::new(),
                role: MessageRole::User,
                text: "Second question".to_owned(),
                activity: Vec::new(),
                status: MessageStatus::Complete,
                error: None,
                request: None,
                completion: None,
                requests: Vec::new(),
            },
            ConversationMessage {
                id: crate::conversations::MessageId::generate().expect("message id"),
                continuation: Vec::new(),
                role: MessageRole::Assistant,
                text: String::new(),
                activity: Vec::new(),
                status: MessageStatus::Pending,
                error: None,
                request: Some(request),
                completion: None,
                requests: Vec::new(),
            },
        ],
        active_job: Some(request),
        queue: crate::conversations::ConversationQueue::default(),
        created_at_ms: 0,
        updated_at_ms: 0,
    };

    let history = history(&record);
    assert_eq!(history.len(), 3);
    assert_eq!(history.last().expect("last turn").text, "Second question");
}
