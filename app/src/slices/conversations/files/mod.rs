//! At-sign file lookup for saved conversations and unsaved drafts.
//!
//! The routes are read-only previews. They resolve approved roots, never start
//! a sandbox and never return file contents. A saved conversation uses its
//! saved settings. A draft carries its own directory grants because it has no
//! durable record yet.

pub(super) mod page;

#[cfg(test)]
mod tests;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use hypergraft::{GraftRequest, PatchStatus};
use serde::Deserialize;

use crate::{
    conversations::ConversationId,
    error::AppResult,
    execution::{
        DirectoryGrant, ProjectFreeAuthority, ToolLocation, resources::effective_roots,
        validate_directories,
    },
    responses,
    sessions::RequiredSession,
    state::AppState,
};

use self::page::Search;

#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct FilesQuery {
    q: String,
    mode: String,
}

/// The draft lookup carries only the fields that resolve roots. The composer
/// form already holds these values, so no server-side draft record is added.
#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct DraftFilesQuery {
    q: String,
    mode: String,
    draft_nonce: String,
    consent_reference: String,
    location: String,
    directory_0: String,
    directory_1: String,
    directory_2: String,
    directory_3: String,
    directory_4: String,
    directory_5: String,
    directory_6: String,
    directory_7: String,
}

impl DraftFilesQuery {
    fn directories(&self) -> Result<Vec<DirectoryGrant>, &'static str> {
        let directories = [
            &self.directory_0,
            &self.directory_1,
            &self.directory_2,
            &self.directory_3,
            &self.directory_4,
            &self.directory_5,
            &self.directory_6,
            &self.directory_7,
        ]
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .map(|value| DirectoryGrant::parse_form(value).ok_or("A directory grant is not valid."))
        .collect::<Result<Vec<_>, _>>()?;
        validate_directories(&directories).map_err(|_| "The draft directories are not valid.")?;
        Ok(directories)
    }

    fn location(&self) -> ToolLocation {
        ToolLocation::parse(self.location.trim()).unwrap_or(ToolLocation::Sandbox)
    }
}

pub(super) async fn lookup_saved(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: GraftRequest,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    Query(query): Query<FilesQuery>,
) -> AppResult<Response> {
    let Some(id) = ConversationId::parse(&conversation_id) else {
        return Ok(responses::request_navigation(graft, "/conversations"));
    };
    let Some(record) = state.conversations.metadata_for(&id) else {
        return Ok(responses::request_navigation(graft, "/conversations"));
    };
    let destination = format!("/conversations/{}", id.as_hex());
    let Some(model) = record.model.as_ref() else {
        return respond(
            graft,
            wants_json(&headers),
            &destination,
            &Search::default(),
            false,
            true,
        );
    };
    let settings = &model.settings;
    let mut restricted = false;
    let grants = settings
        .directories
        .iter()
        .filter(|grant| {
            if grant.revalidate().is_err() {
                restricted = true;
                return false;
            }
            let allowed = !grant.requires_access_consent(state.local_data.root())
                || state
                    .access_consent
                    .authorised_conversation(session.0, record.id, settings, grant);
            if !allowed {
                restricted = true;
            }
            allowed
        })
        .cloned()
        .collect::<Vec<_>>();
    let authority = match ProjectFreeAuthority::from_preview_grants(
        record.revision,
        &grants,
        settings.location,
    ) {
        Ok(authority) => authority,
        Err(_) => {
            return respond(
                graft,
                wants_json(&headers),
                &destination,
                &Search::default(),
                false,
                true,
            );
        }
    };
    let q = match validated_query(&query.q) {
        Ok(q) => q,
        Err(message) => return invalid(graft, wants_json(&headers), &destination, message),
    };
    let roots = effective_roots(&authority.policy, state.local_data.root());
    let no_roots = roots.is_empty();
    let search = match resolve_search(&query.mode, &roots, q, state.local_data.root(), &grants) {
        Ok(search) => search,
        Err(message) => return invalid(graft, wants_json(&headers), &destination, message),
    };
    respond(
        graft,
        wants_json(&headers),
        &destination,
        &search,
        restricted,
        no_roots,
    )
}

