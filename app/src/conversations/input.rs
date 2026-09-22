//! Deterministic composer input classification and skill expansion.
//!
//! The parser is pure. It never reads a skill source. A caller resolves the
//! catalogue and passes validated offers. The leading-character escape and the
//! command syntax run here, so classification stays a submission-time
//! decision.

use serde::{Deserialize, Serialize};

use crate::execution::resources::MAXIMUM_RESOURCE_PATH_BYTES;
use crate::execution::{ResourceKind, ResourceSource};

#[cfg(test)]
mod tests;

pub(crate) const SKILL_PREFIX: &str = "/skill:";

/// A resolved, bounded skill source that the input resolver can expand. One
/// offer is a validated snapshot of a global or project skill.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SkillOffer {
    /// The selector used by `/skill:scope/name`. Global skills use `global`.
    pub(crate) scope: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) body: String,
    /// Immutable source identity and body hash.
    pub(crate) source: ResourceSource,
    /// Model-visible base path for relative references inside the skill.
    pub(crate) relative_base: String,
}

/// Frozen command provenance. The expanded text is the model-visible message
/// body. The typed text stays for display and later resource selection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct InputProvenance {
    pub(crate) typed: String,
    pub(crate) expanded: String,
    pub(crate) source: ResourceSource,
    pub(crate) relative_base: String,
}

impl InputProvenance {
    pub(crate) fn preview_source(&self) -> String {
        serde_json::to_string(&(&self.source.scope, &self.source.path, &self.relative_base))
            .expect("source identity serialises")
    }

