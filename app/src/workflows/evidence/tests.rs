use std::sync::Mutex;

use crate::providers::{AssistantReply, ToolOutput};

use super::*;

impl WorkflowEvidenceStore {
    pub(crate) fn in_memory() -> Self {
        Self {
            dir: None,
            inner: Mutex::new(BTreeMap::new()),
        }
    }
}

#[test]
fn host_evidence_redacts_secrets_and_rejects_path_tokens() {
    let root = tempfile::tempdir().unwrap();
    let store = WorkflowEvidenceStore::open(root.path().to_owned()).unwrap();
    let secret = "provider-secret-for-evidence";
    let mut request = crate::execution::HostCommandRequest {
        token: "a".repeat(64),
        session: crate::sessions::generate_session_token().unwrap().id(),
        conversation: crate::conversations::ConversationId::generate().unwrap(),
        job: crate::sessions::JobId::generate().unwrap(),
        execution_revision: 1,
        command: format!("printf {secret}"),
        directory: PathBuf::from(format!("/tmp/{secret}")),
        explanation: secret.to_owned(),
        run: None,
        step: None,
        attempt: None,
    };
    store
        .host_command(&request, "dispatching", secret, Some(secret), None)
        .unwrap();
    let bytes = fs::read(
        root.path()
            .join("host")
            .join(format!("{}.json", request.token)),
    )
    .unwrap();
    assert!(!String::from_utf8(bytes).unwrap().contains(secret));
    request.token = "../escape".to_owned();
    assert_eq!(
        store.host_command(&request, "dispatching", "", None, None),
        Err(EvidenceError::Corrupt)
    );
    assert!(!root.path().join("escape.json").exists());
}

fn ids() -> (RunId, AttemptId) {
    (
        RunId::generate().expect("run"),
        AttemptId::generate().expect("attempt"),
    )
}

#[test]
fn events_keep_attempt_and_phase_identity() {
    let store = WorkflowEvidenceStore::in_memory();
    let (run, attempt) = ids();
    let other_run = RunId::generate().expect("other run");

    store
        .append_activity(
            run,
            attempt,
            ActivityInput {
                phase: "implementation",
                kind: ActivityKind::Response,
                text: "candidate output",
                label: "",
                provider: None,
                input_tokens: None,
                secret: None,
            },
        )
        .expect("response");
    store
        .append_activity(
            run,
            attempt,
            ActivityInput {
                phase: "review",
                kind: ActivityKind::Thinking,
                text: "review output",
                label: "",
                provider: None,
                input_tokens: None,
                secret: None,
            },
        )
        .expect_err("phase changes must not merge workers");
    let evidence = store.get(&run, &attempt).expect("evidence");
    assert_eq!(evidence.events.len(), 1);
    assert_eq!(evidence.events[0].phase, "implementation");
    assert!(store.get(&other_run, &attempt).is_none());
}

#[test]
fn terminal_records_redact_credentials_before_persistence() {
    let dir = tempfile::tempdir().expect("directory");
    let store = WorkflowEvidenceStore::open(dir.path().to_path_buf()).expect("store");
    let (run, attempt) = ids();
    let secret = "sk-test-secret";
    let mut reply = AssistantReply::from(format!("Reply {secret}"));
    reply.thinking = format!("Thought {secret}");
    reply.tools.push(ToolOutput {
        label: "read".to_owned(),
        output: format!("Output {secret}"),
        command: None,
    });

    store
        .record_terminal(
            run,
            attempt,
            TerminalInput {
                phase: "implementation",
                state: TerminalState::Failed,
                reply: &reply,
                error: Some(&format!("Failure {secret}")),
                secret: Some(secret),
            },
        )
        .expect("terminal");
    let evidence = store.get(&run, &attempt).expect("evidence");
    let terminal = evidence.terminal.as_ref().expect("terminal result");
    let encoded = std::fs::read_to_string(evidence_path(dir.path(), run, attempt).expect("path"))
        .expect("persisted evidence");
    assert!(!encoded.contains(secret));
    assert!(terminal.text.contains("[redacted]"));
    assert!(terminal.thinking.contains("[redacted]"));
    assert!(terminal.tools[0].output.contains("[redacted]"));
    assert!(
        terminal
            .error
            .as_ref()
            .expect("error")
            .contains("[redacted]")
    );
}

