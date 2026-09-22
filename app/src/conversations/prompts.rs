//! Prompt templates with non-recursive argument substitution.
//!
//! Global discovery reads bounded Markdown files directly inside the Power
//! Plant data-directory `prompts` folder. Project discovery reads
//! `.agents/prompts/*.md` directly below an authorised work location. The
//! parser and renderer are pure. Template text is never executed and
//! expansion output never runs command classification again.

use std::{collections::BTreeSet, io, path::Path};

use serde::Deserialize;

use crate::execution::resources::{EffectiveRoot, open_preview_directory, read_preview_file};
use crate::execution::{DirectoryGrant, ResourceKind, ResourceSource};

#[cfg(test)]
mod tests;

pub(crate) const PROMPTS_DIRECTORY: &str = "prompts";
pub(crate) const PROJECT_PROMPTS_DIRECTORY: &str = ".agents/prompts";
pub(crate) const PROMPTS_SCOPE: &str = "global";
pub(crate) const MAXIMUM_PROMPTS: usize = 64;
pub(crate) const MAXIMUM_PROMPT_ENTRIES: usize = 256;
pub(crate) const MAXIMUM_PROMPT_NAME_BYTES: usize = 128;
pub(crate) const MAXIMUM_PROMPT_DESCRIPTION_BYTES: usize = 4 * 1024;
pub(crate) const MAXIMUM_PROMPT_ARGUMENT_HINT_BYTES: usize = 256;
pub(crate) const MAXIMUM_PROMPT_BODY_BYTES: usize = 64 * 1024;
pub(crate) const MAXIMUM_PROMPT_ARGUMENTS: usize = 128;
pub(crate) const PROMPT_EXTENSION: &str = "md";

/// `skill:` is the reserved application command prefix. A template cannot
/// claim the `skill` name, so `/skill` and `/skill:name` stay unambiguous.
pub(crate) const RESERVED_COMMAND_NAMES: &[&str] = &["skill"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PromptTemplate {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) argument_hint: String,
    pub(crate) body: String,
    pub(crate) source: ResourceSource,
}

#[derive(Default)]
pub(crate) struct PromptCatalogue {
    pub(crate) templates: Vec<PromptTemplate>,
    pub(crate) unavailable: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PromptError {
    Malformed,
    Bound,
}

pub(crate) fn discover(root: &Path) -> PromptCatalogue {
    let directory = root.join(PROMPTS_DIRECTORY);
    let metadata = match std::fs::symlink_metadata(&directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return PromptCatalogue::default(),
        Err(_) => {
            return PromptCatalogue {
                unavailable: 1,
                ..PromptCatalogue::default()
            };
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return PromptCatalogue {
            unavailable: 1,
            ..PromptCatalogue::default()
        };
    }
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(_) => {
            return PromptCatalogue {
                unavailable: 1,
                ..PromptCatalogue::default()
            };
        }
    };
    let mut catalogue = PromptCatalogue::default();
    let mut seen = std::collections::BTreeSet::new();
    let mut duplicates = std::collections::BTreeSet::new();
    for (index, entry) in entries.enumerate() {
        if index >= MAXIMUM_PROMPT_ENTRIES {
            catalogue.unavailable += 1;
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                catalogue.unavailable += 1;
                continue;
            }
        };
        let Some(template) = template_from_entry(&directory, &entry) else {
            catalogue.unavailable += 1;
            continue;
        };
        let name = template.name.to_ascii_lowercase();
        if !seen.insert(name.clone()) {
            duplicates.insert(name);
            catalogue.unavailable += 1;
        } else if catalogue.templates.len() >= MAXIMUM_PROMPTS {
            catalogue.unavailable += 1;
        } else {
            catalogue.templates.push(template);
        }
    }
    // Reject every member of a collision, independent of directory order.
    catalogue.templates.retain(|template| {
        let unique = !duplicates.contains(&template.name.to_ascii_lowercase());
        if !unique {
            catalogue.unavailable += 1;
        }
        unique
    });
    catalogue
        .templates
        .sort_by(|left, right| left.name.cmp(&right.name));
    catalogue
}

fn template_from_entry(directory: &Path, entry: &std::fs::DirEntry) -> Option<PromptTemplate> {
    let file_type = entry.file_type().ok()?;
    if file_type.is_symlink() || !file_type.is_file() {
        return None;
    }
    let file_name = entry.file_name();
    let file_name = file_name.to_str()?;
    let name = file_name.strip_suffix(&format!(".{PROMPT_EXTENSION}"))?;
    if !valid_name(name) || reserved_name(name) {
        return None;
    }
    if entry.metadata().ok()?.len() > MAXIMUM_PROMPT_BODY_BYTES as u64 {
        return None;
    }
    let path = directory.join(file_name);
    let bytes = crate::storage::read_private_bounded(&path, MAXIMUM_PROMPT_BODY_BYTES).ok()?;
    template_from_bytes(
        name,
        &bytes,
        PROMPTS_SCOPE,
        format!("{PROMPTS_DIRECTORY}/{file_name}"),
    )
}

