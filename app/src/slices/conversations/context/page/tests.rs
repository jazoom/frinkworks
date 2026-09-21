use askama::Template;

use super::{ContextView, ResourceView};

#[test]
fn context_page_escapes_untrusted_source_text() {
    let view = ContextView {
        heading: "Request context".to_owned(),
        model: "Xai · grok-4.6".to_owned(),
        request_id: "a".repeat(32),
        back_href: "/conversations/abc".to_owned(),
        error: String::new(),
        sources: vec![ResourceView {
            kind: "Instructions",
            scope: "project".to_owned(),
            path: "<script>alert(1)</script>".to_owned(),
            content_hash: "sha256:00".to_owned(),
        }],
        advertised: vec![ResourceView {
            kind: "Skill",
            scope: "project".to_owned(),
            path: "<img src=x>".to_owned(),
            content_hash: "sha256:11".to_owned(),
        }],
    };
    let html = view.render().expect("context page");
    assert!(!html.contains("<script>"));
    assert!(!html.contains("<img src=x>"));
    assert!(html.contains("&#60;script&#62;"));
}
