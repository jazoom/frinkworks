use axum::{
    Form,
    extract::{Path, State},
    response::Response,
};
use hypergraft::{GraftRequest, PatchGraft, PatchStatus};

use crate::{
    conversations::ConversationError,
    error::{AppError, AppResult},
    execution::{DirectoryGrant, DirectoryGrantError, DirectoryGrantId, DirectoryPick},
    local_data::HOST_PATH_RESET_PENDING,
    responses,
    sessions::RequiredSession,
    state::AppState,
};

use super::{
    REVISION_MESSAGE, detail_view, load_conversation, parse_revision, render_detail,
    render_detail_command, status_for,
};

#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub(super) struct DirectoryForm {
    revision: String,
    consent_request: String,
    pending_directory: String,
    existing: String,
    access: String,
    start_directory: String,
}

struct PendingDirectory {
    grant: DirectoryGrant,
    request: String,
    existing: bool,
}

pub(super) async fn pick_new(
    State(state): State<AppState>,
    session: RequiredSession,
    _graft: PatchGraft,
    Form(mut form): Form<super::new::NewForm>,
) -> AppResult<Response> {
    let result = state.directory_picker.pick().await;
    let (status, error) = match result {
        DirectoryPick::Busy => (
            PatchStatus::Conflict,
            "Wait until the directory chooser closes.",
        ),
        DirectoryPick::Cancelled => (PatchStatus::Ok, ""),
        DirectoryPick::Selected(path) => {
            let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
                return render_new(
                    &state,
                    session.0,
                    form,
                    PatchStatus::Conflict,
                    HOST_PATH_RESET_PENDING,
                );
            };
            let existing = match form.directories() {
                Ok(existing) => existing,
                Err(error) => {
                    return render_new(
                        &state,
                        session.0,
                        form,
                        PatchStatus::UnprocessableEntity,
                        error,
                    );
                }
            };
            match DirectoryGrant::from_selected(&path, &existing) {
                Ok(grant) if sensitive(&state, &grant) => {
                    if form.draft_nonce.is_empty() {
                        form.draft_nonce = crate::execution::draft_nonce().map_err(|_| {
                            AppError::new(
                                "create draft consent nonce",
                                std::io::Error::other("system random source unavailable"),
                            )
                        })?;
                    }
                    let mut projected = existing;
                    projected.push(grant.clone());
                    let request = state
                        .access_consent
                        .request_draft(session.0, &form.consent_nonce(), &projected, &grant)
                        .map_err(|_| {
                            AppError::new(
                                "create directory consent request",
                                std::io::Error::other("system random source unavailable"),
                            )
                        })?;
                    form.pending_directory = grant.form_value();
                    form.consent_request = request;
                    form.consent_existing.clear();
                    (PatchStatus::Ok, "")
                }
                Ok(grant) => {
                    let mut directories = existing;
                    directories.push(grant);
                    form.set_directories(&directories);
                    form.pending_directory.clear();
                    form.consent_request.clear();
                    form.consent_existing.clear();
                    (PatchStatus::Ok, "")
                }
                Err(error) => (directory_status(error), error.message()),
            }
        }
    };
    render_new(&state, session.0, form, status, error)
}

pub(super) async fn request_new(
    State(state): State<AppState>,
    session: RequiredSession,
    _graft: PatchGraft,
    Path(grant_id): Path<String>,
    Form(mut form): Form<super::new::NewForm>,
) -> AppResult<Response> {
    let (Some(grant_id), Ok(directories)) =
        (DirectoryGrantId::parse(&grant_id), form.directories())
    else {
        return render_new(
            &state,
            session.0,
            form,
            PatchStatus::UnprocessableEntity,
            DirectoryGrantError::Invalid.message(),
        );
    };
    let Some(grant) = directories
        .iter()
        .find(|grant| grant.id == grant_id && needs_consent(&state, grant))
        .cloned()
    else {
        return render_new(
            &state,
            session.0,
            form,
            PatchStatus::UnprocessableEntity,
            DirectoryGrantError::Invalid.message(),
        );
    };
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
    form.consent_existing = "true".to_owned();
    render_new(&state, session.0, form, PatchStatus::Ok, "")
}

