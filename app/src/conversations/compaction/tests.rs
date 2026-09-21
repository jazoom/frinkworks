use crate::providers::{AssistantActivity, AuthMethod, ModelUsage, ProviderKind, ToolOutput};

use super::{CompactionError, CompactionRecord, project, select_boundary, validate_summary};
use crate::conversations::history::{
    ConversationMessage, MessageRole, MessageStatus, RequestUsage,
};
use crate::conversations::{MessageId, RequestId};

fn identifier() -> MessageId {
    MessageId::generate().expect("message id")
}

fn usage() -> RequestUsage {
    RequestUsage {
        id: RequestId::generate().expect("request"),
        usage: ModelUsage::new(ProviderKind::Xai, "grok-4.6"),
        auth: AuthMethod::ApiKey,
        prices: None,
        sources: Vec::new(),
        advertised: Vec::new(),
    }
}

fn user(text: &str) -> ConversationMessage {
    ConversationMessage {
        id: identifier(),
        role: MessageRole::User,
        text: text.to_owned(),
        activity: Vec::new(),
        continuation: Vec::new(),
        status: MessageStatus::Complete,
        error: None,
        request: None,
        completion: None,
        requests: Vec::new(),
    }
}

fn assistant(text: &str) -> ConversationMessage {
    ConversationMessage {
        id: identifier(),
        role: MessageRole::Assistant,
        text: text.to_owned(),
        activity: Vec::new(),
        continuation: Vec::new(),
        status: MessageStatus::Complete,
        error: None,
        request: None,
        completion: None,
        requests: Vec::new(),
    }
}

fn assistant_tool(id: &str, result: Option<ToolOutput>) -> ConversationMessage {
    ConversationMessage {
        id: identifier(),
        role: MessageRole::Assistant,
        text: String::new(),
        activity: vec![AssistantActivity::ToolCall {
            id: id.to_owned(),
            name: "read".to_owned(),
            arguments: serde_json::json!({"path": "main.rs"}),
            result,
        }],
        continuation: Vec::new(),
        status: MessageStatus::Complete,
        error: None,
        request: None,
        completion: None,
        requests: Vec::new(),
    }
}

fn two_exchanges() -> Vec<ConversationMessage> {
    vec![
        user("First"),
        assistant("First reply"),
        user("Second"),
        assistant("Second reply"),
    ]
}

#[test]
fn compaction_preserves_consumed_resource_provenance() {
    let source = crate::execution::ResourceSource::new(
        crate::execution::ResourceKind::Skill,
        "project",
        "/project/.pi/skills/review/SKILL.md",
        b"review instructions",
    );
    let messages = vec![
        user("Read the skill"),
        assistant_tool(
            "skill-read",
            Some(ToolOutput {
                resource: Some(source.clone()),
                label: "read".to_owned(),
                output: "review instructions".to_owned(),
                command: None,
            }),
        ),
        assistant("Skill read"),
        user("Continue"),
        assistant("Continued"),
    ];
    let turns = crate::conversations::history::project(&messages, None).expect("history");
    let sources = crate::execution::resources::consumed_sources(&[], &turns);
    assert_eq!(sources, vec![source.clone()]);
    let mut request = usage();
    request.sources = sources;
    let (covered_through, retained_from) = select_boundary(&messages, None).expect("boundary");
    let record = CompactionRecord {
        covered_through,
        retained_from,
        text: "The review skill supplied instructions.".to_owned(),
        request: serde_json::from_slice(&serde_json::to_vec(&request).expect("persist request"))
            .expect("reload request"),
        created_at_ms: 1,
    };
    let compacted = project(&messages, None, Some(&record)).expect("compacted history");
    assert_eq!(
        crate::execution::resources::consumed_sources(&[], &compacted),
        vec![source]
    );
}

#[test]
fn a_boundary_keeps_the_latest_complete_exchange() {
    let messages = two_exchanges();
    let (covered, retained) = select_boundary(&messages, None).expect("boundary");
    assert_eq!(covered, messages[1].id);
    assert_eq!(retained, messages[2].id);
}

