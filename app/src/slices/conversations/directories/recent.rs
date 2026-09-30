#[cfg(test)]
mod tests;

use axum::{
    Form,
    extract::{Path, State},
    response::Response,
};
use hypergraft::{PatchGraft, PatchStatus};

use crate::{
    conversations::ConversationError,
    error::{AppError, AppResult},
    execution::{DirectoryGrant, DirectoryGrantError},
    local_data::HOST_PATH_RESET_PENDING,
    preferences::RecentDirectory,
    responses,
    sessions::RequiredSession,
    state::AppState,
};

use super::super::{REVISION_MESSAGE, load_conversation, new::NewForm, parse_revision, status_for};

fn entry(state: &AppState, id: &str) -> Result<RecentDirectory, &'static str> {
    state
        .preferences
        .recent_directories()
        .into_iter()
        .find(|entry| entry.id == id)
        .ok_or("That recent directory is no longer available.")
}

fn selected_grant(
    entry: &RecentDirectory,
    directories: &[DirectoryGrant],
) -> Result<DirectoryGrant, DirectoryGrantError> {
    // A saved shortcut cannot follow a redirected path into a new grant.
    if entry
        .grant()
        .is_none_or(|grant| grant.revalidate().is_err())
    {
        return Err(DirectoryGrantError::Unavailable);
    }
    let mut grant = DirectoryGrant::from_selected(&entry.host_path, directories)?;
    if grant.host_path != entry.host_path {
        return Err(DirectoryGrantError::Unavailable);
    }
    grant.access = entry.access;
    Ok(grant)
}

pub(in crate::slices::conversations) async fn select(
    State(state): State<AppState>,
    session: RequiredSession,
    _graft: PatchGraft,
    Path(id): Path<String>,
    Form(mut form): Form<NewForm>,
) -> AppResult<Response> {
    let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
        return super::render_new(
            &state,
            session.0,
            form,
            PatchStatus::Conflict,
            HOST_PATH_RESET_PENDING,
        );
    };
    let entry = match entry(&state, &id) {
        Ok(entry) => entry,
        Err(error) => {
            return super::render_new(&state, session.0, form, PatchStatus::Conflict, error);
        }
    };
    let mut directories = match form.directories() {
        Ok(directories) => directories,
        Err(error) => {
            return super::render_new(
                &state,
                session.0,
                form,
                PatchStatus::UnprocessableEntity,
                error,
            );
        }
    };
    let grant = match selected_grant(&entry, &directories) {
        Ok(grant) => grant,
        Err(error) => {
            return super::render_new(
                &state,
                session.0,
                form,
                super::directory_status(error),
                error.message(),
            );
        }
    };
    directories.push(grant.clone());
    if super::needs_consent(&state, &grant) {
        let request = state
            .access_consent
            .request_draft(session.0, &form.consent_nonce(), &directories, &grant)
            .map_err(|_| {
                AppError::new(
                    "create directory consent request",
                    std::io::Error::other("system random source unavailable"),
                )
            })?;
        form.pending_directory = grant.form_value();
        form.consent_request = request;
        form.consent_existing.clear();
    } else {
        super::remember(&state, &grant, false)?;
        form.set_directories(&directories);
        form.pending_directory.clear();
        form.consent_request.clear();
        form.consent_existing.clear();
    }
    super::render_new(&state, session.0, form, PatchStatus::Ok, "")
}

pub(in crate::slices::conversations) async fn forget(
    State(state): State<AppState>,
    session: RequiredSession,
    _graft: PatchGraft,
    Path(id): Path<String>,
    Form(form): Form<NewForm>,
) -> AppResult<Response> {
    let entry = match entry(&state, &id) {
        Ok(entry) => entry,
        Err(error) => {
            return super::render_new(&state, session.0, form, PatchStatus::Conflict, error);
        }
    };
    let directories = match form.directories() {
        Ok(directories) => directories,
        Err(error) => {
            return super::render_new(
                &state,
                session.0,
                form,
                PatchStatus::UnprocessableEntity,
                error,
            );
        }
    };
    if directories
        .iter()
        .any(|grant| grant.host_path == entry.host_path)
        || form
            .pending_directory()
            .is_some_and(|grant| grant.host_path == entry.host_path)
    {
        return super::render_new(
            &state,
            session.0,
            form,
            PatchStatus::Conflict,
            "Remove this directory from the conversation before Forget.",
        );
    }
    state
        .preferences
        .forget_directory(&id)
        .map_err(|error| AppError::new("forget recent directory", error))?;
    super::render_new(&state, session.0, form, PatchStatus::Ok, "")
}