pub(super) async fn approve_new(
    State(state): State<AppState>,
    session: RequiredSession,
    _graft: PatchGraft,
    Form(mut form): Form<super::new::NewForm>,
) -> AppResult<Response> {
    let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
        return render_new(
            &state,
            session.0,
            form,
            PatchStatus::Conflict,
            HOST_PATH_RESET_PENDING,
        );
    };
    let existing = match form.directories() {
        Ok(directories) => directories,
        Err(error) => {
            return render_new(
                &state,
                session.0,
                form,
                PatchStatus::UnprocessableEntity,
                error,
            );
        }
    };
    let Some(grant) = form.pending_directory() else {
        return render_new(
            &state,
            session.0,
            form,
            PatchStatus::UnprocessableEntity,
            "The sensitive access request is invalid. Choose the directory again.",
        );
    };
    if grant.revalidate().is_err() {
        return render_new(
            &state,
            session.0,
            form,
            PatchStatus::Conflict,
            DirectoryGrantError::Unavailable.message(),
        );
    }
    let reapproval = form.consent_existing == "true"
        && existing
            .iter()
            .any(|saved| saved == &grant && needs_consent(&state, saved));
    let mut projected = existing;
    if !reapproval {
        projected.push(grant.clone());
    }
    let reference = match state.access_consent.approve_draft(
        &form.consent_request,
        session.0,
        &form.consent_nonce(),
        &projected,
        &grant,
    ) {
        Ok(reference) => reference,
        Err(_) => {
            return render_new(
                &state,
                session.0,
                form,
                PatchStatus::UnprocessableEntity,
                "The sensitive access request expired or changed. Choose the directory again.",
            );
        }
    };
    form.set_directories(&projected);
    if !form.consent_reference.is_empty() {
        form.consent_reference.push(',');
    }
    form.consent_reference.push_str(&reference);
    form.pending_directory.clear();
    form.consent_request.clear();
    form.consent_existing.clear();
    render_new(&state, session.0, form, PatchStatus::Ok, "")
}

pub(super) async fn update_new(
    State(state): State<AppState>,
    session: RequiredSession,
    Path(grant_id): Path<String>,
    Form(mut form): Form<super::new::NewForm>,
) -> AppResult<Response> {
    let (Some(grant_id), Some(access), Ok(mut directories)) = (
        DirectoryGrantId::parse(&grant_id),
        crate::execution::DirectoryAccess::parse(&form.action),
        form.directories(),
    ) else {
        return render_new(
            &state,
            session.0,
            form,
            PatchStatus::UnprocessableEntity,
            DirectoryGrantError::Invalid.message(),
        );
    };
    let Some(index) = directories.iter().position(|grant| grant.id == grant_id) else {
        return render_new(
            &state,
            session.0,
            form,
            PatchStatus::UnprocessableEntity,
            DirectoryGrantError::Invalid.message(),
        );
    };
    directories[index].access = access;
    if let Err(error) = crate::execution::validate_directories(&directories) {
        return render_new(
            &state,
            session.0,
            form,
            PatchStatus::UnprocessableEntity,
            error.message(),
        );
    }
    let grant = directories[index].clone();
    if needs_consent(&state, &grant) {
        if form.draft_nonce.is_empty() {
            form.draft_nonce = crate::execution::draft_nonce().map_err(|_| {
                AppError::new(
                    "create draft consent nonce",
                    std::io::Error::other("system random source unavailable"),
                )
            })?;
        }
        let request = state
            .access_consent
            .request_draft(session.0, &form.consent_nonce(), &directories, &grant)
            .map_err(|_| {
                AppError::new(
                    "create directory consent request",
                    std::io::Error::other("system random source unavailable"),
                )
            })?;
        form.set_directories(&directories);
        form.pending_directory = grant.form_value();
        form.consent_request = request;
        form.consent_existing = "true".to_owned();
    } else {
        form.set_directories(&directories);
        form.pending_directory.clear();
        form.consent_request.clear();
        form.consent_existing.clear();
    }
    render_new(&state, session.0, form, PatchStatus::Ok, "")
}

