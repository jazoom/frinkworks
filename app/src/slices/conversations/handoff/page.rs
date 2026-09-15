use askama::Template;

use crate::{conversations::ConversationRecord, state::AppState};

#[derive(Template)]
#[template(path = "conversations/handoff/templates/index.html")]
pub(super) struct HandoffPage {
    source_id: String,
    source_title: String,
    revision: u32,
    model: String,
    focus: String,
    prompt: String,
    generated: bool,
    pending_changes: bool,
    can_transfer: bool,
    continue_prepared: bool,
    run_fingerprint: String,
    busy: bool,
    error: &'static str,
}

impl HandoffPage {
    pub(super) fn new(
        state: &AppState,
        record: &ConversationRecord,
        form: super::HandoffForm,
        generated: bool,
        error: &'static str,
        busy: bool,
    ) -> Self {
        Self {
            source_id: record.id.as_hex(),
            source_title: record.title.clone(),
            revision: record.revision,
            model: record
                .model
                .as_ref()
                .map(|model| model.settings.model.model.clone())
                .unwrap_or_default(),
            focus: form.focus,
            prompt: form.prompt,
            generated,
            pending_changes: super::super::has_pending_review(state, record.id),
            can_transfer: super::transfer::source_run(state, record).is_some(),
            continue_prepared: form.choice == "continue-prepared",
            run_fingerprint: if form.run_fingerprint.is_empty() {
                super::transfer::source_run(state, record)
                    .map(|run| run.handoff_fingerprint())
                    .unwrap_or_default()
            } else {
                form.run_fingerprint
            },
            busy,
            error,
        }
    }
}