#[test]
fn activity_is_bounded_and_marks_truncation() {
    let store = WorkflowEvidenceStore::in_memory();
    let (run, attempt) = ids();
    let text = "x".repeat(MAXIMUM_ACTIVITY_TEXT_BYTES + 100);

    let appended = store
        .append_activity(
            run,
            attempt,
            ActivityInput {
                phase: "implementation",
                kind: ActivityKind::Response,
                text: &text,
                label: "",
                provider: None,
                input_tokens: None,
                secret: None,
            },
        )
        .expect("activity");
    assert!(appended);
    let evidence = store.get(&run, &attempt).expect("evidence");
    assert!(evidence.activity_truncated);
    assert!(evidence.events[0].truncated);
    assert!(evidence.events[0].text.len() <= MAXIMUM_ACTIVITY_TEXT_BYTES);

    for _ in 0..MAXIMUM_ACTIVITY_EVENTS {
        let _ = store.append_activity(
            run,
            attempt,
            ActivityInput {
                phase: "implementation",
                kind: ActivityKind::Response,
                text: "event",
                label: "",
                provider: None,
                input_tokens: None,
                secret: None,
            },
        );
    }
    let evidence = store.get(&run, &attempt).expect("evidence");
    assert!(evidence.events.len() <= MAXIMUM_ACTIVITY_EVENTS);
    assert!(evidence.activity_truncated);
}

#[test]
fn open_rejects_an_oversized_record_before_deserialisation() {
    let dir = tempfile::tempdir().expect("directory");
    let (run, attempt) = ids();
    let path = evidence_path(dir.path(), run, attempt).expect("path");
    std::fs::write(path, vec![b'x'; MAXIMUM_EVIDENCE_RECORD_BYTES + 1]).expect("write");

    assert!(matches!(
        WorkflowEvidenceStore::open(dir.path().to_path_buf()),
        Err(EvidenceError::Corrupt)
    ));
}

#[test]
fn reopening_preserves_bounded_evidence() {
    let dir = tempfile::tempdir().expect("directory");
    let store = WorkflowEvidenceStore::open(dir.path().to_path_buf()).expect("open");
    let (run, attempt) = ids();
    store
        .append_activity(
            run,
            attempt,
            ActivityInput {
                phase: "review",
                kind: ActivityKind::Usage,
                text: "",
                label: "",
                provider: Some("xai"),
                input_tokens: Some(42),
                secret: None,
            },
        )
        .expect("usage");
    let reopened = WorkflowEvidenceStore::open(dir.path().to_path_buf()).expect("reopen");
    let evidence = reopened.get(&run, &attempt).expect("evidence");
    assert_eq!(evidence.events[0].phase, "review");
    assert_eq!(evidence.events[0].input_tokens, Some(42));
}

