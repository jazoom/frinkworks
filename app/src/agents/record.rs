use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::id::AgentId;
use super::tool_id::ToolId;
use crate::providers::ModelSelection;

pub(crate) const AGENT_RECORD_VERSION: u32 = 1;
pub(crate) const MAXIMUM_AGENTS: usize = 32;
pub(crate) const MAXIMUM_GRANTS: usize = 8;
pub(crate) const MAXIMUM_NETWORK_DOMAINS: usize = 32;
pub(crate) const MAXIMUM_NETWORK_DOMAIN_BYTES: usize = 253;
pub(crate) const MAXIMUM_NETWORK_TEXT_BYTES: usize = 8_127;
pub(crate) const MAXIMUM_NAME_BYTES: usize = 80;
pub(crate) const MAXIMUM_INSTRUCTION_BYTES: usize = 32_768;
pub(crate) const MAXIMUM_ALIAS_BYTES: usize = 32;
pub(crate) const MAXIMUM_PATH_BYTES: usize = 4_096;
pub(crate) const GUEST_PROJECT: &str = "/project";
pub(crate) const GUEST_ACCESS_ROOT: &str = "/access";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AccessMode {
    ReadOnly,
    ReadWrite,
}

impl AccessMode {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "read-only" => Some(Self::ReadOnly),
            "read-write" => Some(Self::ReadWrite),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::ReadWrite => "read-write",
        }
    }

    pub(crate) fn is_writable(self) -> bool {
        self == Self::ReadWrite
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum NetworkAccess {
    #[default]
    None,
    Restricted(Vec<String>),
    Public,
}

impl NetworkAccess {
    pub(crate) fn parse_form(mode: &str, domains: &str) -> Result<Self, AgentError> {
        if domains.len() > MAXIMUM_NETWORK_TEXT_BYTES {
            return Err(AgentError::Network);
        }
        let access = match mode.trim() {
            "none" => Self::None,
            "restricted" => Self::Restricted(
                domains
                    .lines()
                    .filter(|domain| !domain.trim().is_empty())
                    .map(str::to_owned)
                    .collect(),
            ),
            "public" => Self::Public,
            _ => return Err(AgentError::Network),
        };
        normalise_network(access)
    }

    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Restricted(_) => "restricted",
            Self::Public => "public",
        }
    }

    pub(crate) fn domains(&self) -> &[String] {
        match self {
            Self::Restricted(domains) => domains,
            Self::None | Self::Public => &[],
        }
    }

    pub(crate) fn validate(self) -> Result<Self, AgentError> {
        normalise_network(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DirectoryGrant {
    pub(crate) alias: String,
    pub(crate) host_path: PathBuf,
    pub(crate) access: AccessMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AgentRecord {
    pub(crate) id: AgentId,
    pub(crate) revision: u32,
    pub(crate) name: String,
    pub(crate) instructions: String,
    pub(crate) selection: Option<ModelSelection>,
    pub(crate) tools: Vec<ToolId>,
    pub(crate) network: NetworkAccess,
    pub(crate) directories: Vec<DirectoryGrant>,
    pub(crate) primary_directory: String,
}

#[derive(Clone, Debug)]
pub(crate) struct AgentDraft {
    pub(crate) name: String,
    pub(crate) instructions: String,
    pub(crate) selection: Option<ModelSelection>,
    pub(crate) tools: Vec<ToolId>,
    pub(crate) network: NetworkAccess,
    pub(crate) directories: Vec<DirectoryGrant>,
    pub(crate) primary_directory: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentError {
    Random,
    Persist,
    Corrupt,
    Full,
    Missing,
    Conflict,
    Revision,
    Name,
    Instructions,
    Model,
    Tools,
    Network,
    Alias,
    DuplicateAlias,
    Path,
    PathMissing,
    NotADirectory,
    PathAccess,
    NestedPath,
    Primary,
    GrantCount,
}

impl AgentError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Random => "Frinkworks could not create an agent identifier. Try again.",
            Self::Persist => "Frinkworks could not store the agent. Try again.",
            Self::Corrupt => "An agent record is unreadable.",
            Self::Full => "The agent catalogue is full.",
            Self::Missing => "That agent does not exist.",
            Self::Conflict => "That agent changed in another tab. Reload it.",
            Self::Revision => "Frinkworks cannot update this agent again.",
            Self::Name => "Enter a name of at most 80 bytes.",
            Self::Instructions => "Those instructions are too long.",
            Self::Model => "Enter a valid model preference.",
            Self::Tools => "Choose tools from the built-in set.",
            Self::Network => {
                "Choose valid network access. Restricted access needs 1 to 32 domains."
            }
            Self::Alias => {
                "Enter a directory alias that uses letters, numbers, hyphen or underscore."
            }
            Self::DuplicateAlias => "Directory aliases must be unique.",
            Self::Path => "Enter an absolute directory path.",
            Self::PathMissing => "That directory does not exist.",
            Self::NotADirectory => "That path is not a directory.",
            Self::PathAccess => "Frinkworks cannot access that directory.",
            Self::NestedPath => "Directory grants cannot overlap.",
            Self::Primary => "Choose one primary directory from the grants.",
            Self::GrantCount => "Add at most eight directory grants.",
        }
    }
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for AgentError {}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AgentFile {
    pub(super) version: u32,
    pub(super) id: String,
    pub(super) revision: u32,
    pub(super) name: String,
    pub(super) instructions: String,
    #[serde(deserialize_with = "crate::storage::required_option")]
    pub(super) selection: Option<ModelSelection>,
    pub(super) tools: Vec<String>,
    pub(super) network: String,
    pub(super) network_domains: Vec<String>,
    pub(super) directories: Vec<AgentFileGrant>,
    pub(super) primary_directory: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AgentFileGrant {
    pub(super) alias: String,
    pub(super) host_path: String,
    pub(super) access: String,
}

impl AgentRecord {
    pub(super) fn from_file(file: AgentFile) -> Result<Self, AgentError> {
        if file.version != AGENT_RECORD_VERSION {
            return Err(AgentError::Corrupt);
        }
        let id = AgentId::parse(&file.id).ok_or(AgentError::Corrupt)?;
        if file.revision == 0 {
            return Err(AgentError::Corrupt);
        }
        let network = parse_stored_network(&file.network, file.network_domains)?;
        let draft = AgentDraft {
            name: file.name,
            instructions: file.instructions,
            selection: file.selection,
            tools: parse_stored_tools(&file.tools)?,
            network,
            directories: file
                .directories
                .into_iter()
                .map(|grant| {
                    let access = AccessMode::parse(&grant.access).ok_or(AgentError::Corrupt)?;
                    let host_path = PathBuf::from(grant.host_path);
                    if !host_path.is_absolute() {
                        return Err(AgentError::Corrupt);
                    }
                    Ok(DirectoryGrant {
                        alias: grant.alias,
                        host_path,
                        access,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            primary_directory: file.primary_directory,
        };
        let normalised = draft.validate_stored()?;
        Ok(Self {
            id,
            revision: file.revision,
            name: normalised.name,
            instructions: normalised.instructions,
            selection: normalised.selection,
            tools: normalised.tools,
            network: normalised.network,
            directories: normalised.directories,
            primary_directory: normalised.primary_directory,
        })
    }

    pub(super) fn to_file(&self) -> AgentFile {
        AgentFile {
            version: AGENT_RECORD_VERSION,
            id: self.id.as_hex(),
            revision: self.revision,
            name: self.name.clone(),
            instructions: self.instructions.clone(),
            selection: self.selection.clone(),
            tools: self
                .tools
                .iter()
                .map(|tool| tool.as_str().to_owned())
                .collect(),
            network: self.network.as_str().to_owned(),
            network_domains: self.network.domains().to_vec(),
            directories: self
                .directories
                .iter()
                .map(|grant| AgentFileGrant {
                    alias: grant.alias.clone(),
                    host_path: grant.host_path.to_string_lossy().into_owned(),
                    access: grant.access.as_str().to_owned(),
                })
                .collect(),
            primary_directory: self.primary_directory.clone(),
        }
    }
}

impl AgentDraft {
    pub(crate) fn validate(self) -> Result<Self, AgentError> {
        self.validate_inner(true)
    }

    fn validate_stored(self) -> Result<Self, AgentError> {
        self.validate_inner(false)
    }

    fn validate_inner(self, resolve_hosts: bool) -> Result<Self, AgentError> {
        let name = normalise_name(&self.name)?;
        let instructions = normalise_instructions(&self.instructions)?;
        let selection = self.selection.clone().and_then(|selection| {
            ModelSelection::new(selection.provider, selection.model, selection.thinking)
        });
        if self.selection.is_some() && selection.is_none() {
            return Err(AgentError::Model);
        }
        let tools = normalise_tools(&self.tools)?;
        let network = normalise_network(self.network)?;
        if self.directories.len() > MAXIMUM_GRANTS {
            return Err(AgentError::GrantCount);
        }
        let mut directories = Vec::with_capacity(self.directories.len());
        for grant in self.directories {
            let alias = normalise_alias(&grant.alias)?;
            if directories
                .iter()
                .any(|seen: &DirectoryGrant| seen.alias == alias)
            {
                return Err(AgentError::DuplicateAlias);
            }
            let host_path = if resolve_hosts {
                canonical_directory(&grant.host_path)?
            } else {
                stored_directory(&grant.host_path)?
            };
            directories.push(DirectoryGrant {
                alias,
                host_path,
                access: grant.access,
            });
        }
        reject_nested_hosts(&directories)?;
        let primary = self.primary_directory.trim();
        if (directories.is_empty() && !primary.is_empty())
            || (!directories.is_empty()
                && (primary.is_empty() || !directories.iter().any(|grant| grant.alias == primary)))
        {
            return Err(AgentError::Primary);
        }
        let primary_directory = primary.to_owned();
        Ok(Self {
            name,
            instructions,
            selection,
            tools,
            network,
            directories,
            primary_directory,
        })
    }
}

fn parse_stored_network(mode: &str, domains: Vec<String>) -> Result<NetworkAccess, AgentError> {
    match mode {
        "none" if domains.is_empty() => Ok(NetworkAccess::None),
        "restricted" => Ok(NetworkAccess::Restricted(domains)),
        "public" if domains.is_empty() => Ok(NetworkAccess::Public),
        _ => Err(AgentError::Corrupt),
    }
}

fn parse_stored_tools(raw: &[String]) -> Result<Vec<ToolId>, AgentError> {
    let mut tools = Vec::new();
    for name in raw {
        let Some(tool) = ToolId::parse(name) else {
            return Err(AgentError::Corrupt);
        };
        if tools.contains(&tool) {
            return Err(AgentError::Corrupt);
        }
        tools.push(tool);
    }
    Ok(ToolId::ALL
        .into_iter()
        .filter(|tool| tools.contains(tool))
        .collect())
}

fn normalise_network(access: NetworkAccess) -> Result<NetworkAccess, AgentError> {
    let NetworkAccess::Restricted(raw_domains) = access else {
        return Ok(access);
    };
    let mut domains = Vec::with_capacity(raw_domains.len().min(MAXIMUM_NETWORK_DOMAINS));
    for raw in raw_domains {
        let domain = raw
            .trim()
            .trim_start_matches('.')
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if domain.is_empty()
            || domain.len() > MAXIMUM_NETWORK_DOMAIN_BYTES
            || domain.contains('*')
            || microsandbox::NetworkPolicy::builder()
                .default_deny()
                .egress(|rules| rules.allow_domain_suffixes([domain.as_str()]))
                .build()
                .is_err()
        {
            return Err(AgentError::Network);
        }
        if domains.contains(&domain) {
            continue;
        }
        if domains.len() == MAXIMUM_NETWORK_DOMAINS {
            return Err(AgentError::Network);
        }
        domains.push(domain);
    }
    if domains.is_empty() {
        return Err(AgentError::Network);
    }
    Ok(NetworkAccess::Restricted(domains))
}

fn normalise_name(raw: &str) -> Result<String, AgentError> {
    let name = raw.trim();
    if name.is_empty() || name.len() > MAXIMUM_NAME_BYTES || name.chars().any(char::is_control) {
        return Err(AgentError::Name);
    }
    Ok(name.to_owned())
}

fn normalise_instructions(raw: &str) -> Result<String, AgentError> {
    if raw.len() > MAXIMUM_INSTRUCTION_BYTES
        || raw
            .chars()
            .any(|character| character.is_control() && character != '\n' && character != '\t')
    {
        return Err(AgentError::Instructions);
    }
    Ok(raw.to_owned())
}

fn normalise_tools(tools: &[ToolId]) -> Result<Vec<ToolId>, AgentError> {
    if tools.len() > ToolId::ALL.len() {
        return Err(AgentError::Tools);
    }
    let mut unique = Vec::new();
    for tool in tools {
        if unique.contains(tool) {
            return Err(AgentError::Tools);
        }
        unique.push(*tool);
    }
    Ok(ToolId::ALL
        .into_iter()
        .filter(|tool| unique.contains(tool))
        .collect())
}

fn normalise_alias(raw: &str) -> Result<String, AgentError> {
    let alias = raw.trim();
    if alias.is_empty() || alias.len() > MAXIMUM_ALIAS_BYTES {
        return Err(AgentError::Alias);
    }
    let mut characters = alias.chars();
    let Some(first) = characters.next() else {
        return Err(AgentError::Alias);
    };
    if !first.is_ascii_alphabetic() {
        return Err(AgentError::Alias);
    }
    if !characters
        .all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_')
    {
        return Err(AgentError::Alias);
    }
    Ok(alias.to_owned())
}

pub(crate) fn canonical_directory(path: &Path) -> Result<PathBuf, AgentError> {
    let raw = path.to_string_lossy();
    if raw.len() > MAXIMUM_PATH_BYTES || raw.chars().any(char::is_control) {
        return Err(AgentError::Path);
    }
    if !path.is_absolute() {
        return Err(AgentError::Path);
    }
    let metadata = std::fs::metadata(path).map_err(map_fs_error)?;
    if !metadata.is_dir() {
        return Err(AgentError::NotADirectory);
    }
    std::fs::canonicalize(path).map_err(map_fs_error)
}

fn stored_directory(path: &Path) -> Result<PathBuf, AgentError> {
    let raw = path.to_string_lossy();
    if raw.len() > MAXIMUM_PATH_BYTES || raw.chars().any(char::is_control) || !path.is_absolute() {
        return Err(AgentError::Corrupt);
    }
    Ok(path.to_path_buf())
}

fn reject_nested_hosts(directories: &[DirectoryGrant]) -> Result<(), AgentError> {
    for (index, grant) in directories.iter().enumerate() {
        for other in directories.iter().skip(index + 1) {
            if paths_overlap(&grant.host_path, &other.host_path) {
                return Err(AgentError::NestedPath);
            }
        }
    }
    Ok(())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn map_fs_error(error: std::io::Error) -> AgentError {
    match error.kind() {
        std::io::ErrorKind::NotFound => AgentError::PathMissing,
        std::io::ErrorKind::NotADirectory => AgentError::NotADirectory,
        _ => AgentError::PathAccess,
    }
}

pub(crate) fn guest_path_for(alias: &str, primary: &str) -> String {
    if alias == primary {
        GUEST_PROJECT.to_owned()
    } else {
        format!("{GUEST_ACCESS_ROOT}/{alias}")
    }
}

#[cfg(test)]
mod tests;
