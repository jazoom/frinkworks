//! Explicit skill suggestions and expansion previews for the composer.
//!
//! The catalogue is read-only. A preview resolves a bounded skill body from
//! the effective read-only source. It never starts a sandbox and never
//! searches a global home directory.

use std::path::Path;

use askama::Template;
use serde::Serialize;

use crate::conversations::input::{self, InputExpansion, InputProvenance, SkillOffer};
use crate::conversations::prompts::{self, PromptTemplate};
use crate::execution::resources::{effective_roots, preview_project_skills};
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
    pub(super) source_label: String,
    pub(super) description: String,
    pub(super) kind: String,
    pub(super) hint: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Preview {
    pub(super) binding: String,
    pub(super) command: String,
    pub(super) scope: String,
    pub(super) source_label: String,
    pub(super) source: String,
    pub(super) hash: String,
    pub(super) base: String,
    pub(super) expanded: String,
    pub(super) kind: String,
    pub(super) hint: String,
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

/// Global prompt templates come from the private data-directory `prompts`
/// folder. An absent folder produces no templates, and no read spends a
/// provider request or starts a sandbox.
pub(super) fn global_templates(state: &AppState) -> (Vec<PromptTemplate>, usize) {
    let catalogue = prompts::discover(state.local_data.root());
    (catalogue.templates, catalogue.unavailable)
}

/// Project resources use authorised live roots.
pub(super) fn project_resources(
    policy: &crate::agents::DirectoryPolicy,
    grants: &[crate::execution::DirectoryGrant],
    data_root: &Path,
) -> (Vec<SkillOffer>, Vec<PromptTemplate>, usize) {
    let roots = effective_roots(policy, data_root);
    let (skills, skills_unavailable) = preview_project_skills(&roots, grants, data_root);
    let (templates, templates_unavailable) = prompts::discover_project(&roots, grants, data_root);
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
    (
        offers,
        templates,
        skills_unavailable + templates_unavailable,
    )
}

/// Bounded prefix autocomplete. A duplicate unqualified skill or template name
/// always inserts the scope-qualified form so selection is never ambiguous.
pub(super) fn suggest(offers: &[SkillOffer], templates: &[PromptTemplate], query: &str) -> Search {
    let trimmed = query.trim_start();
    let skill_only = trimmed.starts_with(input::SKILL_PREFIX) || trimmed == "/skill";
    let (scope, prefix) = query_parts(query);
    let mut suggestions = Vec::new();
    let mut matched = offers
        .iter()
        .filter(|offer| name_starts_with(&offer.name, prefix))
        .filter(|_| skill_only || scope.is_empty())
        .collect::<Vec<_>>();
    matched.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then(left.scope.cmp(&right.scope))
    });
    for offer in matched {
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
            source_label: source_label(&offer.scope),
            scope: offer.scope.clone(),
            description: offer.description.clone(),
            kind: "skill".to_owned(),
            hint: String::new(),
        });
    }
    if !skill_only {
        let mut matched = templates
            .iter()
            .filter(|template| name_starts_with(&template.name, prefix))
            .filter(|template| {
                scope.is_empty() || template.source.scope.eq_ignore_ascii_case(scope)
            })
            .collect::<Vec<_>>();
        matched.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then(left.source.scope.cmp(&right.source.scope))
        });
        for template in matched {
            let duplicates = templates
                .iter()
                .filter(|candidate| candidate.name.eq_ignore_ascii_case(&template.name))
                .count();
            let command = if duplicates > 1 || !scope.is_empty() {
                format!("/{}/{}", template.source.scope, template.name)
            } else {
                format!("/{}", template.name)
            };
            suggestions.push(Suggestion {
                command,
                name: template.name.clone(),
                source_label: source_label(&template.source.scope),
                scope: template.source.scope.clone(),
                description: template.description.clone(),
                kind: "prompt".to_owned(),
                hint: template.argument_hint.clone(),
            });
        }
    }
    suggestions.sort_by(|left, right| {
        left.command
            .to_lowercase()
            .cmp(&right.command.to_lowercase())
    });
    suggestions.truncate(MAXIMUM_COMMAND_RESULTS);
    Search {
        suggestions,
        ..Search::default()
    }
}