pub(super) async fn lookup_new(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: GraftRequest,
    headers: HeaderMap,
    Query(query): Query<DraftFilesQuery>,
) -> AppResult<Response> {
    let destination = "/conversations/new";
    let grants = match query.directories() {
        Ok(grants) => grants,
        Err(message) => return invalid(graft, wants_json(&headers), destination, message),
    };
    let mut restricted = false;
    let grants = grants
        .into_iter()
        .filter(|grant| {
            if grant.revalidate().is_err() {
                restricted = true;
                return false;
            }
            let allowed = !grant.requires_access_consent(state.local_data.root())
                || state.access_consent.authorised_draft_preview(
                    &query.consent_reference,
                    session.0,
                    &query.draft_nonce,
                    grant,
                );
            if !allowed {
                restricted = true;
            }
            allowed
        })
        .collect::<Vec<_>>();
    let authority = match ProjectFreeAuthority::from_preview_grants(0, &grants, query.location()) {
        Ok(authority) => authority,
        Err(_) => {
            return respond(
                graft,
                wants_json(&headers),
                destination,
                &Search::default(),
                false,
                true,
            );
        }
    };
    let q = match validated_query(&query.q) {
        Ok(q) => q,
        Err(message) => return invalid(graft, wants_json(&headers), destination, message),
    };
    let roots = effective_roots(&authority.policy, state.local_data.root());
    let no_roots = roots.is_empty();
    let search = match resolve_search(&query.mode, &roots, q, state.local_data.root(), &grants) {
        Ok(search) => search,
        Err(message) => return invalid(graft, wants_json(&headers), destination, message),
    };
    respond(
        graft,
        wants_json(&headers),
        destination,
        &search,
        restricted,
        no_roots,
    )
}

fn resolve_search(
    mode: &str,
    roots: &[crate::execution::resources::EffectiveRoot],
    query: &str,
    data_root: &std::path::Path,
    grants: &[DirectoryGrant],
) -> Result<page::Search, &'static str> {
    match mode.trim() {
        "complete" => page::complete(roots, query, data_root, grants),
        "" | "search" => Ok(page::search(roots, query, data_root, grants)),
        _ => Err("The file lookup mode is not valid."),
    }
}

fn validated_query(value: &str) -> Result<&str, &'static str> {
    if value.len() > crate::execution::resources::MAXIMUM_FILE_QUERY_BYTES {
        return Err("The file search text is too long.");
    }
    if value.chars().any(char::is_control) {
        return Err("The file search text is not valid.");
    }
    Ok(value)
}

fn wants_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|part| part.trim() == "application/json")
        })
}

fn invalid(
    graft: GraftRequest,
    json: bool,
    redirect: &str,
    message: &'static str,
) -> AppResult<Response> {
    let search = Search::default();
    if json {
        let payload = page::payload(&search, message);
        let mut response = axum::Json(payload).into_response();
        *response.status_mut() = axum::http::StatusCode::UNPROCESSABLE_ENTITY;
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-store"),
        );
        return Ok(response);
    }
    if graft != GraftRequest::Patch {
        return Ok(responses::request_navigation(graft, redirect));
    }
    let view = page::view(&search, message);
    Ok(hypergraft::outcome::children_patch(
        PatchStatus::UnprocessableEntity,
        "conversation-file-suggestions",
        &page::SuggestionsTemplate { view: &view },
    )?)
}

fn respond(
    graft: GraftRequest,
    json: bool,
    redirect: &str,
    search: &Search,
    restricted: bool,
    no_roots: bool,
) -> AppResult<Response> {
    let message = page::explanation(search, restricted, no_roots);
    if json {
        let payload = page::payload(search, message);
        let mut response = axum::Json(payload).into_response();
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-store"),
        );
        return Ok(response);
    }
    if graft == GraftRequest::Patch {
        let view = page::view(search, message);
        let mut patches = hypergraft::PatchSet::new();
        patches.children(
            "conversation-file-suggestions",
            &page::SuggestionsTemplate { view: &view },
        )?;
        return Ok(patches.respond(PatchStatus::Ok)?);
    }
    Ok(responses::request_navigation(graft, redirect))
}