#[test]
fn a_tool_call_cannot_split_from_its_result() {
    let messages = vec![
        user("Read"),
        assistant_tool("call-1", None),
        user("Later"),
        assistant("Done"),
    ];
    assert_eq!(
        select_boundary(&messages, None),
        Err(CompactionError::Unsettled)
    );
}

#[test]
fn one_exchange_cannot_compact() {
    let messages = vec![user("Hello"), assistant("Hi")];
    assert_eq!(
        select_boundary(&messages, None),
        Err(CompactionError::NothingToCompact)
    );
}

#[test]
fn a_malformed_summary_does_not_replace_context() {
    assert_eq!(validate_summary("", None), Err(CompactionError::Malformed));
    assert_eq!(
        validate_summary("bad\0summary", None),
        Err(CompactionError::Malformed)
    );
    assert_eq!(
        validate_summary("leak", Some("leak")),
        Err(CompactionError::Malformed)
    );
    let messages = two_exchanges();
    let record = CompactionRecord {
        covered_through: messages[1].id,
        retained_from: messages[2].id,
        text: String::new(),
        request: usage(),
        created_at_ms: 1,
    };
    assert!(!record.valid(&messages));
    assert!(project(&messages, None, Some(&record)).is_err());
}

#[test]
fn a_valid_summary_replaces_only_the_model_projection() {
    let messages = two_exchanges();
    let record = CompactionRecord {
        covered_through: messages[1].id,
        retained_from: messages[2].id,
        text: "Earlier the user asked first.".to_owned(),
        request: usage(),
        created_at_ms: 1,
    };
    assert!(record.valid(&messages));
    let history = project(&messages, None, Some(&record)).expect("projection");
    assert_eq!(history.len(), 3);
    assert!(history[0].text.ends_with("Earlier the user asked first."));
    assert_eq!(history[1].text, "Second");
    assert_eq!(history[2].text, "Second reply");
}

#[test]
fn a_missing_cover_boundary_is_rejected() {
    let messages = two_exchanges();
    let record = CompactionRecord {
        covered_through: identifier(),
        retained_from: messages[2].id,
        text: "Stale".to_owned(),
        request: usage(),
        created_at_ms: 1,
    };
    assert!(!record.valid(&messages));
    assert_eq!(
        select_boundary(&messages, Some(&record)),
        Err(CompactionError::Malformed)
    );
}

#[test]
fn a_summary_keeps_its_boundary_when_a_later_request_is_pending() {
    let mut messages = two_exchanges();
    let record = CompactionRecord {
        covered_through: messages[1].id,
        retained_from: messages[2].id,
        text: "Earlier context".to_owned(),
        request: usage(),
        created_at_ms: 1,
    };
    messages.push(user("Next"));
    let mut pending = assistant("");
    pending.status = MessageStatus::Pending;
    messages.push(pending);
    assert!(record.valid(&messages));
    let projected = project(&messages, None, Some(&record)).unwrap();
    assert_eq!(projected.last().unwrap().text, "Next");
    let mut skipped = record.clone();
    skipped.retained_from = messages[4].id;
    assert!(!skipped.valid(&messages));
}

#[test]
fn interrupted_completed_tools_do_not_shift_the_retained_suffix() {
    let result = ToolOutput {
        resource: None,
        label: "read".to_owned(),
        output: "file contents".to_owned(),
        command: None,
    };
    let mut interrupted = assistant_tool("first", Some(result));
    interrupted.status = MessageStatus::Interrupted;
    let messages = vec![
        user("Read"),
        interrupted,
        user("Explain"),
        assistant("Explanation"),
        user("Retained"),
        assistant("Latest"),
    ];
    let record = CompactionRecord {
        covered_through: messages[3].id,
        retained_from: messages[4].id,
        text: "Summary".to_owned(),
        request: usage(),
        created_at_ms: 1,
    };
    let projected = project(&messages, None, Some(&record)).unwrap();
    assert_eq!(projected.len(), 3);
    assert_eq!(projected[1].text, "Retained");
    let covered = super::covered_turns(&messages, None, messages[3].id, None).unwrap();
    assert_eq!(covered.len(), 4);
    assert_eq!(
        covered[1].calls[0].result.as_ref().unwrap().output,
        "file contents"
    );
}