fn template_from_bytes(
    name: &str,
    bytes: &[u8],
    scope: &str,
    path: String,
) -> Option<PromptTemplate> {
    if bytes.len() > MAXIMUM_PROMPT_BODY_BYTES {
        return None;
    }
    let markdown = std::str::from_utf8(bytes).ok()?;
    let document = parse_document(markdown).ok()?;
    Some(PromptTemplate {
        name: name.to_owned(),
        description: document.description,
        argument_hint: document.argument_hint,
        body: document.body,
        source: ResourceSource::new(ResourceKind::Prompt, scope, path, bytes),
    })
}

/// Discover project templates from the effective read-only roots. Discovery
/// reads `.agents/prompts` directly below each root. It never scans a nested
/// directory or an external application configuration. A candidate-backed
/// root resolves names from the immutable candidate entries and reports an
/// unavailable body instead of reading current host files.
pub(crate) fn discover_project(
    roots: &[EffectiveRoot],
    grants: &[DirectoryGrant],
    data_root: &Path,
) -> (Vec<PromptTemplate>, usize) {
    let mut templates = Vec::new();
    let mut unavailable = 0;
    for root in roots {
        if templates.len() >= MAXIMUM_PROMPTS {
            break;
        }
        let Some(host) = root.host_path.as_deref() else {
            unavailable += project_candidate_count(&root.candidate_paths);
            continue;
        };
        let Some(grant) = grants
            .iter()
            .find(|grant| grant.host_path.as_path() == host)
        else {
            unavailable += 1;
            continue;
        };
        read_project_root(
            root,
            host,
            grant,
            data_root,
            &mut templates,
            &mut unavailable,
        );
    }
    templates.sort_by(|left, right| {
        left.source
            .scope
            .cmp(&right.source.scope)
            .then(left.name.cmp(&right.name))
    });
    (templates, unavailable)
}

/// Count the distinct `.agents/prompts/*.md` names in one immutable candidate.
/// The body bytes are not part of that source, so each name stays unavailable.
fn project_candidate_count(entries: &[String]) -> usize {
    let mut seen = BTreeSet::new();
    for entry in entries {
        let Some(rest) = entry.strip_prefix(&format!("{PROJECT_PROMPTS_DIRECTORY}/")) else {
            continue;
        };
        if rest.contains('/') {
            continue;
        }
        let Some(name) = rest.strip_suffix(&format!(".{PROMPT_EXTENSION}")) else {
            continue;
        };
        if !valid_name(name) || reserved_name(name) {
            continue;
        }
        seen.insert(name.to_ascii_lowercase());
    }
    seen.len()
}

fn read_project_root(
    root: &EffectiveRoot,
    host: &Path,
    grant: &DirectoryGrant,
    data_root: &Path,
    templates: &mut Vec<PromptTemplate>,
    unavailable: &mut usize,
) {
    use cap_std::fs::MetadataExt;
    let prompts_path = host.join(PROJECT_PROMPTS_DIRECTORY);
    // An authorised parent must not expose private data through its prompt directory.
    if prompts_path.starts_with(data_root) || data_root.starts_with(&prompts_path) {
        return;
    }
    let Ok(directory) = cap_std::fs::Dir::open_ambient_dir(host, cap_std::ambient_authority())
    else {
        *unavailable += 1;
        return;
    };
    if grant.revalidate().is_err()
        || !directory.dir_metadata().is_ok_and(|metadata| {
            metadata.dev() == grant.identity.device && metadata.ino() == grant.identity.inode
        })
    {
        *unavailable += 1;
        return;
    }
    let Some(directory) = open_preview_directory(&directory, ".agents")
        .and_then(|directory| open_preview_directory(&directory, "prompts"))
    else {
        return;
    };
    let Ok(entries) = directory.entries() else {
        *unavailable += 1;
        return;
    };
    let mut found: Vec<PromptTemplate> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut duplicates = BTreeSet::new();
    for (index, entry) in entries.enumerate() {
        if index >= MAXIMUM_PROMPT_ENTRIES {
            *unavailable += 1;
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                *unavailable += 1;
                continue;
            }
        };
        let Some((name, file_name)) = project_file(&entry) else {
            *unavailable += 1;
            continue;
        };
        let Some(bytes) = read_preview_file(&directory, &file_name, MAXIMUM_PROMPT_BODY_BYTES)
        else {
            *unavailable += 1;
            continue;
        };
        let model_path = root.model_path.trim_end_matches('/');
        let path = format!("{model_path}/{PROJECT_PROMPTS_DIRECTORY}/{file_name}");
        // Grant aliases cannot contain a colon, so this scope cannot collide with another root.
        let scope = if root.scope.eq_ignore_ascii_case(PROMPTS_SCOPE) {
            "project:global"
        } else {
            &root.scope
        };
        let Some(template) = template_from_bytes(&name, &bytes, scope, path) else {
            *unavailable += 1;
            continue;
        };
        let key = name.to_ascii_lowercase();
        if seen.insert(key.clone()) {
            found.push(template);
        } else {
            duplicates.insert(key);
            *unavailable += 1;
        }
    }
    for template in found {
        if templates.len() >= MAXIMUM_PROMPTS {
            *unavailable += 1;
            continue;
        }
        // Reject every member of a case-insensitive collision within one root.
        if duplicates.contains(&template.name.to_ascii_lowercase()) {
            *unavailable += 1;
            continue;
        }
        templates.push(template);
    }
}

