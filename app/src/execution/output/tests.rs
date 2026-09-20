use super::*;
use crate::execution::command::{CommandChunk, CommandResult, CommandStream, CommandTermination};

fn conversation() -> ConversationId {
    ConversationId::generate().expect("conversation")
}

fn key(conversation: ConversationId, tool_call: &str) -> OutputKey {
    OutputKey {
        scope: OutputScope::conversation(conversation),
        job: JobId::generate().expect("job"),
        tool_call: tool_call.to_owned(),
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

#[test]
fn retained_output_round_trips_with_a_bounded_page_and_cursor() {
    let store = OutputStore::ephemeral();
    let conversation = conversation();
    let text = "a".repeat(OUTPUT_PAGE_BYTES + 10);
    let retained = store
        .store(&key(conversation, "call-1"), &result(&text))
        .expect("store");
    assert_eq!(retained.bytes, text.len());
    assert!(!retained.truncated);

    let scope = OutputScope::conversation(conversation);
    let first = store.page(&retained.reference, &scope, 0).expect("page");
    assert_eq!(first.chunks.len(), 1);
    assert_eq!(first.chunks[0].text.len(), OUTPUT_PAGE_BYTES);
    assert_eq!(first.next, Some(OUTPUT_PAGE_BYTES));

    let second = store
        .page(&retained.reference, &scope, first.next.unwrap())
        .expect("second page");
    assert_eq!(second.chunks[0].text.len(), 10);
    assert_eq!(second.next, None);
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
        store.page(&retained.reference, &OutputScope::conversation(other), 0),
        Err(OutputError::Forbidden)
    );
    assert_eq!(
        store.page(&retained.reference, &OutputScope::conversation(owner), 0),
        Ok(OutputPage {
            chunks: vec![CommandChunk {
                stream: CommandStream::Stdout,
                text: "secret".to_owned(),
            }],
            next: None,
            truncated: false,
        })
    );
}

#[test]
fn malformed_cursors_are_rejected() {
    let store = OutputStore::ephemeral();
    let conversation = conversation();
    let retained = store
        .store(&key(conversation, "call-1"), &result("data"))
        .expect("store");
    let scope = OutputScope::conversation(conversation);
    assert_eq!(
        store.page(&retained.reference, &scope, 9999),
        Err(OutputError::Cursor)
    );
    assert_eq!(
        store.page("not-a-reference", &scope, 0),
        Err(OutputError::Missing)
    );
    let unicode = store
        .store(&key(conversation, "unicode"), &result("é"))
        .expect("store");
    assert_eq!(
        store.page(&unicode.reference, &scope, 1),
        Err(OutputError::Cursor)
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
            0,
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
fn pages_preserve_unicode_and_stream_order_at_the_byte_bound() {
    let store = OutputStore::ephemeral();
    let owner = conversation();
    let command = CommandResult::new(
        vec![
            CommandChunk {
                stream: CommandStream::Stdout,
                text: format!("{}é", "a".repeat(OUTPUT_PAGE_BYTES - 1)),
            },
            CommandChunk {
                stream: CommandStream::Stderr,
                text: "diagnostic".to_owned(),
            },
        ],
        CommandTermination::ResourceLimit,
    );
    let retained = store.store(&key(owner, "call"), &command).expect("store");
    assert!(retained.truncated);
    let scope = OutputScope::conversation(owner);
    let first = store.page(&retained.reference, &scope, 0).expect("first");
    assert_eq!(first.next, Some(OUTPUT_PAGE_BYTES - 1));
    assert_eq!(first.chunks.len(), 1);
    let second = store
        .page(&retained.reference, &scope, first.next.unwrap())
        .expect("second");
    assert_eq!(second.chunks[0].text, "é");
    assert_eq!(second.chunks[1], command.chunks[1]);
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