pub(super) async fn remove_new(
    State(state): State<AppState>,
    session: RequiredSession,
    _graft: PatchGraft,
    Path(grant_id): Path<String>,
    Form(mut form): Form<super::new::NewForm>,
) -> AppResult<Response> {
    let Some(grant_id) = DirectoryGrantId::parse(&grant_id) else {
        return render_new(
            &state,
            session.0,
            form,
            PatchStatus::UnprocessableEntity,
            DirectoryGrantError::Invalid.message(),
        );
    };
    if form
        .pending_directory()
        .is_some_and(|grant| grant.id == grant_id)
        && form.consent_existing != "true"
    {
        form.pending_directory.clear();
        form.consent_request.clear();
        form.consent_existing.clear();
        return render_new(&state, session.0, form, PatchStatus::Ok, "");
    }
    let directories = match form.directories() {
        Ok(directories) => directories,
        Err(error) => {
            return render_new(
                &state,
                session.0,
                form,
                PatchStatus::UnprocessableEntity,
                error,
            );
        }
    };
    if !directories.iter().any(|grant| grant.id == grant_id) {
        return render_new(
            &state,
            session.0,
            form,
            PatchStatus::UnprocessableEntity,
            DirectoryGrantError::Invalid.message(),
        );
    }
    let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
        return render_new(
            &state,
            session.0,
            form,
            PatchStatus::Conflict,
            HOST_PATH_RESET_PENDING,
        );
    };
    let retained = directories
        .into_iter()
        .filter(|grant| grant.id != grant_id)
        .collect::<Vec<_>>();
    form.set_directories(&retained);
    form.pending_directory.clear();
    form.consent_request.clear();
    form.consent_existing.clear();
    render_new(&state, session.0, form, PatchStatus::Ok, "")
}

pub(super) async fn pick_saved(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path(conversation_id): Path<String>,
    Form(form): Form<DirectoryForm>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let Some(revision) = parse_revision(&form.revision) else {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::UnprocessableEntity,
            REVISION_MESSAGE,
        );
    };
    match state.directory_picker.pick().await {
        DirectoryPick::Busy => render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::Conflict,
            "Wait until the directory chooser closes.",
        ),
        DirectoryPick::Cancelled => {
            render_saved(&state, session.0, graft, &record, PatchStatus::Ok, "")
        }
        DirectoryPick::Selected(path) => {
            let existing = record
                .model
                .as_ref()
                .map(|model| model.settings.directories.as_slice())
                .unwrap_or_default();
            let grant = match DirectoryGrant::from_selected(&path, existing) {
                Ok(grant) if sensitive(&state, &grant) => {
                    let mut projected = existing.to_vec();
                    projected.push(grant.clone());
                    let Some(mut settings) =
                        record.model.as_ref().map(|model| model.settings.clone())
                    else {
                        return render_saved(
                            &state,
                            session.0,
                            graft,
                            &record,
                            PatchStatus::UnprocessableEntity,
                            "Choose a model before directory access.",
                        );
                    };
                    settings.directories = projected;
                    let request = state
                        .access_consent
                        .request_conversation(session.0, record.id, &settings, &grant)
                        .map_err(|_| {
                            AppError::new(
                                "create directory consent request",
                                std::io::Error::other("system random source unavailable"),
                            )
                        })?;
                    return render_saved_pending(
                        &state,
                        session.0,
                        graft,
                        &record,
                        PatchStatus::Ok,
                        "",
                        PendingDirectory {
                            grant,
                            request,
                            existing: false,
                        },
                    );
                }
                Ok(grant) => grant,
                Err(error) => {
                    return render_saved(
                        &state,
                        session.0,
                        graft,
                        &record,
                        directory_status(error),
                        error.message(),
                    );
                }
            };
            let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
                return render_saved(
                    &state,
                    session.0,
                    graft,
                    &record,
                    PatchStatus::Conflict,
                    HOST_PATH_RESET_PENDING,
                );
            };
            match state
                .conversations
                .add_directory(&record.id, revision, grant)
            {
                Ok(updated) => {
                    state.access_consent.invalidate_conversation(record.id);
                    render_saved(&state, session.0, graft, &updated, PatchStatus::Ok, "")
                }
                Err(error @ (ConversationError::Persist | ConversationError::Corrupt)) => {
                    Err(AppError::new("store conversation directory", error))
                }
                Err(error) => render_saved(
                    &state,
                    session.0,
                    graft,
                    &record,
                    status_for(error),
                    error.message(),
                ),
            }
        }
    }
}

