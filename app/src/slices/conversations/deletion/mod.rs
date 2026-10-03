mod page;

#[cfg(test)]
mod tests;

use axum::{Form, extract::State, response::Response};
use hypergraft::{PatchGraft, PatchStatus};

use crate::{
    conversations::{ConversationError, ConversationId, ConversationMetadata},
    error::{AppError, AppResult},
    sessions::RequiredSession,
    state::AppState,
};

const MAXIMUM_SELECTION: usize = 10_000;
const STALE: &str = "The selection changed. Refresh the list and select the conversations again. Nothing was deleted.";
const BLOCKED: &str = "A selected conversation has active work or a pending decision. Resolve it or remove that conversation from the selection.";

type Selection = Vec<(ConversationId, u32)>;

#[derive(Default)]
struct DeleteForm {
    selected: Selection,
    all: bool,
    confirmed: bool,
    directory: String,
    query: String,
    conversation: String,
}

impl DeleteForm {
    fn parse(pairs: Vec<(String, String)>) -> Result<Self, &'static str> {
        let mut form = Self::default();
        let mut fields = std::collections::BTreeSet::new();
        let mut ids = std::collections::BTreeSet::new();
        for (name, value) in pairs {
            if name != "selected" && !fields.insert(name.clone()) {
                return Err("The selection is not valid. Refresh the list and try again.");
            }
            match name.as_str() {
                "selected" => {
                    let (id, revision) = value.split_once(':').ok_or(STALE)?;
                    let id = ConversationId::parse(id).ok_or(STALE)?;
                    let revision = super::parse_revision(revision).ok_or(STALE)?;
                    if !ids.insert(id) || form.selected.len() >= MAXIMUM_SELECTION {
                        return Err("Select at most 10,000 distinct conversations at a time.");
                    }
                    form.selected.push((id, revision));
                }
                "scope" if value == "all" || value == "selected" => form.all = value == "all",
                "confirm" if value == "delete" => form.confirmed = true,
                "directory" if value.len() <= crate::agents::MAXIMUM_PATH_BYTES => {
                    form.directory = value
                }
                "q" if value.trim().len() <= 256 => form.query = value.trim().to_owned(),
                "conversation" if value.is_empty() || ConversationId::parse(&value).is_some() => {
                    form.conversation = value
                }
                _ => return Err("The selection is not valid. Refresh the list and try again."),
            }
        }
        Ok(form)
    }

    fn href(&self) -> String {
        super::page::catalogue_href(&self.directory, &self.query, &self.conversation, None)
    }
}

fn selected_records(
    state: &AppState,
    selected: &Selection,
) -> Result<Vec<ConversationMetadata>, &'static str> {
    if selected.is_empty() {
        return Err("Select at least one conversation.");
    }
    if selected.len() > MAXIMUM_SELECTION {
        return Err(
            "Select at most 10,000 conversations at a time. Use filters to narrow the list.",
        );
    }
    selected
        .iter()
        .map(|(id, revision)| {
            let record = state.conversations.metadata_for(id).ok_or(STALE)?;
            if record.revision != *revision {
                return Err(STALE);
            }
            if record.active_job.is_some()
                || state.sessions.conversation_reserved(*id)
                || super::has_pending_review(state, *id)
                || state.conversation_runtime.unsettled(*id)
            {
                return Err(BLOCKED);
            }
            Ok(record)
        })
        .collect()
}

pub(super) async fn review(
    State(state): State<AppState>,
    _session: RequiredSession,
    _graft: PatchGraft,
    Form(pairs): Form<Vec<(String, String)>>,
) -> AppResult<Response> {
    let mut form = match DeleteForm::parse(pairs) {
        Ok(form) => form,
        Err(error) => {
            return render(
                &DeleteForm::default(),
                &[],
                error,
                PatchStatus::UnprocessableEntity,
            );
        }
    };
    if form.all {
        // Resolve every match before confirmation, never again at deletion time.
        form.selected = state
            .conversations
            .try_metadata()
            .map_err(|error| AppError::new("load conversations for deletion", error))?
            .iter()
            .filter(|record| super::page::history_matches(record, &form.directory, &form.query))
            .map(|record| (record.id, record.revision))
            .collect();
    }
    match selected_records(&state, &form.selected) {
        Ok(records) => render(&form, &records, "", PatchStatus::Ok),
        Err(error) => render(&form, &[], error, PatchStatus::Conflict),
    }
}

pub(super) async fn delete(
    State(state): State<AppState>,
    session: RequiredSession,
    _graft: PatchGraft,
    Form(pairs): Form<Vec<(String, String)>>,
) -> AppResult<Response> {
    let mut form = match DeleteForm::parse(pairs) {
        Ok(form) if form.confirmed && !form.all => form,
        _ => {
            return render(
                &DeleteForm::default(),
                &[],
                "Review the selection before deletion.",
                PatchStatus::UnprocessableEntity,
            );
        }
    };
    if let Err(error) = selected_records(&state, &form.selected) {
        return render(&form, &[], error, PatchStatus::Conflict);
    }
    let mut reservations = Vec::with_capacity(form.selected.len());
    for (id, _) in &form.selected {
        let Ok(reservation) = state.sessions.reserve_command(session.0, *id) else {
            return render(&form, &[], BLOCKED, PatchStatus::Conflict);
        };
        reservations.push(reservation);
        if super::has_pending_review(&state, *id) || state.conversation_runtime.unsettled(*id) {
            return render(&form, &[], BLOCKED, PatchStatus::Conflict);
        }
    }
    match state.conversations.delete_many(&form.selected) {
        Ok(()) => {
            for (id, _) in &form.selected {
                state
                    .outputs
                    .remove_scope(&crate::execution::OutputScope::conversation(*id));
            }
            if !state.conversations.metadata().iter().any(|record| {
                super::page::history_grants(record)
                    .any(|grant| super::page::history_directory_key(grant) == form.directory)
            }) {
                form.directory.clear();
            }
            if ConversationId::parse(&form.conversation)
                .is_some_and(|id| !state.conversations.contains(&id))
            {
                form.conversation.clear();
            }
            Ok(crate::responses::command_navigation(&form.href()))
        }
        Err(ConversationError::Conflict | ConversationError::Missing) => {
            render(&form, &[], STALE, PatchStatus::Conflict)
        }
        Err(ConversationError::Active) => render(&form, &[], BLOCKED, PatchStatus::Conflict),
        Err(error) => Err(AppError::new("delete conversations", error)),
    }
}

fn render(
    form: &DeleteForm,
    records: &[ConversationMetadata],
    error: &'static str,
    status: PatchStatus,
) -> AppResult<Response> {
    Ok(hypergraft::outcome::children_patch(
        status,
        "conversation-delete-review",
        &page::DeleteView::new(form, records, error),
    )?)
}
