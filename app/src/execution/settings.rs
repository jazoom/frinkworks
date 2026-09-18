use std::path::{Path, PathBuf};

use rand::rand_core::TryRng;
use rand::rngs::SysRng;
use serde::{Deserialize, Serialize};

use crate::{
    agents::{NetworkAccess, ToolId},
    environments::EnvironmentId,
    providers::ModelSelection,
};

pub(crate) const MAXIMUM_INSTRUCTION_BYTES: usize = crate::agents::MAXIMUM_INSTRUCTION_BYTES;
pub(crate) const MAXIMUM_DIRECTORY_GRANTS: usize = 8;
const MAXIMUM_FORM_GRANT_BYTES: usize = 8 * 1024;
const MAXIMUM_ALIAS_BYTES: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolLocation {
    Sandbox,
    Host,
}

impl ToolLocation {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox",
            Self::Host => "host",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "sandbox" | "" => Some(Self::Sandbox),
            "host" => Some(Self::Host),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HostApprovalPolicy {
    AskEachTime,
    Automatic,
}

impl HostApprovalPolicy {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::AskEachTime => "ask-each-time",
            Self::Automatic => "automatic",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "ask-each-time" | "" => Some(Self::AskEachTime),
            "automatic" => Some(Self::Automatic),
            _ => None,
        }
    }

    pub(crate) fn automatic(self) -> bool {
        self == Self::Automatic
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExecutionSettings {
    pub(crate) model: ModelSelection,
    pub(crate) instructions: String,
    pub(crate) tools: Vec<ToolId>,
    pub(crate) environment: EnvironmentId,
    pub(crate) network: NetworkAccess,
    pub(crate) directories: Vec<DirectoryGrant>,
    pub(crate) git_destination: Option<DirectoryGrantId>,
    pub(crate) location: ToolLocation,
    pub(crate) host_approval: HostApprovalPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DirectoryGrant {
    pub(crate) id: DirectoryGrantId,
    pub(crate) host_path: PathBuf,
    pub(crate) identity: CanonicalDirectoryIdentity,
    pub(crate) alias: String,
    pub(crate) access: DirectoryAccess,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct DirectoryGrantId([u8; 16]);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CanonicalDirectoryIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DirectoryAccess {
    ReadOnly,
    ReviewBeforeApply,
    DirectWrite,
}

impl DirectoryAccess {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::ReviewBeforeApply => "review-before-apply",
            Self::DirectWrite => "direct-write",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "read-only" => Some(Self::ReadOnly),
            "review-before-apply" => Some(Self::ReviewBeforeApply),
            "direct-write" => Some(Self::DirectWrite),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DirectoryGrantError {
    Random,
    Path,
    Unavailable,
    Duplicate,
    Overlap,
    Full,
    Invalid,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct ExecutionSettingsFile {
    model: ModelSelection,
    instructions: String,
    tools: Vec<String>,
    environment: String,
    network: String,
    network_domains: Vec<String>,
    directories: Vec<DirectoryGrantFile>,
    #[serde(default)]
    git_destination: Option<String>,
    location: String,
    host_approval: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(super) struct DirectoryGrantFile {
    id: String,
    host_path: PathBuf,
    identity: CanonicalDirectoryIdentity,
    alias: String,
    access: DirectoryAccess,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DirectoryGrantForm {
    id: String,
    host_path: PathBuf,
    identity: CanonicalDirectoryIdentity,
    alias: String,
    access: String,
}

impl ExecutionSettings {
    // This union supplies capture and application authority, never a model step's tool authority.
    pub(crate) fn combined<'a>(settings: impl IntoIterator<Item = &'a Self>) -> Option<Self> {
        let mut settings = settings.into_iter();
        let mut combined = settings.next()?.clone();
        for phase in settings {
            for tool in &phase.tools {
                if !combined.tools.contains(tool) {
                    combined.tools.push(*tool);
                }
            }
            for grant in &phase.directories {
                if let Some(existing) = combined
                    .directories
                    .iter_mut()
                    .find(|existing| existing.identity == grant.identity)
                {
                    if existing.access != grant.access {
                        if existing.access == DirectoryAccess::ReadOnly {
                            existing.access = grant.access;
                        } else if grant.access != DirectoryAccess::ReadOnly {
                            return None;
                        }
                    }
                } else {
                    combined.directories.push(grant.clone());
                }
            }
            if combined.location != phase.location
                || combined.host_approval != phase.host_approval
                || combined.git_destination != phase.git_destination
            {
                return None;
            }
        }
        Some(combined)
    }

    pub(crate) fn new(
        model: ModelSelection,
        instructions: String,
        tools: Vec<ToolId>,
        environment: EnvironmentId,
    ) -> Option<Self> {
        validate_text_and_tools(&instructions, &tools)?;
        Some(Self {
            model,
            instructions,
            tools,
            environment,
            network: NetworkAccess::None,
            directories: Vec::new(),
            git_destination: None,
            location: ToolLocation::Sandbox,
            host_approval: HostApprovalPolicy::AskEachTime,
        })
    }

    pub(crate) fn with_network(mut self, network: NetworkAccess) -> Option<Self> {
        self.network = network.validate().ok()?;
        Some(self)
    }

    pub(crate) fn with_directories(mut self, directories: Vec<DirectoryGrant>) -> Option<Self> {
        validate_directories(&directories).ok()?;
        if self
            .git_destination
            .is_some_and(|id| directories.iter().all(|grant| grant.id != id))
        {
            self.git_destination = None;
        }
        self.directories = directories;
        Some(self)
    }

    pub(crate) fn with_git_destination(mut self, id: Option<DirectoryGrantId>) -> Option<Self> {
        if id.is_some_and(|id| self.directories.iter().all(|grant| grant.id != id)) {
            return None;
        }
        self.git_destination = id;
        Some(self)
    }

    pub(crate) fn git_destination_grant(&self) -> Option<&DirectoryGrant> {
        let id = self.git_destination?;
        self.directories.iter().find(|grant| grant.id == id)
    }

    pub(crate) fn with_location(mut self, location: ToolLocation) -> Self {
        self.location = location;
        self
    }

    pub(crate) fn with_host_approval(mut self, host_approval: HostApprovalPolicy) -> Self {
        self.host_approval = host_approval;
        self
    }

    pub(crate) fn host_tools(&self) -> bool {
        self.location == ToolLocation::Host && self.tools.contains(&ToolId::Run)
    }

    pub(crate) fn automatic_host_commands(&self) -> bool {
        self.host_tools() && self.host_approval.automatic()
    }

    pub(crate) fn to_file(&self) -> ExecutionSettingsFile {
        ExecutionSettingsFile {
            model: self.model.clone(),
            instructions: self.instructions.clone(),
            tools: self
                .tools
                .iter()
                .map(|tool| tool.as_str().to_owned())
                .collect(),
            environment: self.environment.as_hex(),
            network: self.network.as_str().to_owned(),
            network_domains: self.network.domains().to_vec(),
            directories: self
                .directories
                .iter()
                .map(DirectoryGrantFile::from)
                .collect(),
            git_destination: self.git_destination.map(DirectoryGrantId::as_hex),
            location: self.location.as_str().to_owned(),
            host_approval: self.host_approval.as_str().to_owned(),
        }
    }

    pub(crate) fn from_file(file: ExecutionSettingsFile) -> Option<Self> {
        let tools = file
            .tools
            .into_iter()
            .map(|tool| ToolId::parse(&tool))
            .collect::<Option<Vec<_>>>()?;
        let environment = EnvironmentId::parse(&file.environment)?;
        let network =
            NetworkAccess::parse_form(&file.network, &file.network_domains.join("\n")).ok()?;
        let directories = file
            .directories
            .into_iter()
            .map(DirectoryGrantFile::into_grant)
            .collect::<Option<Vec<_>>>()?;
        let git_destination = match file.git_destination.as_deref() {
            None | Some("") => None,
            Some(raw) => Some(DirectoryGrantId::parse(raw)?),
        };
        Some(
            Self::new(file.model, file.instructions, tools, environment)?
                .with_network(network)?
                .with_directories(directories)?
                .with_git_destination(git_destination)?
                .with_location(ToolLocation::parse(&file.location)?)
                .with_host_approval(HostApprovalPolicy::parse(&file.host_approval)?),
        )
    }
}

pub(super) fn validate_text_and_tools(instructions: &str, tools: &[ToolId]) -> Option<()> {
    (instructions.len() <= MAXIMUM_INSTRUCTION_BYTES
        && !instructions
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
        && tools.len() <= ToolId::ALL.len()
        && !tools
            .iter()
            .enumerate()
            .any(|(index, tool)| tools[..index].contains(tool)))
    .then_some(())
}

impl From<&DirectoryGrant> for DirectoryGrantFile {
    fn from(grant: &DirectoryGrant) -> Self {
        Self {
            id: grant.id.as_hex(),
            host_path: grant.host_path.clone(),
            identity: grant.identity,
            alias: grant.alias.clone(),
            access: grant.access,
        }
    }
}

impl DirectoryGrantFile {
    pub(super) fn into_grant(self) -> Option<DirectoryGrant> {
        Some(DirectoryGrant {
            id: DirectoryGrantId::parse(&self.id)?,
            host_path: self.host_path,
            identity: self.identity,
            alias: self.alias,
            access: self.access,
        })
    }
}

impl DirectoryGrant {
    pub(crate) fn from_selected(
        selected: &Path,
        existing: &[Self],
    ) -> Result<Self, DirectoryGrantError> {
        if existing.len() >= MAXIMUM_DIRECTORY_GRANTS {
            return Err(DirectoryGrantError::Full);
        }
        let host_path = canonical_directory(selected)?;
        let identity = directory_identity(&host_path)?;
        if existing
            .iter()
            .any(|grant| grant.host_path == host_path || grant.identity == identity)
        {
            return Err(DirectoryGrantError::Duplicate);
        }
        if existing
            .iter()
            .any(|grant| paths_overlap(&grant.host_path, &host_path))
        {
            return Err(DirectoryGrantError::Overlap);
        }
        let alias = available_alias(&host_path, existing);
        Ok(Self {
            id: DirectoryGrantId::generate()?,
            host_path,
            identity,
            alias,
            access: DirectoryAccess::ReadOnly,
        })
    }

    pub(crate) fn revalidate(&self) -> Result<(), DirectoryGrantError> {
        let canonical = canonical_directory(&self.host_path)?;
        if canonical != self.host_path || directory_identity(&canonical)? != self.identity {
            return Err(DirectoryGrantError::Unavailable);
        }
        Ok(())
    }

    pub(crate) fn is_available(&self) -> bool {
        self.revalidate().is_ok()
    }

    pub(crate) fn guest_path(&self) -> String {
        format!("/access/{}", self.alias)
    }

    pub(crate) fn form_value(&self) -> String {
        serde_json::to_string(&DirectoryGrantForm {
            id: self.id.as_hex(),
            host_path: self.host_path.clone(),
            identity: self.identity,
            alias: self.alias.clone(),
            access: self.access.as_str().to_owned(),
        })
        .expect("directory grant form")
    }

    pub(crate) fn parse_form(value: &str) -> Option<Self> {
        if value.len() > MAXIMUM_FORM_GRANT_BYTES {
            return None;
        }
        let form: DirectoryGrantForm = serde_json::from_str(value).ok()?;
        let grant = Self {
            id: DirectoryGrantId::parse(&form.id)?,
            host_path: form.host_path,
            identity: form.identity,
            alias: form.alias,
            access: DirectoryAccess::parse(&form.access)?,
        };
        validate_directories(std::slice::from_ref(&grant)).ok()?;
        Some(grant)
    }
}

impl DirectoryGrantId {
    pub(crate) fn generate() -> Result<Self, DirectoryGrantError> {
        let mut bytes = [0_u8; 16];
        SysRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| DirectoryGrantError::Random)?;
        Ok(Self(bytes))
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        crate::hex::decode(value).map(Self)
    }

    pub(crate) fn as_hex(self) -> String {
        crate::hex::encode(&self.0)
    }
}

impl DirectoryGrantError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Random => "Power Plant could not create a directory grant. Try again.",
            Self::Path => "Choose an absolute directory path.",
            Self::Unavailable => "That directory is unavailable or changed at the saved path.",
            Self::Duplicate => "That directory already has access.",
            Self::Overlap => "Directory grants cannot overlap.",
            Self::Full => "This conversation has the maximum of eight directories.",
            Self::Invalid => "That directory grant is not valid.",
        }
    }
}

pub(crate) fn validate_directories(
    directories: &[DirectoryGrant],
) -> Result<(), DirectoryGrantError> {
    if directories.len() > MAXIMUM_DIRECTORY_GRANTS {
        return Err(DirectoryGrantError::Full);
    }
    for (index, grant) in directories.iter().enumerate() {
        if !valid_stored_path(&grant.host_path)
            || !valid_alias(&grant.alias)
            || directories[..index].iter().any(|previous| {
                previous.id == grant.id
                    || previous.alias == grant.alias
                    || previous.host_path == grant.host_path
                    || previous.identity == grant.identity
            })
        {
            return Err(DirectoryGrantError::Invalid);
        }
        if directories[..index]
            .iter()
            .any(|previous| paths_overlap(&previous.host_path, &grant.host_path))
        {
            return Err(DirectoryGrantError::Overlap);
        }
    }
    Ok(())
}

fn canonical_directory(path: &Path) -> Result<PathBuf, DirectoryGrantError> {
    if !valid_stored_path(path) {
        return Err(DirectoryGrantError::Path);
    }
    let metadata = std::fs::metadata(path).map_err(|_| DirectoryGrantError::Unavailable)?;
    if !metadata.is_dir() {
        return Err(DirectoryGrantError::Unavailable);
    }
    std::fs::canonicalize(path).map_err(|_| DirectoryGrantError::Unavailable)
}

fn valid_stored_path(path: &Path) -> bool {
    let raw = path.to_string_lossy();
    path.is_absolute()
        && raw.len() <= crate::agents::MAXIMUM_PATH_BYTES
        && !raw.chars().any(char::is_control)
}

#[cfg(unix)]
fn directory_identity(path: &Path) -> Result<CanonicalDirectoryIdentity, DirectoryGrantError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).map_err(|_| DirectoryGrantError::Unavailable)?;
    Ok(CanonicalDirectoryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(not(unix))]
fn directory_identity(_path: &Path) -> Result<CanonicalDirectoryIdentity, DirectoryGrantError> {
    Err(DirectoryGrantError::Unavailable)
}

fn available_alias(path: &Path, existing: &[DirectoryGrant]) -> String {
    let raw = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("directory");
    let mut base = String::new();
    let mut separator = false;
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !base.is_empty() {
                base.push('-');
            }
            separator = false;
            base.push(character.to_ascii_lowercase());
        } else {
            separator = true;
        }
    }
    if base.is_empty() {
        base.push_str("directory");
    }
    if !base.as_bytes()[0].is_ascii_alphabetic() {
        base.insert_str(0, "d-");
    }
    base.truncate(MAXIMUM_ALIAS_BYTES);
    if valid_alias(&base) && existing.iter().all(|grant| grant.alias != base) {
        return base;
    }
    for number in 2..=MAXIMUM_DIRECTORY_GRANTS + 1 {
        let suffix = format!("-{number}");
        let mut candidate = base.clone();
        candidate.truncate(MAXIMUM_ALIAS_BYTES - suffix.len());
        candidate.push_str(&suffix);
        if existing.iter().all(|grant| grant.alias != candidate) {
            return candidate;
        }
    }
    unreachable!("the grant bound leaves an alias available")
}

pub(crate) fn valid_alias(alias: &str) -> bool {
    // Workflow authority reserves this alias for its legacy primary source.
    alias != "project"
        && !alias.is_empty()
        && alias.len() <= MAXIMUM_ALIAS_BYTES
        && alias.as_bytes()[0].is_ascii_alphabetic()
        && alias
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

#[cfg(test)]
mod tests;
