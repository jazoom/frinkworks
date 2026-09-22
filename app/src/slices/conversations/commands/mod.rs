//! Explicit skill commands for saved conversations and unsaved drafts.
//!
//! The routes are read-only previews. They resolve approved roots through the
//! effective-source resolver, never start a sandbox and never return an
//! unapproved project body. The same catalogue builder serves session
//! submission, so a preview and a send agree on the source identity.

pub(super) mod page;
pub(super) mod run;

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
    conversations::{ConversationId, ConversationModelConfiguration, PromptTemplate, SkillOffer},
    error::AppResult,
    execution::{
        DirectoryGrant, ProjectFreeAuthority, ToolLocation, resources::CandidateRoot,
        validate_directories,
    },
    responses,
    sessions::RequiredSession,
    state::AppState,
};

use self::page::Search;

/// The resolved skill catalogue and the limitations of its sources.
#[derive(Default)]
pub(super) struct Catalogue {
    pub(super) offers: Vec<SkillOffer>,
    pub(super) templates: Vec<PromptTemplate>,
    pub(super) unavailable: usize,
    pub(super) restricted: bool,
    pub(super) no_roots: bool,
}

impl Catalogue {
    fn without_credentials(mut self, state: &AppState) -> Self {
        for (provider, auth) in state.vault.providers() {
            if auth != crate::providers::AuthMethod::ApiKey {
                continue;
            }
            let selection = crate::providers::ModelSelection {
                provider,
                model: String::new(),
                thinking: None,
            };
            let Some(secret) =
                super::provider_secret(state, &selection).filter(|secret| !secret.is_empty())
            else {
                continue;
            };
            let before = self.offers.len();
            self.offers.retain(|offer| {
                ![
                    &offer.name,
                    &offer.description,
                    &offer.body,
                    &offer.source.scope,
                    &offer.source.path,
                    &offer.relative_base,
                ]
                .iter()
                .any(|value| value.contains(&secret))
            });
            let before_templates = self.templates.len();
            self.templates.retain(|template| {
                ![
                    &template.name,
                    &template.description,
                    &template.argument_hint,
                    &template.body,
                    &template.source.scope,
                    &template.source.path,
                ]
                .iter()
                .any(|value| value.contains(&secret))
            });
            self.unavailable +=
                (before - self.offers.len()) + (before_templates - self.templates.len());
        }
        self
    }