pub(super) async fn request_saved(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path((conversation_id, grant_id)): Path<(String, String)>,
    Form(form): Form<DirectoryForm>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let (Some(revision), Some(grant_id)) = (
        parse_revision(&form.revision),
        DirectoryGrantId::parse(&grant_id),
    ) else {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::UnprocessableEntity,
            REVISION_MESSAGE,
        );
    };
    if revision != record.revision {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::Conflict,
            REVISION_MESSAGE,
        );
    }
    let Some(settings) = record.model.as_ref().map(|model| &model.settings) else {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::UnprocessableEntity,
            DirectoryGrantError::Invalid.message(),
        );
    };
    let Some(grant) = settings
        .directories
        .iter()
        .find(|grant| grant.id == grant_id && needs_consent(&state, grant))
        .cloned()
    else {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::UnprocessableEntity,
            DirectoryGrantError::Invalid.message(),
        );
    };
    let request = state
        .access_consent
        .request_conversation(session.0, record.id, settings, &grant)
        .map_err(|_| {
            AppError::new(
                "create directory consent request",
                std::io::Error::other("system random source unavailable"),
            )
        })?;
    render_saved_pending(
        &state,
        session.0,
        graft,
        &record,
        PatchStatus::Ok,
        "",
        PendingDirectory {
            grant,
            request,
            existing: true,
        },
    )
}

pub(super) async fn approve_saved(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path(conversation_id): Path<String>,
    Form(form): Form<DirectoryForm>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let Some(revision) = parse_revision(&form.revision) else {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::UnprocessableEntity,
            REVISION_MESSAGE,
        );
    };
    let Some(grant) = DirectoryGrant::parse_form(&form.pending_directory) else {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::UnprocessableEntity,
            "The sensitive access request is invalid. Choose the directory again.",
        );
    };
    if grant.revalidate().is_err() {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::Conflict,
            DirectoryGrantError::Unavailable.message(),
        );
    }
    let existing = record
        .model
        .as_ref()
        .map(|model| model.settings.directories.as_slice())
        .unwrap_or_default();
    let reapproval = form.existing == "true" && existing.iter().any(|saved| saved.id == grant.id);
    if !reapproval && DirectoryGrant::from_selected(&grant.host_path, existing).is_err() {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::UnprocessableEntity,
            "The sensitive access request no longer matches this conversation.",
        );
    }
    let mut projected = existing.to_vec();
    if reapproval {
        let Some(index) = projected.iter().position(|stored| stored.id == grant.id) else {
            return render_saved(
                &state,
                session.0,
                graft,
                &record,
                PatchStatus::Conflict,
                REVISION_MESSAGE,
            );
        };
        projected[index] = grant.clone();
    } else {
        projected.push(grant.clone());
    }
    if revision != record.revision || record.active_job.is_some() {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::Conflict,
            REVISION_MESSAGE,
        );
    }
    let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::Conflict,
            HOST_PATH_RESET_PENDING,
        );
    };
    let Some(mut settings) = record.model.as_ref().map(|model| model.settings.clone()) else {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::UnprocessableEntity,
            "Choose a model before directory access.",
        );
    };
    settings.directories = projected;
    if state
        .access_consent
        .approve_conversation(
            &form.consent_request,
            session.0,
            record.id,
            &settings,
            &grant,
        )
        .is_err()
    {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::UnprocessableEntity,
            "The sensitive access request expired or changed. Choose the directory again.",
        );
    }
    let approval_grant = grant.clone();
    let changed = if reapproval {
        state
            .conversations
            .update_directory(&record.id, revision, grant)
    } else {
        state
            .conversations
            .add_directory(&record.id, revision, grant)
    };
    match changed {
        Ok(updated) => {
            let approval = updated.model.as_ref().map(|model| {
                crate::conversations::DirectoryApproval::for_grant(&model.settings, &approval_grant)
            });
            let updated = match approval {
                None => updated,
                Some(approval) => match state.conversations.record_directory_approval(
                    &updated.id,
                    updated.revision,
                    approval,
                ) {
                    Ok(updated) => updated,
                    Err(error) => {
                        state.access_consent.invalidate_conversation(record.id);
                        if matches!(
                            error,
                            ConversationError::Persist | ConversationError::Corrupt
                        ) {
                            return Err(AppError::new(
                                "store sensitive conversation directory",
                                error,
                            ));
                        }
                        return render_saved(
                            &state,
                            session.0,
                            graft,
                            &updated,
                            status_for(error),
                            error.message(),
                        );
                    }
                },
            };
            render_saved(&state, session.0, graft, &updated, PatchStatus::Ok, "")
        }
        Err(error) => {
            state.access_consent.invalidate_conversation(record.id);
            if matches!(
                error,
                ConversationError::Persist | ConversationError::Corrupt
            ) {
                return Err(AppError::new(
                    "store sensitive conversation directory",
                    error,
                ));
            }
            render_saved(
                &state,
                session.0,
                graft,
                &record,
                status_for(error),
                error.message(),
            )
        }
    }
}