fn project_file(entry: &cap_std::fs::DirEntry) -> Option<(String, String)> {
    let file_type = entry.file_type().ok()?;
    if file_type.is_symlink() || !file_type.is_file() {
        return None;
    }
    let file_name = entry.file_name();
    let file_name = file_name.to_str()?;
    let name = file_name.strip_suffix(&format!(".{PROMPT_EXTENSION}"))?;
    if !valid_name(name) || reserved_name(name) {
        return None;
    }
    Some((name.to_owned(), file_name.to_owned()))
}

pub(crate) fn valid_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAXIMUM_PROMPT_NAME_BYTES || !name.is_ascii() {
        return false;
    }
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub(crate) fn reserved_name(name: &str) -> bool {
    RESERVED_COMMAND_NAMES
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(name))
}

#[derive(Debug)]
struct PromptDocument {
    description: String,
    argument_hint: String,
    body: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Frontmatter {
    description: String,
    #[serde(rename = "argument-hint")]
    argument_hint: String,
}

fn parse_document(markdown: &str) -> Result<PromptDocument, PromptError> {
    if markdown.contains('\0') || markdown.len() > MAXIMUM_PROMPT_BODY_BYTES {
        return Err(PromptError::Malformed);
    }
    let (frontmatter, body) = split_frontmatter(markdown)?;
    let (mut description, mut argument_hint) = (String::new(), String::new());
    if let Some(yaml) = frontmatter {
        let parsed: Frontmatter = if yaml.trim().is_empty() {
            Frontmatter::default()
        } else {
            serde_yaml_ng::from_str(yaml).map_err(|_| PromptError::Malformed)?
        };
        description = collapse(&parsed.description);
        argument_hint = collapse(&parsed.argument_hint);
    }
    if description.len() > MAXIMUM_PROMPT_DESCRIPTION_BYTES
        || argument_hint.len() > MAXIMUM_PROMPT_ARGUMENT_HINT_BYTES
    {
        return Err(PromptError::Bound);
    }
    let body = body.trim().to_owned();
    if body.is_empty() || body.contains('\0') {
        return Err(PromptError::Malformed);
    }
    Ok(PromptDocument {
        description,
        argument_hint,
        body,
    })
}

fn split_frontmatter(markdown: &str) -> Result<(Option<&str>, &str), PromptError> {
    let Some(first) = markdown.split_inclusive('\n').next() else {
        return Ok((None, markdown));
    };
    if first.trim_end_matches(['\n', '\r']).trim_end() != "---" {
        return Ok((None, markdown));
    }
    let rest = &markdown[first.len()..];
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\n', '\r']).trim_end() == "---" {
            return Ok((Some(&rest[..offset]), &rest[offset + line.len()..]));
        }
        offset += line.len();
    }
    Err(PromptError::Malformed)
}