#[test]
fn a_full_store_rejects_new_attempts_but_keeps_existing_attempts_writable() {
    let store = WorkflowEvidenceStore::in_memory();
    let (run, attempt) = ids();
    {
        let mut records = store.lock();
        records.insert((run, attempt), empty_record(run, attempt, "review"));
        for _ in 1..MAXIMUM_EVIDENCE_RECORDS {
            let (run, attempt) = ids();
            records.insert((run, attempt), empty_record(run, attempt, "review"));
        }
    }
    let reply = AssistantReply::from("Finished");
    let input = || TerminalInput {
        phase: "review",
        state: TerminalState::Completed,
        reply: &reply,
        error: None,
        secret: None,
    };
    let (other_run, other_attempt) = ids();
    assert_eq!(
        store.record_terminal(other_run, other_attempt, input()),
        Err(EvidenceError::Full)
    );
    assert_eq!(
        store.append_activity(
            other_run,
            other_attempt,
            ActivityInput {
                phase: "review",
                kind: ActivityKind::Response,
                text: "overflow",
                label: "",
                provider: None,
                input_tokens: None,
                secret: None,
            }
        ),
        Err(EvidenceError::Full)
    );
    store
        .record_terminal(run, attempt, input())
        .expect("existing terminal");
    assert!(store.get(&other_run, &other_attempt).is_none());
    assert_eq!(
        store.get(&run, &attempt).unwrap().terminal.unwrap().text,
        "Finished"
    );
}

#[test]
fn escaped_activity_cannot_displace_the_terminal_result() {
    let dir = tempfile::tempdir().expect("directory");
    let store = WorkflowEvidenceStore::open(dir.path().to_path_buf()).expect("store");
    let (run, attempt) = ids();
    let text = "\u{0001}".repeat(MAXIMUM_ACTIVITY_TEXT_BYTES);
    for _ in 0..MAXIMUM_ACTIVITY_BYTES / MAXIMUM_ACTIVITY_TEXT_BYTES {
        store
            .append_activity(
                run,
                attempt,
                ActivityInput {
                    phase: "review",
                    kind: ActivityKind::Response,
                    text: &text,
                    label: "",
                    provider: None,
                    input_tokens: None,
                    secret: None,
                },
            )
            .expect("activity");
    }
    let mut reply = AssistantReply::from(text.clone());
    reply.thinking = text.clone();
    reply.tools = vec![
        ToolOutput {
            label: text.clone(),
            output: text.clone(),
            command: None,
        };
        4
    ];
    store
        .record_terminal(
            run,
            attempt,
            TerminalInput {
                phase: "review",
                state: TerminalState::Completed,
                reply: &reply,
                error: Some(&text),
                secret: None,
            },
        )
        .expect("terminal fits despite JSON escaping");
    let reopened = WorkflowEvidenceStore::open(dir.path().to_path_buf()).expect("reopen");
    let evidence = reopened.get(&run, &attempt).expect("evidence");
    assert!(evidence.activity_truncated);
    let terminal = evidence.terminal.expect("retained terminal");
    assert_eq!(terminal.text, reply.text);
    assert!(terminal.truncated);
}

#[test]
fn configured_step_history_stays_within_its_attempt() {
    use crate::providers::{ChatToolCall, ChatTurn, Role};

    let store = WorkflowEvidenceStore::in_memory();
    let run = RunId::generate().expect("run");
    let attempt = AttemptId::generate().expect("attempt");
    let other = AttemptId::generate().expect("attempt");
    let turn = ChatTurn {
        role: Role::Assistant,
        text: "Reading".to_owned(),
        thinking: String::new(),
        tools: Vec::new(),
        activity: Vec::new(),
        usage: None,
        calls: vec![ChatToolCall {
            id: "call-1".to_owned(),
            name: "read".to_owned(),
            arguments: serde_json::json!({"path": "src/main.rs"}),
            result: Some(ToolOutput {
                label: "read src/main.rs".to_owned(),
                output: "fn main() {}".to_owned(),
                command: None,
            }),
        }],
        continuation: Vec::new(),
    };
    store
        .append_history(run, attempt, &turn, "review")
        .expect("history");
    store
        .append_history(run, other, &turn, "review")
        .expect("independent attempt");
    let evidence = store.get(&run, &attempt).expect("evidence");
    assert_eq!(evidence.history, vec![turn.clone()]);
    assert_eq!(
        store.get(&run, &other).expect("other").history,
        vec![turn.clone()]
    );
    assert_eq!(
        store.get(&run, &attempt).expect("stable").history,
        vec![turn]
    );
}
