//! Bounded instruction sources and project skill advertisements.
//!
//! Discovery stays at authorised roots and rejects symbolic links.
//! It never searches global home directories outside those roots.

use std::path::{Path, PathBuf};
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
pub(crate) const MAXIMUM_SKILL_METADATA_BYTES: usize = 16 * 1024;
pub(crate) const MAXIMUM_SKILL_NAME_BYTES: usize = 256;
pub(crate) const MAXIMUM_SKILL_DESCRIPTION_BYTES: usize = 4 * 1024;
pub(crate) const MAXIMUM_SKILL_ADVERTISEMENT_BYTES: usize = 256 * 1024;
pub(crate) const MAXIMUM_SKILL_BODY_BYTES: usize = 1024 * 1024;
pub(crate) const SKILL_METADATA_NAME: &str = "SKILL.md";

/// File suggestions are a preview. The traversal work budget bounds search
/// cost independently of the retained history bound.
pub(crate) const MAXIMUM_FILE_RESULTS: usize = 50;
pub(crate) const MAXIMUM_FILE_TRAVERSAL_ENTRIES: usize = 20_000;
pub(crate) const MAXIMUM_FILE_QUERY_BYTES: usize = 200;
pub(crate) const MAXIMUM_FILE_PATH_BYTES: usize = 4096;
pub(crate) const MAXIMUM_FILE_DEPTH: usize = 32;