/// Resolve one full typed message. A changed source changes only the message;
/// the composer keeps the draft.
pub(super) fn preview(
    offers: &[SkillOffer],
    templates: &[PromptTemplate],
    text: &str,
    secret: Option<&str>,
) -> Search {
    match input::expand(text, offers, templates, secret) {
        Ok(expansion) => Search {
            preview: preview_from(&expansion, templates),
            ..Search::default()
        },
        Err(error) => Search {
            message: error.message().to_owned(),
            ..Search::default()
        },
    }
}

fn preview_from(expansion: &InputExpansion, templates: &[PromptTemplate]) -> Option<Preview> {
    let provenance: &InputProvenance = expansion.provenance.as_ref()?;
    let kind = match provenance.source.kind {
        ResourceKind::Prompt => "prompt",
        _ => "skill",
    };
    let hint = templates
        .iter()
        .find(|template| template.source == provenance.source)
        .map(|template| template.argument_hint.clone())
        .unwrap_or_default();
    Some(Preview {
        binding: provenance.preview_source(),
        command: provenance.typed.clone(),
        source_label: source_label(&provenance.source.scope),
        scope: provenance.source.scope.clone(),
        source: provenance.source.path.clone(),
        hash: provenance.source.content_hash.clone(),
        base: provenance.relative_base.clone(),
        expanded: expansion.expanded.clone(),
        kind: kind.to_owned(),
        hint,
    })
}

fn query_parts(query: &str) -> (&str, &str) {
    let trimmed = query.trim();
    let rest = if let Some(rest) = trimmed.strip_prefix(input::SKILL_PREFIX) {
        rest
    } else if trimmed == "/skill" {
        ""
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

/// A readable source label. Global resources read as `Global`. Project
/// resources keep their directory alias.
fn source_label(scope: &str) -> String {
    if scope.eq_ignore_ascii_case(GLOBAL_SCOPE) {
        "Global".to_owned()
    } else {
        scope.to_owned()
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
        "Some work locations need approval before their skills and templates appear."
    } else if search.unavailable > 0 {
        if no_roots {
            "A command source is not available for previews."
        } else {
            "Some command sources are not available for previews."
        }
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
    pub(super) source_label: String,
    pub(super) description: String,
    pub(super) kind: String,
    pub(super) hint: String,
}

#[derive(Serialize)]
pub(super) struct PreviewPayload {
    pub(super) binding: String,
    pub(super) command: String,
    pub(super) scope: String,
    pub(super) source_label: String,
    pub(super) source: String,
    pub(super) hash: String,
    pub(super) base: String,
    pub(super) expanded: String,
    pub(super) kind: String,
    pub(super) hint: String,
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
    pub(super) source_label: String,
    pub(super) description: String,
    pub(super) kind: String,
    pub(super) hint: String,
}

pub(super) struct PreviewRow {
    pub(super) command: String,
    pub(super) scope: String,
    pub(super) source_label: String,
    pub(super) source: String,
    pub(super) hash: String,
    pub(super) base: String,
    pub(super) expanded: String,
    pub(super) kind: String,
    pub(super) hint: String,
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
                source_label: suggestion.source_label.clone(),
                description: suggestion.description.clone(),
                kind: suggestion.kind.clone(),
                hint: suggestion.hint.clone(),
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
            source_label: preview.source_label.clone(),
            source: preview.source.clone(),
            hash: preview.hash.clone(),
            base: preview.base.clone(),
            expanded: preview.expanded.clone(),
            kind: preview.kind.clone(),
            hint: preview.hint.clone(),
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
                source_label: suggestion.source_label.clone(),
                description: suggestion.description.clone(),
                kind: suggestion.kind.clone(),
                hint: suggestion.hint.clone(),
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
            source_label: preview.source_label.clone(),
            source: preview.source.clone(),
            hash: preview.hash.clone(),
            base: preview.base.clone(),
            expanded: preview.expanded.clone(),
            kind: preview.kind.clone(),
            hint: preview.hint.clone(),
        }),
    }
}
