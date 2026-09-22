//! Explicit skill suggestions and expansion previews for the composer.
//!
//! The catalogue is read-only. A preview resolves a bounded skill body from
//! the effective read-only source. It never starts a sandbox and never
//! searches a global home directory.

use std::path::Path;

use askama::Template;
use serde::Serialize;

use crate::conversations::input::{self, InputExpansion, InputProvenance, SkillOffer};
use crate::execution::resources::{CandidateRoot, effective_roots, preview_project_skills};
use crate::execution::{GUEST_GLOBAL_SKILLS, ResourceKind, ResourceSource};
use crate::state::AppState;

/// Bound autocomplete results independently of the advertised catalogue.
pub(super) const MAXIMUM_COMMAND_RESULTS: usize = 50;
pub(super) const MAXIMUM_COMMAND_QUERY_BYTES: usize = 512;
pub(super) const GLOBAL_SCOPE: &str = "global";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Suggestion {
    pub(super) command: String,
    pub(super) name: String,
    pub(super) scope: String,
    pub(super) description: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Preview {
    pub(super) binding: String,
    pub(super) command: String,
    pub(super) scope: String,
    pub(super) source: String,
    pub(super) hash: String,
    pub(super) base: String,
    pub(super) expanded: String,
}

#[derive(Default)]
pub(super) struct Search {
    pub(super) suggestions: Vec<Suggestion>,
    pub(super) unavailable: usize,
    pub(super) message: String,
    pub(super) preview: Option<Preview>,
}

/// Global skills come from the private store. A store failure hides global
/// skills and reports the limitation instead of an empty silent catalogue.
pub(super) fn global_offers(
    state: &AppState,
    location: crate::execution::ToolLocation,
) -> (Vec<SkillOffer>, usize) {
    let base = if location == crate::execution::ToolLocation::Host {
        let Some(directory) = state.skills.host_dir().and_then(|path| path.to_str()) else {
            return (Vec::new(), 1);
        };
        directory
    } else {
        GUEST_GLOBAL_SKILLS
    };
    match state.skills.list() {
        Ok(records) => (
            records
                .into_iter()
                .map(|record| global_offer(record, base))
                .collect(),
            0,
        ),
        Err(_) => (Vec::new(), 1),
    }
}

fn global_offer(record: crate::skills::SkillRecord, base: &str) -> SkillOffer {
    let path = format!("{base}/{}/SKILL.md", record.directory);
    let source = ResourceSource::new(
        ResourceKind::Skill,
        "Global skills",
        path.as_str(),
        record.markdown.as_bytes(),
    );
    SkillOffer {
        scope: GLOBAL_SCOPE.to_owned(),
        name: record.name,
        description: record.description,
        body: record.markdown,
        source,
        relative_base: format!("{base}/{}", record.directory),
    }
}

/// Project skills come from the effective read-only roots. A candidate-backed
/// root has no readable body, so it stays unavailable rather than substituting
/// the current host files.
pub(super) fn project_offers(
    policy: &crate::agents::DirectoryPolicy,
    grants: &[crate::execution::DirectoryGrant],
    candidates: &[CandidateRoot],
    data_root: &Path,
) -> (Vec<SkillOffer>, usize) {
    let roots = effective_roots(policy, candidates, data_root);
    let (skills, unavailable) = preview_project_skills(&roots, grants, data_root);
    let offers = skills
        .into_iter()
        .map(|skill| {
            let scope = skill.scope.clone();
            SkillOffer {
                scope: skill.scope,
                name: skill.name,
                description: skill.description,
                body: skill.body,
                source: ResourceSource {
                    kind: ResourceKind::Skill,
                    scope,
                    path: skill.path,
                    content_hash: skill.content_hash,
                },
                relative_base: skill.relative_base,
            }
        })
        .collect();
    (offers, unavailable)
}

/// Bounded prefix autocomplete. A duplicate unqualified name always inserts
/// the qualified form so selection is never ambiguous.
pub(super) fn suggest(offers: &[SkillOffer], query: &str) -> Search {
    let (scope, prefix) = query_parts(query);
    let mut matched = offers
        .iter()
        .filter(|offer| {
            name_starts_with(&offer.name, prefix)
                && (scope.is_empty() || offer.scope.starts_with(scope))
        })
        .collect::<Vec<_>>();
    matched.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then(left.scope.cmp(&right.scope))
    });
    let mut suggestions = Vec::new();
    for offer in matched {
        if suggestions.len() >= MAXIMUM_COMMAND_RESULTS {
            break;
        }
        let duplicates = offers
            .iter()
            .filter(|candidate| candidate.name == offer.name)
            .count();
        let command = if duplicates > 1 {
            format!("/skill:{}/{}", offer.scope, offer.name)
        } else {
            format!("/skill:{}", offer.name)
        };
        suggestions.push(Suggestion {
            command,
            name: offer.name.clone(),
            scope: offer.scope.clone(),
            description: offer.description.clone(),
        });
    }
    Search {
        suggestions,
        ..Search::default()
    }
}