    pub(crate) fn valid(&self) -> bool {
        !self.typed.is_empty()
            && self.typed.len() <= super::MAXIMUM_MESSAGE_BYTES
            && !self.typed.contains('\0')
            && !self
                .typed
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
            && !self.expanded.is_empty()
            && self.expanded.len() <= super::MAXIMUM_MESSAGE_BYTES
            && !self.expanded.contains('\0')
            && self.source.valid()
            && self.source.kind == ResourceKind::Skill
            && !self.relative_base.is_empty()
            && self.relative_base.len() <= MAXIMUM_RESOURCE_PATH_BYTES
            && !self.relative_base.chars().any(char::is_control)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InputExpansion {
    pub(crate) typed: String,
    pub(crate) expanded: String,
    pub(crate) provenance: Option<InputProvenance>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InputError {
    Unknown,
    Ambiguous,
    Empty,
    Bound,
    Credential,
    Changed,
}

impl InputError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Unknown => "No skill matches that command. Check the name and try again.",
            Self::Ambiguous => {
                "More than one skill uses that name. Use /skill:scope/name to choose one."
            }
            Self::Empty => "Enter a skill name after /skill:.",
            Self::Bound => "The expanded message is too large. Use a smaller skill or message.",
            Self::Credential => "That skill contains a provider credential and cannot be used.",
            Self::Changed => {
                "That skill changed after the preview. Select it again before you send."
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum InputSyntax {
    /// Literal text. An escaped prefix has its escape removed.
    Literal(String),
    /// An explicit skill command plus trailing instructions.
    Skill {
        reference: String,
        instructions: String,
    },
}

/// Classify one message. The first non-whitespace character decides the
/// syntax. A backslash directly before a leading `/` or `!` escapes the prefix.
pub(crate) fn classify(text: &str) -> InputSyntax {
    if let Some(literal) = unescape_prefix(text) {
        return InputSyntax::Literal(literal);
    }
    let trimmed = text.trim_start();
    if let Some(rest) = trimmed.strip_prefix(SKILL_PREFIX) {
        let (reference, instructions) = split_reference(rest);
        return InputSyntax::Skill {
            reference: reference.to_owned(),
            instructions: instructions.to_owned(),
        };
    }
    InputSyntax::Literal(text.to_owned())
}

/// Resolve one message against a validated offer snapshot. An unpreviewed
/// command resolves here. An unknown or ambiguous command leaves the draft
/// untouched because this function reads no durable state.
pub(crate) fn expand(
    text: &str,
    offers: &[SkillOffer],
    secret: Option<&str>,
) -> Result<InputExpansion, InputError> {
    if text.trim_start().starts_with('/') && !text.trim_start().starts_with(SKILL_PREFIX) {
        return Err(InputError::Unknown);
    }
    match classify(text) {
        InputSyntax::Literal(literal) => Ok(InputExpansion {
            typed: text.to_owned(),
            expanded: literal,
            provenance: None,
        }),
        InputSyntax::Skill {
            reference,
            instructions,
        } => {
            let offer = select(&reference, offers)?;
            validate_offer(offer, secret)?;
            let expanded = compose(&offer.body, &instructions, &offer.relative_base);
            if expanded.is_empty() || expanded.len() > super::MAXIMUM_MESSAGE_BYTES {
                return Err(InputError::Bound);
            }
            let provenance = InputProvenance {
                typed: text.to_owned(),
                expanded: expanded.clone(),
                source: offer.source.clone(),
                relative_base: offer.relative_base.clone(),
            };
            if !provenance.valid() {
                return Err(InputError::Bound);
            }
            Ok(InputExpansion {
                typed: text.to_owned(),
                expanded,
                provenance: Some(provenance),
            })
        }
    }
}

/// Escape a leading command prefix so restored literal text is never
/// reinterpreted as a command. The escape survives one round trip through the
/// resolver and keeps the visible text unchanged.
pub(crate) fn escape_leading(text: &str) -> String {
    let Some(index) = text.find(|character: char| !character.is_whitespace()) else {
        return text.to_owned();
    };
    let Some(leading) = text[index..].chars().next() else {
        return text.to_owned();
    };
    if leading != '/' && leading != '!' {
        return text.to_owned();
    }
    let mut escaped = String::with_capacity(text.len() + 1);
    escaped.push_str(&text[..index]);
    escaped.push('\\');
    escaped.push_str(&text[index..]);
    escaped
}

/// Expand one message and reject a preview whose source identity or body hash
/// no longer matches the current snapshot.
pub(crate) fn expand_checked(
    text: &str,
    offers: &[SkillOffer],
    secret: Option<&str>,
    preview: Option<(&str, &str)>,
) -> Result<InputExpansion, InputError> {
    let expansion = expand(text, offers, secret)?;
    let Some((path, hash)) = preview else {
        return Ok(expansion);
    };
    match &expansion.provenance {
        Some(provenance)
            if provenance.preview_source() == path && provenance.source.content_hash == hash =>
        {
            Ok(expansion)
        }
        _ => Err(InputError::Changed),
    }
}

fn unescape_prefix(text: &str) -> Option<String> {
    let start = text.find(|character: char| !character.is_whitespace())?;
    let after = &text[start..];
    if !after.starts_with('\\') {
        return None;
    }
    let rest = &after[1..];
    let escaped = rest.chars().next()?;
    if escaped != '/' && escaped != '!' {
        return None;
    }
    let mut literal = String::with_capacity(text.len() - 1);
    literal.push_str(&text[..start]);
    literal.push_str(rest);
    Some(literal)
}

fn split_reference(rest: &str) -> (&str, &str) {
    let rest = rest.trim_start();
    match rest.find(char::is_whitespace) {
        Some(index) => (&rest[..index], rest[index..].trim_start()),
        None => (rest, ""),
    }
}

fn select<'a>(reference: &str, offers: &'a [SkillOffer]) -> Result<&'a SkillOffer, InputError> {
    let reference = reference.trim();
    if reference.is_empty() {
        return Err(InputError::Empty);
    }
    match reference.split_once('/') {
        Some((scope, name)) => {
            let mut matches = offers
                .iter()
                .filter(|offer| offer.scope == scope && offer.name == name);
            let first = matches.next().ok_or(InputError::Unknown)?;
            if matches.next().is_some() {
                return Err(InputError::Ambiguous);
            }
            Ok(first)
        }
        None => {
            let matches = offers
                .iter()
                .filter(|offer| offer.name == reference)
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [] => Err(InputError::Unknown),
                [only] => Ok(only),
                _ => Err(InputError::Ambiguous),
            }
        }
    }
}

fn validate_offer(offer: &SkillOffer, secret: Option<&str>) -> Result<(), InputError> {
    if offer.source.kind != ResourceKind::Skill || !offer.source.valid() {
        return Err(InputError::Unknown);
    }
    if offer.body.len() > crate::execution::resources::MAXIMUM_SKILL_BODY_BYTES {
        return Err(InputError::Bound);
    }
    if let Some(secret) = secret.filter(|secret| !secret.is_empty())
        && (offer.source.path.contains(secret)
            || offer.source.scope.contains(secret)
            || offer.relative_base.contains(secret)
            || offer.body.contains(secret))
    {
        return Err(InputError::Credential);
    }
    Ok(())
}

fn compose(body: &str, instructions: &str, relative_base: &str) -> String {
    let mut expanded = format!("Skill directory: {relative_base}\n\n{}", body.trim());
    let instructions = instructions.trim();
    if !instructions.is_empty() {
        if !expanded.is_empty() {
            expanded.push_str("\n\n");
        }
        expanded.push_str(instructions);
    }
    expanded
}