    fn search(&self, query: &str, mode: &str, secret: Option<&str>) -> Search {
        let mut search = match mode.trim() {
            "preview" => page::preview(&self.offers, &self.templates, query, secret),
            "" | "suggest" => page::suggest(&self.offers, &self.templates, query),
            _ => Search {
                message: "The command lookup mode is not valid.".to_owned(),
                ..Search::default()
            },
        };
        // Surface a candidate-backed or unreadable source as a limitation
        // instead of a silent empty catalogue.
        search.unavailable = search.unavailable.saturating_add(self.unavailable);
        search
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct CommandsQuery {
    q: String,
    mode: String,
}

/// The draft lookup carries only the fields that resolve roots. The composer
/// form already holds these values, so no server-side draft record is added.
#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct DraftCommandsQuery {
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

impl DraftCommandsQuery {
    pub(super) fn directories(&self) -> Result<Vec<DirectoryGrant>, &'static str> {
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

    pub(super) fn location(&self) -> ToolLocation {
        ToolLocation::parse(self.location.trim()).unwrap_or(ToolLocation::Sandbox)
    }
}

/// Build the read-only catalogue for one saved conversation. A grant without
/// approval stays out of the catalogue instead of hiding behind a host path.
pub(super) fn catalogue_for_record(
    state: &AppState,
    session: crate::sessions::SessionId,
    id: ConversationId,
    revision: u32,
    model: Option<&ConversationModelConfiguration>,
) -> Catalogue {
    let mut catalogue = Catalogue::default();
    fill_templates(&mut catalogue, state);
    let Some(model) = model else {
        let (offers, unavailable) = page::global_offers(state, ToolLocation::Sandbox);
        catalogue.offers = offers;
        catalogue.unavailable += unavailable;
        catalogue.no_roots = true;
        return catalogue.without_credentials(state);
    };
    let settings = &model.settings;
    let grants = settings
        .directories
        .iter()
        .filter(|grant| {
            if grant.revalidate().is_err() {
                catalogue.restricted = true;
                return false;
            }
            let allowed = !grant.requires_access_consent(state.local_data.root())
                || state
                    .access_consent
                    .authorised_conversation(session, id, settings, grant);
            if !allowed {
                catalogue.restricted = true;
            }
            allowed
        })
        .cloned()
        .collect::<Vec<_>>();
    let candidates = match super::files::candidate_roots(state, id) {
        Ok(candidates) => candidates,
        Err(()) => {
            let (offers, unavailable) = page::global_offers(state, settings.location);
            catalogue.offers = offers;
            catalogue.unavailable += unavailable + 1;
            return catalogue.without_credentials(state);
        }
    };
    fill(
        &mut catalogue,
        state,
        settings.location,
        revision,
        &grants,
        &candidates,
    );
    catalogue.without_credentials(state)
}

/// Build the read-only catalogue for one unsaved draft from its submitted
/// grants and consent reference.
pub(super) fn catalogue_for_draft(
    state: &AppState,
    session: crate::sessions::SessionId,
    grants: &[DirectoryGrant],
    consent_reference: &str,
    draft_nonce: &str,
    location: ToolLocation,
) -> Catalogue {
    let mut catalogue = Catalogue::default();
    fill_templates(&mut catalogue, state);
    let grants = grants
        .iter()
        .filter(|grant| {
            if grant.revalidate().is_err() {
                catalogue.restricted = true;
                return false;
            }
            let allowed = !grant.requires_access_consent(state.local_data.root())
                || state.access_consent.authorised_draft_preview(
                    consent_reference,
                    session,
                    draft_nonce,
                    grant,
                );
            if !allowed {
                catalogue.restricted = true;
            }
            allowed
        })
        .cloned()
        .collect::<Vec<_>>();
    fill(&mut catalogue, state, location, 0, &grants, &[]);
    catalogue.without_credentials(state)
}

fn fill_templates(catalogue: &mut Catalogue, state: &AppState) {
    let (templates, unavailable) = page::global_templates(state);
    catalogue.templates = templates;
    catalogue.unavailable += unavailable;
}

fn fill(
    catalogue: &mut Catalogue,
    state: &AppState,
    location: ToolLocation,
    revision: u32,
    grants: &[DirectoryGrant],
    candidates: &[CandidateRoot],
) {
    let (offers, unavailable) =
        match ProjectFreeAuthority::from_preview_grants(revision, grants, location) {
            Ok(authority) => {
                let (mut offers, mut unavailable) = page::global_offers(state, location);
                let (project, project_templates, project_unavailable) = page::project_resources(
                    &authority.policy,
                    grants,
                    candidates,
                    state.local_data.root(),
                );
                offers.extend(project);
                catalogue.templates.extend(project_templates);
                unavailable += project_unavailable;
                (offers, unavailable)
            }
            Err(_) => {
                catalogue.restricted = true;
                page::global_offers(state, location)
            }
        };
    catalogue.no_roots = offers.is_empty() && catalogue.templates.is_empty();
    catalogue.offers = offers;
    catalogue.unavailable += unavailable;
}

/// Resolve one submitted message against the catalogue. This is the single
/// entry point that submission uses, so an unpreviewed command still comes
/// from one validated source snapshot.
pub(super) fn expand(
    message: &str,
    catalogue: &Catalogue,
    secret: Option<&str>,
    preview: Option<(&str, &str)>,
) -> Result<crate::conversations::InputExpansion, crate::conversations::InputError> {
    crate::conversations::input::expand_checked(
        message,
        &catalogue.offers,
        &catalogue.templates,
        secret,
        preview,
    )
}

/// Read the frozen preview binding from a submitted form. The binding is
/// optional: an unpreviewed command resolves from the current snapshot.
pub(super) fn preview_binding<'a>(source: &'a str, hash: &'a str) -> Option<(&'a str, &'a str)> {
    let source = source.trim();
    let hash = hash.trim();
    if source.is_empty() && hash.is_empty() {
        None
    } else {
        Some((source, hash))
    }
}

pub(super) async fn lookup_saved(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: GraftRequest,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    Query(query): Query<CommandsQuery>,
) -> AppResult<Response> {
    let Some(id) = ConversationId::parse(&conversation_id) else {
        return Ok(responses::request_navigation(graft, "/conversations"));
    };
    let Some(record) = state.conversations.metadata_for(&id) else {
        return Ok(responses::request_navigation(graft, "/conversations"));
    };
    let destination = format!("/conversations/{}", id.as_hex());
    let q = match validated_query(&query.q) {
        Ok(q) => q,
        Err(message) => return invalid(graft, wants_json(&headers), &destination, message),
    };
    let catalogue = catalogue_for_record(
        &state,
        session.0,
        record.id,
        record.revision,
        record.model.as_ref(),
    );
    let secret = record
        .model
        .as_ref()
        .and_then(|model| super::provider_secret(&state, &model.settings.model));
    let search = catalogue.search(q, &query.mode, secret.as_deref());
    respond(
        graft,
        wants_json(&headers),
        &destination,
        &search,
        catalogue.restricted,
        catalogue.no_roots,
    )
}

pub(super) async fn lookup_new(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: GraftRequest,
    headers: HeaderMap,
    Query(query): Query<DraftCommandsQuery>,
) -> AppResult<Response> {
    let destination = "/conversations/new";
    let grants = match query.directories() {
        Ok(grants) => grants,
        Err(message) => return invalid(graft, wants_json(&headers), destination, message),
    };
    let q = match validated_query(&query.q) {
        Ok(q) => q,
        Err(message) => return invalid(graft, wants_json(&headers), destination, message),
    };
    let catalogue = catalogue_for_draft(
        &state,
        session.0,
        &grants,
        &query.consent_reference,
        &query.draft_nonce,
        query.location(),
    );
    let search = catalogue.search(q, &query.mode, None);
    respond(
        graft,
        wants_json(&headers),
        destination,
        &search,
        catalogue.restricted,
        catalogue.no_roots,
    )
}

fn validated_query(value: &str) -> Result<&str, &'static str> {
    if value.len() > page::MAXIMUM_COMMAND_QUERY_BYTES {
        return Err("The command text is too long.");
    }
    if value.chars().any(char::is_control) {
        return Err("The command text is not valid.");
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
    let search = Search {
        message: message.to_owned(),
        ..Search::default()
    };
    if json {
        let payload = page::payload(&search, "");
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
    let view = page::view(&search, "");
    Ok(hypergraft::outcome::children_patch(
        PatchStatus::UnprocessableEntity,
        "conversation-command-suggestions",
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
    let explanation = page::explanation(search, restricted, no_roots);
    if json {
        let payload = page::payload(search, explanation);
        let mut response = axum::Json(payload).into_response();
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-store"),
        );
        return Ok(response);
    }
    if graft == GraftRequest::Patch {
        let view = page::view(search, explanation);
        let mut patches = hypergraft::PatchSet::new();
        patches.children(
            "conversation-command-suggestions",
            &page::SuggestionsTemplate { view: &view },
        )?;
        return Ok(patches.respond(PatchStatus::Ok)?);
    }
    Ok(responses::request_navigation(graft, redirect))
}
