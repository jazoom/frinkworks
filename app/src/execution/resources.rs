//! Bounded instruction sources and project skill advertisements.
//!
//! Discovery stays at authorised roots and rejects symbolic links.
//! It never searches global home directories outside those roots.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::execution::command::CommandStream;
use crate::sandbox::{CommandEvent, GuestExec, GuestSandbox};

#[cfg(test)]
mod tests;

pub(crate) const MAXIMUM_RESOURCE_SOURCES: usize = 128;
pub(crate) const MAXIMUM_RESOURCE_SCOPE_BYTES: usize = 256;
pub(crate) const MAXIMUM_RESOURCE_PATH_BYTES: usize = 4096;
pub(crate) const MAXIMUM_SKILLS: usize = 64;
pub(crate) const MAXIMUM_SKILL_METADATA_BYTES: usize = 4 * 1024;
pub(crate) const MAXIMUM_SKILL_NAME_BYTES: usize = 128;
pub(crate) const MAXIMUM_SKILL_DESCRIPTION_BYTES: usize = 1024;
pub(crate) const MAXIMUM_SKILL_ADVERTISEMENT_BYTES: usize = 64 * 1024;
pub(crate) const MAXIMUM_SKILL_BODY_BYTES: usize = 256 * 1024;
pub(crate) const SKILL_METADATA_NAME: &str = "SKILL.md";

