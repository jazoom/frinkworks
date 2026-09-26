use crate::execution::command::{CommandChunk, CommandStream};

#[test]
fn output_body_escapes_untrusted_stream_text() {
    let html = super::body_html(
        String::new(),
        &[CommandChunk {
            stream: CommandStream::Stderr,
            text: "<script>alert(1)</script>".to_owned(),
        }],
        false,
    );
    assert!(!html.contains("<script>"));
    assert!(html.contains("&#60;script&#62;alert(1)&#60;/script&#62;"));
}
