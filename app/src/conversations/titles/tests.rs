use super::*;

#[tokio::test]
async fn title_request_excludes_tools_presets_and_later_history() {
    use crate::{
        config::RuntimeConfig,
        providers::{ChatBackend, ModelSelection, ProviderKind, tests::ScriptedBackend},
        sessions::JobId,
    };
    let mut state = crate::tests::test_state(RuntimeConfig::development());
    let backend = ScriptedBackend::chunks([Ok("Parser repair".to_owned())]);
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(backend.clone()));
    let connection =
        ProviderConnection::with_key(ProviderKind::Deepseek, "private-key", "title-model");
    let initial = state.conversations.create_untitled(None).unwrap();
    let job = JobId::generate().unwrap();
    state
        .conversations
        .begin_message(
            &initial.id,
            initial.revision,
            ModelSelection::new(
                ProviderKind::Deepseek,
                "conversation-model".to_owned(),
                None,
            )
            .unwrap(),
            job,
            "Fix the parser".to_owned(),
        )
        .unwrap();
    state
        .conversations
        .settle_message(
            &initial.id,
            job,
            "Reply".repeat(1000),
            super::super::MessageStatus::Complete,
            None,
        )
        .unwrap();
    let mut record = state.conversations.claim_title(&initial.id).unwrap();
    record.model.as_mut().unwrap().settings.instructions = "Private preset instructions".to_owned();
    record.messages.push(super::super::ConversationMessage {
        role: super::super::MessageRole::User,
        text: "Later private message".to_owned(),
        activity: Vec::new(),
        status: super::super::MessageStatus::Complete,
        error: None,
        request: None,
    });
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::ACCEPT_LANGUAGE,
        "en-GB,en;q=0.9".parse().unwrap(),
    );
    let language = BrowserLanguage::from_headers(&headers).unwrap();
    assert_eq!(
        request_title(&state, &connection, &record, Some(&language))
            .await
            .as_deref(),
        Some("Parser repair")
    );
    assert!(backend.last_tools().is_empty());
    assert_eq!(backend.last_connection().unwrap().0, ProviderKind::Deepseek);
    assert_eq!(backend.last_connection().unwrap().1, "title-model");
    let mut instructions = INSTRUCTIONS.to_owned();
    language.append_instructions(&mut instructions);
    assert_eq!(
        backend.last_preamble().as_deref(),
        Some(instructions.as_str())
    );
    let history = backend.last_history();
    assert_eq!(history.len(), 1);
    assert!(
        history[0].text.len() + instructions.len()
            <= crate::models::models_dev::TITLE_INPUT_TOKENS as usize
    );
    assert!(!history[0].text.contains("Later private"));
    assert!(!history[0].text.contains("Private preset"));

    state.chat = std::sync::Arc::new(ChatBackend::Scripted(ScriptedBackend::tool_then(
        "run",
        serde_json::json!({}),
        "bad",
    )));
    assert!(
        request_title(&state, &connection, &record, None)
            .await
            .is_none()
    );
    state.chat = std::sync::Arc::new(ChatBackend::Scripted(ScriptedBackend::chunks([Err(
        crate::providers::ProviderError::Rejected,
    )])));
    assert!(
        request_title(&state, &connection, &record, None)
            .await
            .is_none()
    );
    assert_eq!(
        state.conversations.get(&initial.id).unwrap().title,
        "New conversation"
    );
}

#[test]
fn generated_titles_reject_untrusted_output_and_credentials() {
    for text in [
        "",
        "two\nlines",
        "<script>bad</script>",
        "`markup`",
        "contains secret-key",
    ] {
        assert!(valid_title(text, Some("secret-key")).is_none());
    }
    assert!(valid_title(&"x".repeat(121), None).is_none());
    assert_eq!(
        valid_title(" \"Fix the parser\" ", None).as_deref(),
        Some("Fix the parser")
    );
}

#[test]
fn title_normalisation_preserves_quoted_words_and_strips_only_outer_wrappers() {
    for (raw, expected) in [
        ("Greeting with “cobber”", "Greeting with “cobber”"),
        ("“Cobber” greeting", "“Cobber” greeting"),
        ("“Cobber” and “mate”", "“Cobber” and “mate”"),
        ("Greeting with \"cobber\"", "Greeting with \"cobber\""),
        ("\"Cobber\" greeting", "\"Cobber\" greeting"),
        ("\"Cobber\" and \"mate\"", "\"Cobber\" and \"mate\""),
        (" \"Parser repair\" ", "Parser repair"),
        (" “Parser repair” ", "Parser repair"),
        ("\"Greeting with “cobber”\"", "Greeting with “cobber”"),
        ("“Greeting with \"cobber\"”", "Greeting with \"cobber\""),
    ] {
        assert_eq!(valid_title(raw, None).as_deref(), Some(expected));
    }
    for raw in ["\"\"", "“”", "\" \""] {
        assert!(valid_title(raw, None).is_none());
    }
}

#[test]
fn excerpt_bounds_unicode_and_control_characters() {
    let text = excerpt(&format!("  first\nsecond\0 {}", "界".repeat(120)));
    assert!(text.len() <= MAXIMUM_TITLE_BYTES);
    assert!(!text.chars().any(char::is_control));
    assert_eq!(excerpt("\n\0"), "New conversation");
}