fn collapse(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Render a template body for one argument string. Substituted argument text is
/// appended without a second pass, so a value that contains `$1` stays literal.
pub(crate) fn render(body: &str, arguments: &str) -> Result<String, PromptError> {
    let arguments = tokenize(arguments)?;
    render_arguments(body, &arguments)
}

/// Split the trailing argument string into quoted groups. There is no shell
/// interpolation: a dollar sign or backtick inside an argument stays literal.
fn tokenize(input: &str) -> Result<Vec<String>, PromptError> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut characters = input.chars();
    while let Some(character) = characters.next() {
        match character {
            character if character.is_whitespace() => {
                if started {
                    arguments.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            '"' => {
                started = true;
                loop {
                    match characters.next() {
                        None => return Err(PromptError::Malformed),
                        Some('"') => break,
                        Some('\\') => match characters.next() {
                            None => return Err(PromptError::Malformed),
                            Some(escaped) => push_argument(&mut current, escaped)?,
                        },
                        Some(other) => push_argument(&mut current, other)?,
                    }
                }
            }
            '\'' => {
                started = true;
                loop {
                    match characters.next() {
                        None => return Err(PromptError::Malformed),
                        Some('\'') => break,
                        Some(other) => push_argument(&mut current, other)?,
                    }
                }
            }
            '\\' => {
                started = true;
                match characters.next() {
                    None => return Err(PromptError::Malformed),
                    Some(escaped) => push_argument(&mut current, escaped)?,
                }
            }
            other => {
                started = true;
                push_argument(&mut current, other)?;
            }
        }
        if arguments.len() > MAXIMUM_PROMPT_ARGUMENTS {
            return Err(PromptError::Bound);
        }
    }
    if started {
        arguments.push(current);
    }
    if arguments.len() > MAXIMUM_PROMPT_ARGUMENTS {
        return Err(PromptError::Bound);
    }
    Ok(arguments)
}

fn push_argument(current: &mut String, character: char) -> Result<(), PromptError> {
    if current.len().saturating_add(character.len_utf8()) > super::MAXIMUM_MESSAGE_BYTES {
        return Err(PromptError::Bound);
    }
    current.push(character);
    Ok(())
}

fn render_arguments(body: &str, arguments: &[String]) -> Result<String, PromptError> {
    let mut output = String::with_capacity(body.len().min(super::MAXIMUM_MESSAGE_BYTES));
    let mut index = 0;
    while index < body.len() {
        let character = body[index..].chars().next().ok_or(PromptError::Malformed)?;
        if character == '\\' {
            if body[index + 1..].starts_with('$') {
                push(&mut output, "$")?;
                index += 2;
            } else {
                push(&mut output, "\\")?;
                index += 1;
            }
            continue;
        }
        if character != '$' {
            push(&mut output, &character.to_string())?;
            index += character.len_utf8();
            continue;
        }
        let rest = &body[index + 1..];
        if let Some(after) = rest.strip_prefix('{') {
            let close = after.find('}').ok_or(PromptError::Malformed)?;
            push(&mut output, &substitute_braced(&after[..close], arguments)?)?;
            index += 1 + 1 + close + 1;
            continue;
        }
        if rest.starts_with('@') {
            push(&mut output, &arguments.join(" "))?;
            index += 2;
            continue;
        }
        if rest.starts_with("ARGUMENTS") {
            push(&mut output, &arguments.join(" "))?;
            index += 1 + "ARGUMENTS".len();
            continue;
        }
        if let Some(digits) = leading_digits(rest) {
            let position = parse_position(digits)?;
            if let Some(value) = arguments.get(position - 1) {
                push(&mut output, value)?;
            }
            index += 1 + digits.len();
            continue;
        }
        push(&mut output, "$")?;
        index += 1;
    }
    Ok(output)
}

fn substitute_braced(inner: &str, arguments: &[String]) -> Result<String, PromptError> {
    if let Some((left, default)) = inner.split_once(":-") {
        let value = match left {
            "@" | "ARGUMENTS" => arguments.join(" "),
            name => {
                let position = parse_position(name)?;
                arguments.get(position - 1).cloned().unwrap_or_default()
            }
        };
        return Ok(if value.is_empty() {
            default.to_owned()
        } else {
            value
        });
    }
    if let Some(range) = inner.strip_prefix('@') {
        if range.is_empty() {
            return Ok(arguments.join(" "));
        }
        let range = range.strip_prefix(':').ok_or(PromptError::Malformed)?;
        let (start, length) = match range.split_once(':') {
            Some((start, length)) => (start, Some(length)),
            None => (range, None),
        };
        let start = parse_position(start)?;
        let slice = arguments.get(start - 1..).unwrap_or(&[]);
        let slice = match length {
            Some(length) => {
                let length: usize = length.parse().map_err(|_| PromptError::Malformed)?;
                &slice[..slice.len().min(length)]
            }
            None => slice,
        };
        return Ok(slice.join(" "));
    }
    if inner == "ARGUMENTS" {
        return Ok(arguments.join(" "));
    }
    let position = parse_position(inner)?;
    Ok(arguments.get(position - 1).cloned().unwrap_or_default())
}

fn leading_digits(value: &str) -> Option<&str> {
    let end = value.bytes().take_while(u8::is_ascii_digit).count();
    (end > 0).then(|| &value[..end])
}

fn parse_position(value: &str) -> Result<usize, PromptError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(PromptError::Malformed);
    }
    let position: usize = value.parse().map_err(|_| PromptError::Malformed)?;
    if position == 0 {
        return Err(PromptError::Malformed);
    }
    Ok(position)
}

fn push(output: &mut String, value: &str) -> Result<(), PromptError> {
    if output.len().saturating_add(value.len()) > super::MAXIMUM_MESSAGE_BYTES {
        return Err(PromptError::Bound);
    }
    output.push_str(value);
    Ok(())
}
