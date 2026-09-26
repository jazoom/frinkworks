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
    pub(crate) location: ToolLocation,
    pub(crate) host_approval: HostApprovalPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DirectoryGrant {
    pub(crate) id: DirectoryGrantId,
    pub(crate) host_path: PathBuf,
    pub(crate) alias: String,
    pub(crate) access: DirectoryAccess,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct DirectoryGrantId([u8; 16]);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DirectoryAccess {
    Read,
    Write,
}

impl DirectoryAccess {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
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
    location: String,
    host_approval: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(super) struct DirectoryGrantFile {
    id: String,
    host_path: PathBuf,
    alias: String,
    access: DirectoryAccess,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DirectoryGrantForm {
    id: String,
    host_path: PathBuf,
    alias: String,
    access: String,
}

impl ExecutionSettings {
    // This union describes run-wide requested access, never a model step's tool authority.
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
                    .find(|existing| existing.host_path == grant.host_path)
                {
                    if existing.access != grant.access {
                        if existing.access == DirectoryAccess::Read {
                            existing.access = grant.access;
                        } else if grant.access != DirectoryAccess::Read {
                            return None;
                        }
                    }
                } else {
                    combined.directories.push(grant.clone());
                }
            }
            if combined.location != phase.location || combined.host_approval != phase.host_approval
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
        self.directories = directories;
        Some(self)
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
        self.location == ToolLocation::Host && !self.tools.is_empty()
    }

    pub(crate) fn automatic_host_commands(&self) -> bool {
        self.host_approval.automatic()
    }

    pub(crate) fn host_access_allowed(&self) -> bool {
        self.location != ToolLocation::Host
            || self
                .directories
                .iter()
                .all(|grant| grant.access == DirectoryAccess::Write)
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
        Some(
            Self::new(file.model, file.instructions, tools, environment)?
                .with_network(network)?
                .with_directories(directories)?
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
        if existing.iter().any(|grant| grant.host_path == host_path) {
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
            alias,
            access: DirectoryAccess::Read,
        })
    }

    // Approval follows this canonical location, including a replacement directory at the same path.
    pub(crate) fn revalidate(&self) -> Result<(), DirectoryGrantError> {
        let canonical = canonical_directory(&self.host_path)?;
        if canonical != self.host_path {
            return Err(DirectoryGrantError::Unavailable);
        }
        Ok(())
    }

    pub(crate) fn open_directory(&self) -> Result<cap_std::fs::Dir, DirectoryGrantError> {
        self.revalidate()?;
        let directory = open_directory_without_links(&self.host_path)
            .map_err(|_| DirectoryGrantError::Unavailable)?;
        self.revalidate()?;
        Ok(directory)
    }

    pub(crate) fn is_available(&self) -> bool {
        self.revalidate().is_ok()
    }

    pub(crate) fn requires_access_consent(&self, data_root: &Path) -> bool {
        self.access == DirectoryAccess::Write
            || crate::execution::authority::sensitive_directory(&self.host_path, data_root)
    }

    pub(crate) fn guest_path(&self) -> String {
        format!("/access/{}", self.alias)
    }

    pub(crate) fn form_value(&self) -> String {
        serde_json::to_string(&DirectoryGrantForm {
            id: self.id.as_hex(),
            host_path: self.host_path.clone(),
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
            Self::Random => "Frinkworks could not create a directory grant. Try again.",
            Self::Path => "Choose an absolute directory path.",
            Self::Unavailable => {
                "That directory is unavailable or no longer resolves to its saved path."
            }
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

fn open_directory_without_links(path: &Path) -> std::io::Result<cap_std::fs::Dir> {
    if !path.is_absolute() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    let root = path
        .ancestors()
        .last()
        .ok_or(std::io::ErrorKind::InvalidInput)?;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| std::io::ErrorKind::InvalidInput)?;
    let mut directory = cap_primitives::fs::open_ambient_dir(root, cap_std::ambient_authority())?;
    // Each open uses the retained parent handle and rejects links in that component.
    // Separate canonical-path checks cannot prevent a transient symbolic-link redirect.
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(std::io::ErrorKind::InvalidInput.into());
        };
        directory = cap_primitives::fs::open_dir_nofollow(&directory, Path::new(name))?;
    }
    Ok(cap_std::fs::Dir::from_std_file(directory))
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