const DISCOVERY_DEADLINE: Duration = if cfg!(test) {
    Duration::from_millis(50)
} else {
    Duration::from_secs(5)
};
const SKILL_LIST_COMMAND: &str = r#"
export LC_ALL=C
if [ -L .pi ] || [ -L .pi/skills ]; then exit 6; fi
if [ ! -d .pi/skills ]; then exit 3; fi
count=0
for dir in .pi/skills/*; do
    count=$((count + 1))
    if [ "$count" -gt 64 ]; then exit 7; fi
    if [ -L "$dir" ]; then exit 6; fi
    file="$dir/SKILL.md"
    if [ -L "$file" ]; then exit 6; fi
    if [ ! -f "$file" ] || [ ! -r "$file" ]; then continue; fi
    printf '%s\0' "$file"
done
"#;
// Hash the bounded file, but return only its header during discovery.
const SKILL_READ_COMMAND: &str = r#"
file=$1
if [ -L .pi ] || [ -L .pi/skills ] || [ -L "${file%/*}" ] || [ -L "$file" ]; then exit 6; fi
if [ ! -f "$file" ] || [ ! -r "$file" ]; then exit 1; fi
size=$(wc -c < "$file") || exit 1
if [ "$size" -gt 262144 ]; then exit 7; fi
sha256sum < "$file" || exit 1
head -c 4096 -- "$file" | awk '{ print; if (NR > 1 && $0 ~ /^---\r?$/) exit }'
"#;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ResourceKind {
    Instruction,
    Skill,
}

impl ResourceKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Instruction => "Instructions",
            Self::Skill => "Skill",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct ResourceSource {
    pub(crate) kind: ResourceKind,
    pub(crate) scope: String,
    pub(crate) path: String,
    pub(crate) content_hash: String,
}

impl ResourceSource {
    pub(crate) fn new(
        kind: ResourceKind,
        scope: impl Into<String>,
        path: impl Into<String>,
        bytes: &[u8],
    ) -> Self {
        Self {
            kind,
            scope: scope.into(),
            path: path.into(),
            content_hash: content_hash(bytes),
        }
    }

    pub(crate) fn validate_read(
        &self,
        bytes: &[u8],
        advertised: &[Self],
        secret: Option<&str>,
    ) -> Result<(), ResourceError> {
        if !self.valid() || self.content_hash != content_hash(bytes) {
            return Err(ResourceError::Invalid);
        }
        if advertised
            .iter()
            .any(|source| source.path == self.path && source != self)
        {
            return Err(ResourceError::Changed);
        }
        if secret.is_some_and(|secret| {
            !secret.is_empty()
                && (self.path.contains(secret)
                    || self.scope.contains(secret)
                    || contains_secret(bytes, secret))
        }) {
            return Err(ResourceError::Credential);
        }
        Ok(())
    }

    pub(crate) fn valid(&self) -> bool {
        !self.scope.is_empty()
            && self.scope.len() <= MAXIMUM_RESOURCE_SCOPE_BYTES
            && !self.scope.chars().any(char::is_control)
            && !self.path.is_empty()
            && self.path.len() <= MAXIMUM_RESOURCE_PATH_BYTES
            && !self.path.chars().any(char::is_control)
            && !self.path.contains('\0')
            && self.content_hash.len() == 71
            && self.content_hash.starts_with("sha256:")
            && self.content_hash[7..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InstructionSource {
    pub(crate) source: ResourceSource,
    pub(crate) text: String,
}

impl InstructionSource {
    pub(crate) fn new(scope: impl Into<String>, path: impl Into<String>, text: String) -> Self {
        let source = ResourceSource::new(ResourceKind::Instruction, scope, path, text.as_bytes());
        Self { source, text }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SkillMetadata {
    pub(crate) name: String,
    pub(crate) description: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SkillAdvertisement {
    pub(crate) source: ResourceSource,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) read_path: String,
}

impl SkillAdvertisement {
    pub(crate) fn advertisement(&self) -> String {
        format!(
            "- {}: {} (read `{}`)",
            self.name, self.description, self.read_path
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResourceError {
    Read,
    UnsafeLink,
    Invalid,
    Bound,
    Credential,
    Changed,
}

impl ResourceError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Read => "Power Plant could not read a project resource.",
            Self::UnsafeLink => "A project resource path is a link outside its authorised root.",
            Self::Invalid => "A project resource is not valid text.",
            Self::Bound => "A project resource exceeds its size limit.",
            Self::Credential => "A project resource contains a provider credential.",
            Self::Changed => "That skill changed after discovery. Start a new request before use.",
        }
    }
}

pub(crate) fn content_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(7 + digest.len() * 2);
    out.push_str("sha256:");
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Parse the leading YAML frontmatter of a skill file. Unknown keys and extra
/// body content are ignored. Malformed metadata is rejected.
pub(crate) fn parse_skill_metadata(bytes: &[u8]) -> Result<SkillMetadata, ResourceError> {
    if bytes.contains(&0) {
        return Err(ResourceError::Invalid);
    }
    let end = metadata_end(bytes).ok_or(ResourceError::Invalid)?;
    if end > MAXIMUM_SKILL_METADATA_BYTES {
        return Err(ResourceError::Bound);
    }
    let text = std::str::from_utf8(&bytes[..end]).map_err(|_| ResourceError::Invalid)?;
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Err(ResourceError::Invalid);
    }
    let mut name = None;
    let mut description = None;
    let mut closed = false;
    for line in lines {
        if line.trim() == "---" {
            closed = true;
            break;
        }
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let (key, value) = line.split_once(':').ok_or(ResourceError::Invalid)?;
        let value = value.trim();
        let value = if value.starts_with(['\'', '"']) {
            let quote = value.as_bytes()[0] as char;
            value
                .strip_prefix(quote)
                .and_then(|v| v.strip_suffix(quote))
                .ok_or(ResourceError::Invalid)?
        } else {
            if value.starts_with(['[', '{', '|', '>']) {
                return Err(ResourceError::Invalid);
            }
            value
        };
        match key.trim() {
            "name" if name.is_none() => name = Some(value.to_owned()),
            "description" if description.is_none() => description = Some(value.to_owned()),
            "name" | "description" => return Err(ResourceError::Invalid),
            _ => {}
        }
    }
    if !closed {
        return Err(ResourceError::Invalid);
    }
    let name = name.filter(|name| valid_metadata_field(name, MAXIMUM_SKILL_NAME_BYTES));
    let description =
        description.filter(|value| valid_metadata_field(value, MAXIMUM_SKILL_DESCRIPTION_BYTES));
    match (name, description) {
        (Some(name), Some(description)) => Ok(SkillMetadata { name, description }),
        _ => Err(ResourceError::Invalid),
    }
}

fn metadata_end(bytes: &[u8]) -> Option<usize> {
    let mut offset = 0;
    for (index, line) in bytes.split_inclusive(|byte| *byte == b'\n').enumerate() {
        offset += line.len();
        if index > 0 && line.trim_ascii() == b"---" {
            return Some(offset);
        }
    }
    None
}

fn valid_metadata_field(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

/// Recognise a path below `.pi/skills/` and return the enclosing skill directory.
pub(crate) fn skill_directory(path: &str) -> Option<String> {
    let marker = "/.pi/skills/";
    let index = path.find(marker)?;
    let rest = &path[index + marker.len()..];
    let name = rest.split('/').next()?;
    if name.is_empty() || name == "." || name == ".." || name.contains('\\') {
        return None;
    }
    Some(format!("{}{}{}", &path[..index], marker, name))
}

pub(crate) fn validate_skill_read(path: &str, bytes: &[u8]) -> Result<(), ResourceError> {
    if skill_directory(path).is_none() {
        return Ok(());
    }
    if path.ends_with(SKILL_METADATA_NAME) {
        parse_skill_metadata(bytes)?;
    }
    Ok(())
}

/// Discover skill metadata below each authorised grant root. The grant order is
/// preserved and the total count and advertised bytes stay bounded.
pub(crate) async fn discover_skills(
    sandbox: &GuestSandbox,
    grants: &[crate::agents::PolicyGrant],
    secret: Option<&str>,
) -> Result<Vec<SkillAdvertisement>, ResourceError> {
    let mut skills = Vec::new();
    let mut advertised_bytes = 0usize;
    for grant in grants {
        let files = list_skill_files(sandbox, &grant.guest_path).await?;
        for relative in files {
            if skills.len() >= MAXIMUM_SKILLS {
                return Ok(skills);
            }
            let (hash, bytes) =
                match read_skill_metadata(sandbox, &grant.guest_path, &relative).await? {
                    Some(snapshot) => snapshot,
                    None => continue,
                };
            if secret.is_some_and(|secret| !secret.is_empty() && contains_secret(&bytes, secret)) {
                return Err(ResourceError::Credential);
            }
            let metadata = parse_skill_metadata(&bytes)?;
            let path = format!("{}/{}", grant.guest_path, relative);
            if secret.is_some_and(|secret| {
                !secret.is_empty() && (path.contains(secret) || grant.alias.contains(secret))
            }) {
                return Err(ResourceError::Credential);
            }
            let advertisement = SkillAdvertisement {
                source: ResourceSource {
                    kind: ResourceKind::Skill,
                    scope: grant.alias.clone(),
                    path: path.clone(),
                    content_hash: hash,
                },
                name: metadata.name,
                description: metadata.description,
                read_path: path,
            };
            if !advertisement.source.valid() {
                return Err(ResourceError::Invalid);
            }
            advertised_bytes = advertised_bytes.saturating_add(
                advertisement.name.len()
                    + advertisement.description.len()
                    + advertisement.read_path.len(),
            );
            if advertised_bytes > MAXIMUM_SKILL_ADVERTISEMENT_BYTES {
                return Err(ResourceError::Bound);
            }
            skills.push(advertisement);
        }
    }
    Ok(skills)
}

async fn list_skill_files(
    sandbox: &GuestSandbox,
    directory: &str,
) -> Result<Vec<String>, ResourceError> {
    let command = sandbox
        .exec_cmd(GuestExec::shell(SKILL_LIST_COMMAND).in_dir(directory))
        .await
        .map_err(|_| ResourceError::Read)?;
    let stdout =
        collect_stdout(command, MAXIMUM_SKILLS * (MAXIMUM_RESOURCE_PATH_BYTES + 1)).await?;
    let text = std::str::from_utf8(&stdout).map_err(|_| ResourceError::Invalid)?;
    let mut files = Vec::new();
    for line in text.split('\0') {
        if line.is_empty() {
            continue;
        }
        if line.len() > MAXIMUM_RESOURCE_PATH_BYTES || line.chars().any(char::is_control) {
            return Err(ResourceError::Bound);
        }
        let parts = line.split('/').collect::<Vec<_>>();
        if parts.len() != 4
            || parts[0] != ".pi"
            || parts[1] != "skills"
            || parts[2].is_empty()
            || matches!(parts[2], "." | "..")
            || parts[3] != SKILL_METADATA_NAME
        {
            return Err(ResourceError::Invalid);
        }
        files.push(line.to_owned());
        if files.len() > MAXIMUM_SKILLS {
            return Err(ResourceError::Bound);
        }
    }
    Ok(files)
}

async fn read_skill_metadata(
    sandbox: &GuestSandbox,
    directory: &str,
    relative: &str,
) -> Result<Option<(String, Vec<u8>)>, ResourceError> {
    let mut command = sandbox
        .exec_cmd(
            GuestExec::command(
                "sh",
                vec![
                    "-c".to_owned(),
                    SKILL_READ_COMMAND.to_owned(),
                    "skill-read".to_owned(),
                    relative.to_owned(),
                ],
            )
            .in_dir(directory),
        )
        .await
        .map_err(|_| ResourceError::Read)?;
    let mut stdout = Vec::new();
    let deadline = Instant::now() + DISCOVERY_DEADLINE;
    let exited = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            command.kill().await;
            command.close().await;
            return Err(ResourceError::Read);
        }
        let event = match tokio::time::timeout(remaining, command.recv()).await {
            Ok(event) => event,
            Err(_) => {
                command.kill().await;
                command.close().await;
                return Err(ResourceError::Read);
            }
        };
        let Some(event) = event else {
            break None;
        };
        match event {
            CommandEvent::Output { stream, bytes } => {
                if stream != CommandStream::Stdout {
                    continue;
                }
                if stdout.len().saturating_add(bytes.len()) > MAXIMUM_SKILL_METADATA_BYTES + 68 {
                    command.kill().await;
                    command.close().await;
                    return Err(ResourceError::Bound);
                }
                stdout.extend_from_slice(&bytes);
            }
            CommandEvent::Exited(code) => break Some(code),
            CommandEvent::Failed => {
                command.close().await;
                return Err(ResourceError::Read);
            }
        }
    };
    command.close().await;
    if exited == Some(6) {
        return Err(ResourceError::UnsafeLink);
    }
    if exited == Some(1) {
        return Ok(None);
    }
    if exited == Some(7) {
        return Err(ResourceError::Bound);
    }
    if exited != Some(0) {
        return Err(ResourceError::Read);
    }
    let newline = stdout
        .iter()
        .position(|byte| *byte == b'\n')
        .ok_or(ResourceError::Invalid)?;
    let digest = std::str::from_utf8(&stdout[..newline]).map_err(|_| ResourceError::Invalid)?;
    let digest = digest
        .split_whitespace()
        .next()
        .ok_or(ResourceError::Invalid)?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ResourceError::Invalid);
    }
    let hash = format!("sha256:{digest}");
    let bytes = &stdout[newline + 1..];
    let end = metadata_end(bytes).ok_or(ResourceError::Invalid)?;
    Ok(Some((hash, bytes[..end].to_vec())))
}

async fn collect_stdout(
    mut command: crate::sandbox::CommandSession,
    maximum: usize,
) -> Result<Vec<u8>, ResourceError> {
    let mut stdout = Vec::new();
    let deadline = Instant::now() + DISCOVERY_DEADLINE;
    let mut exited = None;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            command.kill().await;
            command.close().await;
            return Err(ResourceError::Read);
        }
        let event = match tokio::time::timeout(remaining, command.recv()).await {
            Ok(event) => event,
            Err(_) => {
                command.kill().await;
                command.close().await;
                return Err(ResourceError::Read);
            }
        };
        let Some(event) = event else {
            break;
        };
        match event {
            CommandEvent::Output { stream, bytes } => {
                if stream == CommandStream::Stdout {
                    if stdout.len().saturating_add(bytes.len()) > maximum {
                        command.kill().await;
                        command.close().await;
                        return Err(ResourceError::Bound);
                    }
                    stdout.extend_from_slice(&bytes);
                }
            }
            CommandEvent::Exited(code) => {
                exited = Some(code);
                break;
            }
            CommandEvent::Failed => {
                command.close().await;
                return Err(ResourceError::Read);
            }
        }
    }
    command.close().await;
    match exited {
        Some(0 | 3) => Ok(stdout),
        Some(6) => Err(ResourceError::UnsafeLink),
        Some(7) => Err(ResourceError::Bound),
        _ => Err(ResourceError::Read),
    }
}

pub(crate) fn consumed_sources(
    base: &[ResourceSource],
    turns: &[crate::providers::ChatTurn],
) -> Vec<ResourceSource> {
    let mut sources = base.to_vec();
    for source in turns
        .iter()
        .flat_map(|turn| &turn.calls)
        .filter_map(|call| call.result.as_ref()?.resource.as_ref())
        .chain(
            turns
                .iter()
                .filter(|turn| turn.role == crate::providers::Role::User)
                .flat_map(|turn| &turn.usage)
                .flat_map(|request| &request.sources),
        )
    {
        if !sources.contains(source) {
            sources.push(source.clone());
        }
    }
    sources
}

pub(crate) fn contains_secret(bytes: &[u8], secret: &str) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|text| text.contains(secret))
}

/// Compose consumed instruction sources in grant order. The text of each source
/// keeps its own scope label. Explicit task text stays before this block.
pub(crate) fn compose_instructions(sources: &[InstructionSource]) -> String {
    let mut composed = String::new();
    for source in sources {
        let header = format!(
            "## Instructions from {} ({})\n",
            source.source.scope, source.source.path
        );
        if !composed.is_empty() {
            composed.push('\n');
        }
        composed.push_str(&header);
        composed.push_str(source.text.trim_end());
        composed.push('\n');
    }
    composed
}

/// Compose the skill advertisement block. Skill text stays below the explicit
/// task and server boundary instructions.
pub(crate) fn compose_skills(skills: &[SkillAdvertisement]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut composed =
        String::from("Available project skills. Read a skill body before you use it:\n");
    for skill in skills {
        composed.push_str(&skill.advertisement());
        composed.push('\n');
    }
    composed
}