/// Resolve one full typed message. A changed source changes only the message;
/// the composer keeps the draft.
pub(super) fn preview(offers: &[SkillOffer], text: &str, secret: Option<&str>) -> Search {
    match input::expand(text, offers, secret) {
        Ok(expansion) => Search {
            preview: preview_from(&expansion),
            ..Search::default()
        },
        Err(error) => Search {
            message: error.message().to_owned(),
            ..Search::default()
        },
    }
}

fn preview_from(expansion: &InputExpansion) -> Option<Preview> {
    let provenance: &InputProvenance = expansion.provenance.as_ref()?;
    Some(Preview {
        binding: provenance.preview_source(),
        command: provenance.typed.clone(),
        scope: provenance.source.scope.clone(),
        source: provenance.source.path.clone(),
        hash: provenance.source.content_hash.clone(),
        base: provenance.relative_base.clone(),
        expanded: expansion.expanded.clone(),
    })
}

fn query_parts(query: &str) -> (&str, &str) {
    let trimmed = query.trim();
    let rest = if let Some(rest) = trimmed.strip_prefix(input::SKILL_PREFIX) {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("/skill") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix('/') {
        rest
    } else {
        trimmed
    };
    let rest = rest.trim_start();
    match rest.split_once('/') {
        Some((scope, name)) => (scope.trim(), name.trim_start()),
        None => ("", rest),
    }
}

fn name_starts_with(name: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    name.to_lowercase().starts_with(&prefix.to_lowercase())
}

pub(super) fn explanation(search: &Search, restricted: bool, no_roots: bool) -> &'static str {
    if !search.message.is_empty() {
        return "";
    }
    if restricted {
        "Some work locations need approval before their skills appear."
    } else if no_roots && search.unavailable > 0 {
        "A work location is not available for skill previews."
    } else {
        ""
    }
}

#[derive(Serialize)]
pub(super) struct SuggestionsPayload {
    pub(super) suggestions: Vec<SuggestionPayload>,
    pub(super) unavailable: usize,
    pub(super) message: String,
    pub(super) preview: Option<PreviewPayload>,
}

#[derive(Serialize)]
pub(super) struct SuggestionPayload {
    pub(super) command: String,
    pub(super) name: String,
    pub(super) scope: String,
    pub(super) description: String,
}

#[derive(Serialize)]
pub(super) struct PreviewPayload {
    pub(super) binding: String,
    pub(super) command: String,
    pub(super) scope: String,
    pub(super) source: String,
    pub(super) hash: String,
    pub(super) base: String,
    pub(super) expanded: String,
}

pub(super) struct SuggestionsView {
    pub(super) suggestions: Vec<SuggestionRow>,
    pub(super) message: String,
    pub(super) preview: Option<PreviewRow>,
}

pub(super) struct SuggestionRow {
    pub(super) command: String,
    pub(super) name: String,
    pub(super) scope: String,
    pub(super) description: String,
}

pub(super) struct PreviewRow {
    pub(super) command: String,
    pub(super) scope: String,
    pub(super) source: String,
    pub(super) hash: String,
    pub(super) base: String,
    pub(super) expanded: String,
}

#[derive(Template)]
#[template(path = "conversations/commands/templates/index.html")]
pub(super) struct SuggestionsTemplate<'a> {
    pub(super) view: &'a SuggestionsView,
}

pub(super) fn payload(search: &Search, message: &str) -> SuggestionsPayload {
    SuggestionsPayload {
        suggestions: search
            .suggestions
            .iter()
            .map(|suggestion| SuggestionPayload {
                command: suggestion.command.clone(),
                name: suggestion.name.clone(),
                scope: suggestion.scope.clone(),
                description: suggestion.description.clone(),
            })
            .collect(),
        unavailable: search.unavailable,
        message: if message.is_empty() {
            search.message.clone()
        } else {
            message.to_owned()
        },
        preview: search.preview.as_ref().map(|preview| PreviewPayload {
            binding: preview.binding.clone(),
            command: preview.command.clone(),
            scope: preview.scope.clone(),
            source: preview.source.clone(),
            hash: preview.hash.clone(),
            base: preview.base.clone(),
            expanded: preview.expanded.clone(),
        }),
    }
}

pub(super) fn view(search: &Search, message: &str) -> SuggestionsView {
    SuggestionsView {
        suggestions: search
            .suggestions
            .iter()
            .map(|suggestion| SuggestionRow {
                command: suggestion.command.clone(),
                name: suggestion.name.clone(),
                scope: suggestion.scope.clone(),
                description: suggestion.description.clone(),
            })
            .collect(),
        message: if message.is_empty() {
            search.message.clone()
        } else {
            message.to_owned()
        },
        preview: search.preview.as_ref().map(|preview| PreviewRow {
            command: preview.command.clone(),
            scope: preview.scope.clone(),
            source: preview.source.clone(),
            hash: preview.hash.clone(),
            base: preview.base.clone(),
            expanded: preview.expanded.clone(),
        }),
    }
}