pub(in crate::slices::conversations) async fn select_saved(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path((conversation_id, id)): Path<(String, String)>,
    Form(form): Form<super::DirectoryForm>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let Some(revision) = parse_revision(&form.revision) else {
        return super::render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::UnprocessableEntity,
            REVISION_MESSAGE,
        );
    };
    if revision != record.revision || record.active_job.is_some() {
        let error = if revision != record.revision {
            ConversationError::Conflict
        } else {
            ConversationError::Active
        };
        return super::render_saved(
            &state,
            session.0,
            graft,
            &record,
            status_for(error),
            error.message(),
        );
    }
    let entry = match entry(&state, &id) {
        Ok(entry) => entry,
        Err(error) => {
            return super::render_saved(
                &state,
                session.0,
                graft,
                &record,
                PatchStatus::Conflict,
                error,
            );
        }
    };
    let Some(mut settings) = record.model.as_ref().map(|model| model.settings.clone()) else {
        return super::render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::UnprocessableEntity,
            "Choose a model before directory access.",
        );
    };
    let grant = match selected_grant(&entry, &settings.directories) {
        Ok(grant) => grant,
        Err(error) => {
            return super::render_saved(
                &state,
                session.0,
                graft,
                &record,
                super::directory_status(error),
                error.message(),
            );
        }
    };
    settings.directories.push(grant.clone());
    let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
        return super::render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::Conflict,
            HOST_PATH_RESET_PENDING,
        );
    };
    if super::needs_consent(&state, &grant) {
        let request = state
            .access_consent
            .request_conversation(session.0, record.id, &settings, &grant)
            .map_err(|_| {
                AppError::new(
                    "create directory consent request",
                    std::io::Error::other("system random source unavailable"),
                )
            })?;
        return super::render_saved_pending(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::Ok,
            "",
            super::PendingDirectory {
                grant,
                request,
                existing: false,
            },
        );
    }
    match state
        .conversations
        .add_directory(&record.id, revision, grant.clone())
    {
        Ok(updated) => {
            super::remember(&state, &grant, false)?;
            state.access_consent.invalidate_conversation(record.id);
            super::render_saved(&state, session.0, graft, &updated, PatchStatus::Ok, "")
        }
        Err(error @ (ConversationError::Persist | ConversationError::Corrupt)) => {
            Err(AppError::new("store conversation directory", error))
        }
        Err(error) => super::render_saved(
            &state,
            session.0,
            graft,
            &record,
            status_for(error),
            error.message(),
        ),
    }
}

pub(in crate::slices::conversations) async fn forget_saved(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path((conversation_id, id)): Path<(String, String)>,
    Form(form): Form<super::DirectoryForm>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    if parse_revision(&form.revision) != Some(record.revision) {
        return super::render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::Conflict,
            REVISION_MESSAGE,
        );
    }
    let entry = match entry(&state, &id) {
        Ok(entry) => entry,
        Err(error) => {
            return super::render_saved(
                &state,
                session.0,
                graft,
                &record,
                PatchStatus::Conflict,
                error,
            );
        }
    };
    if record.model.as_ref().is_some_and(|model| {
        model
            .settings
            .directories
            .iter()
            .any(|grant| grant.host_path == entry.host_path)
    }) {
        return super::render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::Conflict,
            "Remove this directory from the conversation before Forget.",
        );
    }
    state
        .preferences
        .forget_directory(&id)
        .map_err(|error| AppError::new("forget recent directory", error))?;
    super::render_saved(&state, session.0, graft, &record, PatchStatus::Ok, "")
}
