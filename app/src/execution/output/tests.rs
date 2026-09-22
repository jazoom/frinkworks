use super::*;
use crate::execution::command::{CommandChunk, CommandResult, CommandStream, CommandTermination};
use crate::tools::read::parse_request;

fn conversation() -> ConversationId {
    ConversationId::generate().expect("conversation")
}

fn key(conversation: ConversationId, tool_call: &str) -> OutputKey {
    OutputKey {
        scope: OutputScope::conversation(conversation),
        job: JobId::generate().expect("job"),
        tool_call: tool_call.to_owned(),
        model_hidden: false,
    }
}

fn result(text: &str) -> CommandResult {
    CommandResult::new(
        vec![CommandChunk {
            stream: CommandStream::Stdout,
            text: text.to_owned(),
        }],
        CommandTermination::Exited(0),
    )
}

fn first_page() -> crate::tools::read::PageRequest {
    parse_request(None, None).expect("first page")
}

#[test]
fn model_access_is_refused_for_a_hidden_record_without_an_explicit_reference() {
    let store = OutputStore::ephemeral();
    let conversation = conversation();
    let mut hidden = key(conversation, "call-1");
    hidden.model_hidden = true;
    let retained = store
        .store(&hidden, &result("excluded sentinel"))
        .expect("store");
    let scope = OutputScope::conversation(conversation);
    // The local view still serves the retained output.
    store
        .page(&retained.reference, &scope, first_page())
        .expect("local page");
    // The model path refuses it even with the exact reference.
    assert_eq!(
        store.model_page(&retained.reference, &scope, first_page()),
        Err(OutputError::Forbidden)
    );
}

#[test]
fn retained_output_round_trips_with_a_bounded_page_and_offset() {
    let store = OutputStore::ephemeral();
    let conversation = conversation();
    let text = (1..=20)
        .map(|line| format!("line-{line}\n"))
        .collect::<String>();
    let retained = store
        .store(&key(conversation, "call-1"), &result(&text))
        .expect("store");
    assert_eq!(retained.bytes, text.len());
    assert!(!retained.truncated);

    let scope = OutputScope::conversation(conversation);
    let first = store
        .page(
            &retained.reference,
            &scope,
            parse_request(Some(1), Some(5)).expect("first"),
        )
        .expect("page");
    assert_eq!(
        first.chunks[0].text,
        "line-1\nline-2\nline-3\nline-4\nline-5\n"
    );
    assert_eq!(first.next, Some(6));

    let second = store
        .page(
            &retained.reference,
            &scope,
            parse_request(first.next, Some(5)).expect("second"),
        )
        .expect("second page");
    assert!(second.chunks[0].text.starts_with("line-6\n"));
    assert!(!second.chunks[0].text.contains("line-1\n"));
    assert_eq!(second.next, Some(11));
}

#[test]
fn another_conversation_cannot_read_output() {
    let store = OutputStore::ephemeral();
    let owner = conversation();
    let retained = store
        .store(&key(owner, "call-1"), &result("secret"))
        .expect("store");
    let other = conversation();
    assert_eq!(
        store.page(
            &retained.reference,
            &OutputScope::conversation(other),
            first_page()
        ),
        Err(OutputError::Forbidden)
    );
    assert_eq!(
        store.page(
            &retained.reference,
            &OutputScope::conversation(owner),
            first_page()
        ),
        Ok(OutputPage {
            chunks: vec![CommandChunk {
                stream: CommandStream::Stdout,
                text: "secret".to_owned(),
            }],
            next: None,
            truncated: false,
            line_truncated: false,
        })
    );
}

#[test]
fn malformed_offsets_are_rejected() {
    let store = OutputStore::ephemeral();
    let conversation = conversation();
    let retained = store
        .store(&key(conversation, "call-1"), &result("data\n"))
        .expect("store");
    let scope = OutputScope::conversation(conversation);
    assert_eq!(
        store.page(
            &retained.reference,
            &scope,
            parse_request(Some(9), None).expect("past end")
        ),
        Err(OutputError::Cursor)
    );
    assert_eq!(
        store.page("not-a-reference", &scope, first_page()),
        Err(OutputError::Missing)
    );
}

#[test]
fn a_result_beyond_the_retained_limit_is_truncated() {
    let store = OutputStore::ephemeral();
    let conversation = conversation();
    let text = "x".repeat(MAXIMUM_RETAINED_BYTES + 1);
    let retained = store
        .store(&key(conversation, "call-1"), &result(&text))
        .expect("store");
    assert!(retained.truncated);
    assert_eq!(retained.bytes, MAXIMUM_RETAINED_BYTES);
}

#[test]
fn redaction_happens_before_storage() {
    let secret = "sk-secret";
    let command = result("before sk-secret after").redacted(Some(secret));
    let store = OutputStore::ephemeral();
    let conversation = conversation();
    let retained = store
        .store(&key(conversation, "call-1"), &command)
        .expect("store");
    let page = store
        .page(
            &retained.reference,
            &OutputScope::conversation(conversation),
            first_page(),
        )
        .expect("page");
    let text: String = page
        .chunks
        .iter()
        .map(|chunk| chunk.text.as_str())
        .collect();
    assert!(!text.contains(secret));
    assert!(text.contains("[redacted]"));
}

#[test]
fn pages_preserve_unicode_and_stream_order() {
    let store = OutputStore::ephemeral();
    let owner = conversation();
    let command = CommandResult::new(
        vec![
            CommandChunk {
                stream: CommandStream::Stdout,
                text: format!("é\n{}\n", "a".repeat(40)),
            },
            CommandChunk {
                stream: CommandStream::Stderr,
                text: "diagnostic\n".to_owned(),
            },
        ],
        CommandTermination::ResourceLimit,
    );
    let retained = store.store(&key(owner, "call"), &command).expect("store");
    let scope = OutputScope::conversation(owner);
    let first = store
        .page(
            &retained.reference,
            &scope,
            parse_request(Some(1), Some(1)).expect("first"),
        )
        .expect("first");
    assert_eq!(first.next, Some(2));
    assert_eq!(first.chunks[0].text, "é\n");
    let second = store
        .page(
            &retained.reference,
            &scope,
            parse_request(first.next, Some(2)).expect("second"),
        )
        .expect("second");
    assert_eq!(second.chunks[0].text, format!("{}\n", "a".repeat(40)));
    assert_eq!(second.chunks[1].text, "diagnostic\n");
    assert_eq!(second.next, None);
}

#[test]
fn removed_scopes_free_the_aggregate_budget() {
    let store = OutputStore::ephemeral();
    let conversation = conversation();
    store
        .store(&key(conversation, "call-1"), &result("data"))
        .expect("store");
    store.remove_scope(&OutputScope::conversation(conversation));
    assert_eq!(store.lock().bytes, 0);
}
