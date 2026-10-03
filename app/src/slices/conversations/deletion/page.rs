use askama::Template;

#[derive(Template)]
#[template(path = "conversations/deletion/templates/index.html")]
pub(super) struct DeleteView<'a> {
    form: &'a super::DeleteForm,
    count: usize,
    selected: Vec<String>,
    titles: Vec<&'a str>,
    remaining: usize,
    error: &'static str,
    href: String,
}

impl<'a> DeleteView<'a> {
    pub(super) fn new(
        form: &'a super::DeleteForm,
        records: &'a [crate::conversations::ConversationMetadata],
        error: &'static str,
    ) -> Self {
        Self {
            form,
            count: form.selected.len(),
            selected: if error.is_empty() {
                form.selected
                    .iter()
                    .map(|(id, revision)| format!("{}:{revision}", id.as_hex()))
                    .collect()
            } else {
                Vec::new()
            },
            titles: records
                .iter()
                .take(8)
                .map(|record| record.title.as_str())
                .collect(),
            remaining: records.len().saturating_sub(8),
            error,
            href: form.href(),
        }
    }
}