pub(super) async fn update_saved(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path((conversation_id, grant_id)): Path<(String, String)>,
    Form(form): Form<DirectoryForm>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let (Some(revision), Some(grant_id), Some(access)) = (
        parse_revision(&form.revision),
        DirectoryGrantId::parse(&grant_id),
        crate::execution::DirectoryAccess::parse(&form.access),
    ) else {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::UnprocessableEntity,
            DirectoryGrantError::Invalid.message(),
        );
    };
    if revision != record.revision {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::Conflict,
            REVISION_MESSAGE,
        );
    }
    let Some(mut grant) = record
        .model
        .as_ref()
        .and_then(|model| {
            model
                .settings
                .directories
                .iter()
                .find(|grant| grant.id == grant_id)
        })
        .cloned()
    else {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::UnprocessableEntity,
            DirectoryGrantError::Invalid.message(),
        );
    };
    grant.access = access;
    let mut settings = record
        .model
        .as_ref()
        .expect("checked model")
        .settings
        .clone();
    let index = settings
        .directories
        .iter()
        .position(|stored| stored.id == grant.id)
        .expect("checked grant");
    settings.directories[index] = grant.clone();
    if let Err(error) = crate::execution::validate_directories(&settings.directories) {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::UnprocessableEntity,
            error.message(),
        );
    }
    if needs_consent(&state, &grant) {
        let request = state
            .access_consent
            .request_conversation(session.0, record.id, &settings, &grant)
            .map_err(|_| {
                AppError::new(
                    "create directory consent request",
                    std::io::Error::other("system random source unavailable"),
                )
            })?;
        return render_saved_pending(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::Ok,
            "",
            PendingDirectory {
                grant,
                request,
                existing: true,
            },
        );
    }
    let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::Conflict,
            HOST_PATH_RESET_PENDING,
        );
    };
    match state
        .conversations
        .update_directory(&record.id, revision, grant)
    {
        Ok(updated) => {
            state.access_consent.invalidate_conversation(record.id);
            render_saved(&state, session.0, graft, &updated, PatchStatus::Ok, "")
        }
        Err(error) => render_saved(
            &state,
            session.0,
            graft,
            &record,
            status_for(error),
            error.message(),
        ),
    }
}

pub(super) async fn remove_saved(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path((conversation_id, grant_id)): Path<(String, String)>,
    Form(form): Form<DirectoryForm>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let (Some(revision), Some(grant_id)) = (
        parse_revision(&form.revision),
        DirectoryGrantId::parse(&grant_id),
    ) else {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::UnprocessableEntity,
            REVISION_MESSAGE,
        );
    };
    let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::Conflict,
            HOST_PATH_RESET_PENDING,
        );
    };
    match state
        .conversations
        .remove_directory(&record.id, revision, grant_id)
    {
        Ok(updated) => {
            state.access_consent.invalidate_conversation(record.id);
            render_saved(&state, session.0, graft, &updated, PatchStatus::Ok, "")
        }
        Err(error @ (ConversationError::Persist | ConversationError::Corrupt)) => {
            Err(AppError::new("remove conversation directory", error))
        }
        Err(error) => render_saved(
            &state,
            session.0,
            graft,
            &record,
            status_for(error),
            error.message(),
        ),
    }
}

pub(super) async fn start_new(
    State(state): State<AppState>,
    session: RequiredSession,
    _graft: PatchGraft,
    Form(mut form): Form<super::new::NewForm>,
) -> AppResult<Response> {
    let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
        return render_new(
            &state,
            session.0,
            form,
            PatchStatus::Conflict,
            HOST_PATH_RESET_PENDING,
        );
    };
    let mut directories = match form.directories() {
        Ok(directories) => directories,
        Err(error) => {
            return render_new(
                &state,
                session.0,
                form,
                PatchStatus::UnprocessableEntity,
                error,
            );
        }
    };
    match select_start_directory(&mut directories, &form.start_directory) {
        Ok(true) => {
            form.set_directories(&directories);
            form.consent_reference.clear();
            form.consent_request.clear();
            form.pending_directory.clear();
            form.consent_existing.clear();
            form.host_consent_request.clear();
        }
        Ok(false) => {}
        Err(error) => {
            return render_new(
                &state,
                session.0,
                form,
                directory_status(error),
                error.message(),
            );
        }
    }
    render_new(&state, session.0, form, PatchStatus::Ok, "")
}

