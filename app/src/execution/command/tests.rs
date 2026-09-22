use super::{CommandCapture, CommandStream, CommandTermination};

#[test]
fn a_credential_split_across_reads_never_leaks_at_any_boundary() {
    let secret = "sk-sécret";
    let mut capture = CommandCapture::with_secret(Some(secret));
    let bytes = format!("before {secret} after").into_bytes();
    let mut published = String::new();
    for byte in &bytes {
        for chunk in capture
            .push(CommandStream::Stdout, std::slice::from_ref(byte))
            .safe
        {
            published.push_str(&chunk.text);
        }
        assert!(!published.contains(secret));
    }
    let chunks = capture.finish();
    let retained: String = chunks.iter().map(|chunk| chunk.text.as_str()).collect();
    assert_eq!(retained, "before [redacted] after");
    assert_eq!(published, retained);
}

#[test]
fn interleaved_streams_keep_the_original_order_after_a_possible_secret_prefix() {
    let mut capture = CommandCapture::with_secret(Some("secret"));
    assert!(capture.push(CommandStream::Stdout, b"sec").safe.is_empty());
    assert!(
        capture
            .push(CommandStream::Stderr, b"diagnostic")
            .safe
            .is_empty()
    );
    let published = capture.push(CommandStream::Stdout, b"tion").safe;
    let result = capture.into_result(CommandTermination::Exited(0));
    assert_eq!(published, result.chunks);
    assert_eq!(result.chunks[0].text, "sec");
    assert_eq!(result.chunks[1].stream, CommandStream::Stderr);
    assert_eq!(result.chunks[1].text, "diagnostic");
    assert_eq!(result.chunks[2].text, "tion");
}

#[test]
fn interleaved_streams_hide_credentials_and_interrupted_prefixes() {
    for split_stream in [CommandStream::Stdout, CommandStream::Stderr] {
        let mut capture = CommandCapture::with_secret(Some("secret"));
        capture.push(CommandStream::Stdout, b"sec");
        capture.push(split_stream, b"ret");
        capture.push(CommandStream::Stderr, b" sec");
        let result = capture.into_result(CommandTermination::Cancelled);
        let text = result.combined();
        assert!(!text.contains("secret"));
        assert!(!text.contains("sec"));
        assert!(text.contains("[redacted]"));
    }
}

#[test]
fn partial_output_survives_restart_and_storage_failure_retains_its_reference() {
    use crate::execution::output::{OutputKey, OutputScope, OutputStore};

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("output");
    let store = OutputStore::open(path.clone()).unwrap();
    let owner = crate::conversations::ConversationId::generate().unwrap();
    let scope = OutputScope::conversation(owner);
    let key = OutputKey {
        scope: scope.clone(),
        job: crate::sessions::JobId::generate().unwrap(),
        tool_call: "call".to_owned(),
        model_hidden: false,
    };
    let mut capture = CommandCapture::with_output(Some("secret"), &store, &key).unwrap();
    let reference = capture.retained.as_ref().unwrap().reference.clone();
    capture.push(CommandStream::Stdout, b"before secret after");
    let reopened = OutputStore::open(path.clone()).unwrap();
    let page = reopened
        .page(
            &reference,
            &scope,
            crate::tools::read::parse_request(None, None).expect("page"),
        )
        .unwrap();
    assert_eq!(page.chunks[0].text, "before [redacted] after");

    std::fs::rename(&path, directory.path().join("previous")).unwrap();
    std::fs::write(&path, b"not a directory").unwrap();
    assert!(capture.push(CommandStream::Stderr, b"later").overflow);
    let result = capture.into_result(CommandTermination::Exited(0));
    assert_eq!(result.termination, CommandTermination::StorageFailure);
    assert_eq!(result.retained_reference(), Some(reference.as_str()));
    assert!(result.combined().ends_with("later"));
}

#[test]
fn split_utf8_keeps_its_first_byte_position_across_streams() {
    let mut capture = CommandCapture::new();
    capture.push(CommandStream::Stdout, &[0xc3]);
    capture.push(CommandStream::Stderr, b"diagnostic");
    capture.push(CommandStream::Stdout, &[0xa9, b'!']);
    let result = capture.into_result(CommandTermination::Exited(0));
    assert_eq!(result.chunks[0].text, "é");
    assert_eq!(result.chunks[1].text, "diagnostic");
    assert_eq!(result.chunks[2].text, "!");
}
