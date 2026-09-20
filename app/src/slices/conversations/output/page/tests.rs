use askama::Template;

use super::OutputView;

#[test]
fn output_page_escapes_untrusted_stream_text() {
    let view = OutputView {
        heading: "Command output".to_owned(),
        reference: "a".repeat(32),
        back_href: "/conversations/abc".to_owned(),
        error: String::new(),
        chunks: vec![super::OutputChunkView {
            stream: "stderr",
            stderr: true,
            text: "<script>alert(1)</script>".to_owned(),
        }],
        next_cursor: Some("10".to_owned()),
        next_href: "/conversations/abc/output/ref?cursor=10".to_owned(),
        truncated: false,
    };
    let html = view.render().expect("output page");
    assert!(!html.contains("<script>"));
    assert!(html.contains("&#60;script&#62;alert(1)&#60;/script&#62;"));
}