pub(super) async fn start_saved(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path(conversation_id): Path<String>,
    Form(form): Form<DirectoryForm>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let Some(revision) = parse_revision(&form.revision) else {
        return render_saved(
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
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            status_for(error),
            error.message(),
        );
    }
    let Some(model) = &record.model else {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::UnprocessableEntity,
            DirectoryGrantError::Invalid.message(),
        );
    };
    let Ok(_permit) = state.local_data.begin_host_path_mutation().await else {
        return render_saved(
            &state,
            session.0,
            graft,
            &record,
            PatchStatus::Conflict,
            HOST_PATH_RESET_PENDING,
        );
    };
    let mut settings = model.settings.clone();
    match select_start_directory(&mut settings.directories, &form.start_directory) {
        Ok(true) => {}
        Ok(false) => return render_saved(&state, session.0, graft, &record, PatchStatus::Ok, ""),
        Err(error) => {
            return render_saved(
                &state,
                session.0,
                graft,
                &record,
                directory_status(error),
                error.message(),
            );
        }
    }
    match state
        .conversations
        .update_execution_settings(&record.id, revision, settings)
    {
        Ok(updated) => {
            state.access_consent.invalidate_conversation(record.id);
            state.host_approvals.invalidate_conversation(record.id);
            state
                .conversations
                .invalidate_questions_for_conversation(record.id);
            render_saved(&state, session.0, graft, &updated, PatchStatus::Ok, "")
        }
        Err(error @ (ConversationError::Persist | ConversationError::Corrupt)) => {
            Err(AppError::new("store command start directory", error))
        }
        Err(error) => render_saved(
            &state,
            session.0,
            graft,
            &record,
            status_for(error),
            error.message(),
        ),
    }
}

fn select_start_directory(
    directories: &mut [DirectoryGrant],
    selected: &str,
) -> Result<bool, DirectoryGrantError> {
    let id = DirectoryGrantId::parse(selected).ok_or(DirectoryGrantError::Invalid)?;
    let index = directories
        .iter()
        .position(|grant| grant.id == id)
        .ok_or(DirectoryGrantError::Invalid)?;
    directories[index].revalidate()?;
    // The first grant selects the command location. Keep every grant and its authority unchanged.
    directories[..=index].rotate_right(1);
    Ok(index != 0)
}

fn needs_consent(state: &AppState, grant: &DirectoryGrant) -> bool {
    grant.requires_access_consent(state.local_data.root())
}

fn sensitive(state: &AppState, grant: &DirectoryGrant) -> bool {
    crate::execution::authority::sensitive_directory(&grant.host_path, state.local_data.root())
}

fn directory_status(error: DirectoryGrantError) -> PatchStatus {
    match error {
        DirectoryGrantError::Full => PatchStatus::Conflict,
        _ => PatchStatus::UnprocessableEntity,
    }
}

fn render_new(
    state: &AppState,
    session: crate::sessions::SessionId,
    form: super::new::NewForm,
    status: PatchStatus,
    error: &'static str,
) -> AppResult<Response> {
    render_detail(
        state,
        session,
        GraftRequest::Patch,
        status,
        super::page::ConversationDetailView::from_new(state, session, form, error)
            .open_directories(),
    )
}

fn render_saved_pending(
    state: &AppState,
    session: crate::sessions::SessionId,
    graft: PatchGraft,
    record: &crate::conversations::ConversationRecord,
    status: PatchStatus,
    error: &'static str,
    pending: PendingDirectory,
) -> AppResult<Response> {
    render_detail_command(
        graft,
        status,
        detail_view(state, session, record, &record.title, error)
            .with_pending_directory(state, pending.grant, pending.request, pending.existing)
            .open_directories(),
    )
}

fn render_saved(
    state: &AppState,
    session: crate::sessions::SessionId,
    graft: PatchGraft,
    record: &crate::conversations::ConversationRecord,
    status: PatchStatus,
    error: &'static str,
) -> AppResult<Response> {
    render_detail_command(
        graft,
        status,
        detail_view(state, session, record, &record.title, error).open_directories(),
    )
}

#[cfg(test)]
mod tests;
