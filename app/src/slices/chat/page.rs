use askama::Template;

#[derive(Template)]
#[template(path = "chat/templates/thinking_visibility.html")]
pub(crate) struct ThinkingVisibilityControl {
    pub(crate) show_thinking: bool,
    pub(crate) thinking_visibility_error: Option<&'static str>,
}