const DISCOVERY_DEADLINE: Duration = Duration::from_secs(5);
const SKILL_LIST_COMMAND: &str = r#"
export LC_ALL=C
root=${1:-.agents/skills}
if [ "$root" = .agents/skills ] && [ -L .agents ]; then exit 6; fi
if [ -L "$root" ]; then exit 6; fi
if [ ! -d "$root" ]; then exit 3; fi
count=0
for dir in "$root"/*; do
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
root=${2:-.agents/skills}
if [ "$root" = .agents/skills ] && [ -L .agents ]; then exit 6; fi
if [ -L "$root" ] || [ -L "${file%/*}" ] || [ -L "$file" ]; then exit 6; fi
if [ ! -f "$file" ] || [ ! -r "$file" ]; then exit 1; fi
size=$(wc -c < "$file") || exit 1
if [ "$size" -gt 1048576 ]; then exit 7; fi
sha256sum < "$file" || exit 1
head -c 16384 -- "$file" | awk '{ print; if (NR > 1 && $0 ~ /^---\r?$/) exit }'
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
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
    let yaml = lines
        .take_while(|line| line.trim_end() != "---")
        .collect::<Vec<_>>()
        .join("\n");
    let mut metadata: SkillMetadata =
        serde_yaml_ng::from_str(&yaml).map_err(|_| ResourceError::Invalid)?;
    metadata.name = metadata.name.trim().to_owned();
    // YAML block scalars are valid descriptions. Advertisements remain single-line text.
    metadata.description = metadata
        .description
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if !valid_metadata_field(&metadata.name, MAXIMUM_SKILL_NAME_BYTES)
        || !valid_metadata_field(&metadata.description, MAXIMUM_SKILL_DESCRIPTION_BYTES)
    {
        return Err(ResourceError::Invalid);
    }
    Ok(metadata)
}

fn metadata_end(bytes: &[u8]) -> Option<usize> {
    let mut offset = 0;
    for (index, line) in bytes.split_inclusive(|byte| *byte == b'\n').enumerate() {
        offset += line.len();
        if index > 0 && line.trim_ascii_end() == b"---" {
            return Some(offset);
        }
    }
    None
}

fn valid_metadata_field(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

/// Recognise a path below the skill directory and return the enclosing skill
/// directory. Project paths use `<grant>/.agents/skills/`. Global skills use
/// `/.agents/skills/`.
pub(crate) fn skill_directory(path: &str) -> Option<String> {
    let marker = "/.agents/skills/";
    let index = path.find(marker)?;
    let rest = &path[index + marker.len()..];
    let name = rest.split('/').next()?;
    if name.is_empty() || name == "." || name == ".." || name.contains('\\') {
        return None;
    }
    Some(format!("{}{}{}", &path[..index], marker, name))
}

pub(crate) fn validate_skill_read(path: &str, bytes: &[u8]) -> Result<(), ResourceError> {
    if path.ends_with(&format!("/{SKILL_METADATA_NAME}")) {
        parse_skill_metadata(bytes)?;
    }
    Ok(())
}

/// Discover skill metadata below each authorised grant root, then below the
/// global skill root. The grant order is preserved and the total count and
/// advertised bytes stay bounded.
pub(crate) async fn discover_skills(
    sandbox: Option<&GuestSandbox>,
    policy: &crate::agents::DirectoryPolicy,
    secret: Option<&str>,
) -> Result<Vec<SkillAdvertisement>, ResourceError> {
    let mut skills = Vec::new();
    let mut advertised_bytes = 0usize;
    for grant in policy.grants() {
        discover_in_root(
            sandbox,
            &grant.guest_path,
            ".agents/skills",
            &grant.alias,
            secret,
            &mut skills,
            &mut advertised_bytes,
        )
        .await?;
    }
    if let Some(root) = policy.skill_root() {
        discover_in_root(
            sandbox,
            "/",
            if sandbox.is_some() {
                ".agents/skills"
            } else {
                &root.guest_path
            },
            &root.scope,
            secret,
            &mut skills,
            &mut advertised_bytes,
        )
        .await?;
    }
    Ok(skills)
}

async fn discover_in_root(
    sandbox: Option<&GuestSandbox>,
    search_directory: &str,
    root: &str,
    scope: &str,
    secret: Option<&str>,
    skills: &mut Vec<SkillAdvertisement>,
    advertised_bytes: &mut usize,
) -> Result<(), ResourceError> {
    for relative in list_skill_files(sandbox, search_directory, root).await? {
        if skills.len() >= MAXIMUM_SKILLS {
            return Ok(());
        }
        let (hash, bytes) =
            match read_skill_metadata(sandbox, search_directory, root, &relative).await? {
                Some(snapshot) => snapshot,
                None => continue,
            };
        if secret.is_some_and(|secret| !secret.is_empty() && contains_secret(&bytes, secret)) {
            return Err(ResourceError::Credential);
        }
        let metadata = parse_skill_metadata(&bytes)?;
        let path = if relative.starts_with('/') {
            relative
        } else {
            format!("{}/{}", search_directory.trim_end_matches('/'), relative)
        };
        if secret.is_some_and(|secret| {
            !secret.is_empty() && (path.contains(secret) || scope.contains(secret))
        }) {
            return Err(ResourceError::Credential);
        }
        let advertisement = SkillAdvertisement {
            source: ResourceSource {
                kind: ResourceKind::Skill,
                scope: scope.to_owned(),
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
        *advertised_bytes = advertised_bytes.saturating_add(
            advertisement.name.len()
                + advertisement.description.len()
                + advertisement.read_path.len(),
        );
        if *advertised_bytes > MAXIMUM_SKILL_ADVERTISEMENT_BYTES {
            return Err(ResourceError::Bound);
        }
        skills.push(advertisement);
    }
    Ok(())
}

async fn list_skill_files(
    sandbox: Option<&GuestSandbox>,
    directory: &str,
    root: &str,
) -> Result<Vec<String>, ResourceError> {
    let (exit, stdout) = resource_output(
        sandbox,
        GuestExec::command(
            "sh",
            vec![
                "-c".to_owned(),
                SKILL_LIST_COMMAND.to_owned(),
                "skill-list".to_owned(),
                root.to_owned(),
            ],
        )
        .in_dir(directory),
        MAXIMUM_SKILLS * (MAXIMUM_RESOURCE_PATH_BYTES + 1),
    )
    .await?;
    match exit {
        Some(0 | 3) => {}
        Some(6) => return Err(ResourceError::UnsafeLink),
        Some(7) => return Err(ResourceError::Bound),
        _ => return Err(ResourceError::Read),
    }
    let text = std::str::from_utf8(&stdout).map_err(|_| ResourceError::Invalid)?;
    let mut files = Vec::new();
    for line in text.split('\0') {
        if line.is_empty() {
            continue;
        }
        if line.len() > MAXIMUM_RESOURCE_PATH_BYTES || line.chars().any(char::is_control) {
            return Err(ResourceError::Bound);
        }
        let relative = line
            .strip_prefix(root)
            .and_then(|path| path.strip_prefix('/'))
            .ok_or(ResourceError::Invalid)?;
        let parts = relative.split('/').collect::<Vec<_>>();
        if parts.len() != 2
            || parts[0].is_empty()
            || matches!(parts[0], "." | "..")
            || parts[1] != SKILL_METADATA_NAME
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
    sandbox: Option<&GuestSandbox>,
    directory: &str,
    root: &str,
    relative: &str,
) -> Result<Option<(String, Vec<u8>)>, ResourceError> {
    let (exited, stdout) = resource_output(
        sandbox,
        GuestExec::command(
            "sh",
            vec![
                "-c".to_owned(),
                SKILL_READ_COMMAND.to_owned(),
                "skill-read".to_owned(),
                relative.to_owned(),
                root.to_owned(),
            ],
        )
        .in_dir(directory),
        MAXIMUM_SKILL_METADATA_BYTES + 68,
    )
    .await?;
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

async fn resource_output(
    sandbox: Option<&GuestSandbox>,
    request: GuestExec,
    maximum: usize,
) -> Result<(Option<i32>, Vec<u8>), ResourceError> {
    let Some(sandbox) = sandbox else {
        let output = super::capture_host_file(request, None, DISCOVERY_DEADLINE, maximum)
            .await
            .map_err(|failure| {
                if failure.result.termination == super::CommandTermination::ResourceLimit {
                    ResourceError::Bound
                } else {
                    ResourceError::Read
                }
            })?;
        return Ok((output.status.code(), output.stdout));
    };
    let mut command = sandbox
        .exec_cmd(request)
        .await
        .map_err(|_| ResourceError::Read)?;
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
    Ok((exited, stdout))
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

/// An immutable candidate view for one authorised root alias. A
/// review-before-apply root reads this captured manifest instead of the host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CandidateRoot {
    pub(crate) alias: String,
    pub(crate) entries: Vec<String>,
}

/// The effective read-only source for one authorised root. `model_path` is
/// the path prefix a model request uses. A candidate-backed root has no host
/// directory because its entries are the immutable source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EffectiveRoot {
    pub(crate) scope: String,
    pub(crate) model_path: String,
    pub(crate) host_path: Option<PathBuf>,
    pub(crate) candidate_paths: Vec<String>,
}

impl EffectiveRoot {
    pub(crate) fn candidate(&self) -> bool {
        self.host_path.is_none()
    }
}

/// Resolve the effective read-only roots for idle previews. This never starts
/// a sandbox. Host work locations keep their host paths; sandbox roots use
/// grant aliases. An immutable candidate replaces its review root so a preview
/// cannot substitute newer host files for a captured candidate.
pub(crate) fn effective_roots(
    policy: &crate::agents::DirectoryPolicy,
    candidates: &[CandidateRoot],
    data_root: &Path,
) -> Vec<EffectiveRoot> {
    let mut roots = Vec::new();
    for grant in policy.grants() {
        if !grant.host_path.is_absolute() {
            continue;
        }
        // Private Power Plant data stays out of previews even when a parent
        // directory is authorised. The traversal also rejects nested data.
        if grant.host_path.starts_with(data_root)
            || grant
                .host_path
                .components()
                .any(|part| part.as_os_str() == ".git")
        {
            continue;
        }
        if let Some(candidate) = candidates
            .iter()
            .find(|candidate| candidate.alias == grant.alias)
        {
            roots.push(EffectiveRoot {
                scope: grant.alias.clone(),
                model_path: grant.guest_path.clone(),
                host_path: None,
                candidate_paths: candidate.entries.clone(),
            });
            continue;
        }
        roots.push(EffectiveRoot {
            scope: grant.alias.clone(),
            model_path: grant.guest_path.clone(),
            host_path: Some(grant.host_path.clone()),
            candidate_paths: Vec::new(),
        });
    }
    roots
}

/// A bounded project skill body read from an effective read-only root. This
/// never starts a sandbox. A candidate-backed root has no host directory, so
/// its body stays unavailable instead of using current host files.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreviewSkill {
    pub(crate) scope: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) body: String,
    /// The model-visible SKILL.md path.
    pub(crate) path: String,
    /// The model-visible directory that holds relative references.
    pub(crate) relative_base: String,
    pub(crate) content_hash: String,
}

/// Read project skill bodies from the effective read-only roots. The result is
/// bounded by the skill count and body size. Symbolic links never enter a
/// preview. A candidate-backed root counts as unavailable.
pub(crate) fn preview_project_skills(
    roots: &[EffectiveRoot],
    grants: &[super::DirectoryGrant],
    data_root: &Path,
) -> (Vec<PreviewSkill>, usize) {
    let mut skills = Vec::new();
    let mut unavailable = 0;
    for root in roots {
        match &root.host_path {
            Some(path) => {
                if let Some(grant) = grants.iter().find(|grant| grant.alias == root.scope) {
                    read_preview_root(root, path, grant, data_root, &mut skills);
                }
            }
            None => unavailable += 1,
        }
        if skills.len() >= MAXIMUM_SKILLS {
            break;
        }
    }
    (skills, unavailable)
}

fn read_preview_root(
    root: &EffectiveRoot,
    host: &Path,
    grant: &super::DirectoryGrant,
    data_root: &Path,
    skills: &mut Vec<PreviewSkill>,
) {
    use cap_std::fs::MetadataExt;
    let skill_path = host.join(".agents/skills");
    if skill_path.starts_with(data_root) || data_root.starts_with(&skill_path) {
        return;
    }
    let Ok(directory) = cap_std::fs::Dir::open_ambient_dir(host, cap_std::ambient_authority())
    else {
        return;
    };
    if grant.revalidate().is_err()
        || !directory.dir_metadata().is_ok_and(|metadata| {
            metadata.dev() == grant.identity.device && metadata.ino() == grant.identity.inode
        })
    {
        return;
    }
    let Some(directory) = open_preview_directory(&directory, ".agents")
        .and_then(|directory| open_preview_directory(&directory, "skills"))
    else {
        return;
    };
    let Ok(entries) = directory.entries() else {
        return;
    };
    for entry in entries.take(MAXIMUM_SKILLS + 1) {
        if skills.len() >= MAXIMUM_SKILLS {
            return;
        }
        let Ok(entry) = entry else {
            continue;
        };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() || !kind.is_dir() {
            continue;
        }
        let Some(folder) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if folder.is_empty() || matches!(folder.as_str(), "." | ".." | ".git") {
            continue;
        }
        let Some(bytes) = open_preview_directory(&directory, &folder)
            .and_then(|directory| read_preview_bytes(&directory, MAXIMUM_SKILL_BODY_BYTES))
        else {
            continue;
        };
        let Ok(metadata) = parse_skill_metadata(&bytes) else {
            continue;
        };
        let Ok(body) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let relative_base = format!("{}/.agents/skills/{folder}", root.model_path);
        skills.push(PreviewSkill {
            scope: root.scope.clone(),
            name: metadata.name,
            description: metadata.description,
            body: body.to_owned(),
            path: format!("{relative_base}/{SKILL_METADATA_NAME}"),
            relative_base,
            content_hash: content_hash(&bytes),
        });
    }
}

fn open_preview_directory(directory: &cap_std::fs::Dir, name: &str) -> Option<cap_std::fs::Dir> {
    use cap_std::fs::OpenOptionsExt;
    let mut options = cap_std::fs::OpenOptions::new();
    // Directory handles and O_NOFOLLOW prevent replacement with an escaping link.
    options.read(true).custom_flags(0o200000 | 0o400000);
    let file = directory.open_with(name, &options).ok()?;
    Some(cap_std::fs::Dir::from_std_file(file.into_std()))
}

fn read_preview_bytes(directory: &cap_std::fs::Dir, maximum: usize) -> Option<Vec<u8>> {
    use cap_std::fs::OpenOptionsExt;
    use std::io::Read;
    let mut options = cap_std::fs::OpenOptions::new();
    // O_NONBLOCK avoids a FIFO stall before the regular-file check.
    options.read(true).custom_flags(0o400000 | 0o4000);
    let file = directory.open_with(SKILL_METADATA_NAME, &options).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > u64::try_from(maximum).ok()? {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(u64::try_from(maximum).ok()? + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= maximum).then_some(bytes)
}

/// Compose the skill advertisement block. Skill text stays below the explicit
/// task and server boundary instructions.
pub(crate) fn compose_skills(skills: &[SkillAdvertisement]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut composed = String::from("Available skills. Read a skill body before you use it:\n");
    for skill in skills {
        composed.push_str(&skill.advertisement());
        composed.push('\n');
    }
    composed
}
