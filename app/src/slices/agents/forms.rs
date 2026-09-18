use std::path::PathBuf;

use serde::Deserialize;

use crate::agents::{
    AccessMode, AgentDraft, AgentError, AgentRecord, DirectoryGrant, MAXIMUM_GRANTS,
    MAXIMUM_INSTRUCTION_BYTES, MAXIMUM_NAME_BYTES, MAXIMUM_NETWORK_TEXT_BYTES, MAXIMUM_PATH_BYTES,
    NetworkAccess, ToolId,
};
use crate::providers::{ModelSelection, ProviderKind, ThinkingEffort};

pub(super) const REVISION_MESSAGE: &str = "Reload the agent and try again.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FormError {
    Intent,
    Index,
    UnknownField,
    DuplicateField,
    Sparse,
    Excessive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FormIntent {
    Save,
    AddDirectory,
    RemoveDirectory(usize),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DirectoryDraft {
    pub(super) alias: String,
    pub(super) path: String,
    pub(super) access: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AgentFormState {
    pub(super) name: String,
    pub(super) instructions: String,
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) thinking: String,
    pub(super) primary: String,
    pub(super) revision: String,
    pub(super) tools: Vec<ToolId>,
    pub(super) network: String,
    pub(super) network_domains: String,
    pub(super) directories: Vec<DirectoryDraft>,
}

impl FormError {
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::Intent => "That form action is not valid.",
            Self::Index => "That form row is not valid.",
            Self::UnknownField => "That form includes an unknown field.",
            Self::DuplicateField => "That form includes a duplicate field.",
            Self::Sparse => "That form row is not valid.",
            Self::Excessive => "That form has too many rows.",
        }
    }
}

impl AgentFormState {
    pub(super) fn blank() -> Self {
        Self {
            name: String::new(),
            instructions: String::new(),
            provider: String::new(),
            model: String::new(),
            thinking: String::new(),
            primary: String::new(),
            revision: String::new(),
            tools: crate::slices::execution_settings::page::initial_tool_ids(),
            network: NetworkAccess::None.as_str().to_owned(),
            network_domains: String::new(),
            directories: Vec::new(),
        }
    }

    pub(super) fn from_record(record: &AgentRecord) -> Self {
        Self {
            name: record.name.clone(),
            instructions: record.instructions.clone(),
            provider: record
                .selection
                .as_ref()
                .map(|selection| selection.provider.as_str().to_owned())
                .unwrap_or_default(),
            model: record
                .selection
                .as_ref()
                .map(|selection| selection.model.clone())
                .unwrap_or_default(),
            thinking: record
                .selection
                .as_ref()
                .and_then(|selection| selection.thinking.as_ref())
                .map(|effort| effort.as_str().to_owned())
                .unwrap_or_default(),
            primary: record.primary_directory.clone(),
            revision: record.revision.to_string(),
            tools: record.tools.clone(),
            network: record.network.as_str().to_owned(),
            network_domains: record.network.domains().join("\n"),
            directories: record
                .directories
                .iter()
                .map(|grant| DirectoryDraft {
                    alias: grant.alias.clone(),
                    path: grant.host_path.to_string_lossy().into_owned(),
                    access: grant.access.as_str().to_owned(),
                })
                .collect(),
        }
    }

    pub(super) fn parse(pairs: Vec<(String, String)>) -> Result<(Self, FormIntent), FormError> {
        let mut seen = Vec::new();
        let mut name = String::new();
        let mut instructions = String::new();
        let mut provider = String::new();
        let mut model = String::new();
        let mut thinking = String::new();
        let mut primary = String::new();
        let mut revision = String::new();
        let mut network = NetworkAccess::None.as_str().to_owned();
        let mut network_domains = String::new();
        let mut intent = None;
        let mut tools = Vec::new();
        let mut directory_fields = Vec::new();
        for (key, value) in pairs {
            if seen.iter().any(|item: &String| item == &key) {
                return Err(FormError::DuplicateField);
            }
            seen.push(key.clone());
            match parse_field(&key)? {
                Field::Name => name = value,
                Field::Instructions => instructions = value,
                Field::Provider => provider = value,
                Field::Model => model = value,
                Field::Thinking => thinking = value,
                Field::Primary => primary = value,
                Field::Revision => revision = value,
                Field::Network => network = value,
                Field::NetworkDomains => network_domains = value,
                Field::Intent => intent = Some(parse_intent(&value)?),
                Field::Tool(tool) => {
                    if is_checked(&value) && !tools.contains(&tool) {
                        tools.push(tool);
                    }
                }
                Field::Directory { index, part } => directory_fields.push((index, part, value)),
            }
        }
        let intent = intent.ok_or(FormError::Intent)?;
        let directories = collect_directories(directory_fields)?;
        tools = ToolId::ALL
            .into_iter()
            .filter(|tool| tools.contains(tool))
            .collect();
        Ok((
            Self {
                name,
                instructions,
                provider,
                model,
                thinking,
                primary,
                revision,
                tools,
                network,
                network_domains,
                directories,
            },
            intent,
        ))
    }

    pub(super) fn apply(&mut self, intent: FormIntent) -> Result<(), FormError> {
        match intent {
            FormIntent::Save => Ok(()),
            FormIntent::AddDirectory => {
                if self.directories.len() >= MAXIMUM_GRANTS {
                    return Err(FormError::Excessive);
                }
                self.directories.push(blank_directory());
                Ok(())
            }
            FormIntent::RemoveDirectory(index) => {
                if index >= self.directories.len() {
                    return Err(FormError::Index);
                }
                self.directories.remove(index);
                if self.directories.is_empty() {
                    self.primary.clear();
                } else if !self
                    .directories
                    .iter()
                    .any(|row| row.alias.trim() == self.primary.trim())
                {
                    self.primary = self.directories[0].alias.clone();
                }
                Ok(())
            }
        }
    }

    pub(super) fn revision(&self) -> Result<Option<u32>, &'static str> {
        parse_revision(&self.revision)
    }

    pub(super) fn draft(&self) -> Result<AgentDraft, AgentError> {
        if self.name.len() > MAXIMUM_NAME_BYTES {
            return Err(AgentError::Name);
        }
        if self.instructions.len() > MAXIMUM_INSTRUCTION_BYTES {
            return Err(AgentError::Instructions);
        }
        if self.network_domains.len() > MAXIMUM_NETWORK_TEXT_BYTES {
            return Err(AgentError::Network);
        }
        let network = NetworkAccess::parse_form(&self.network, &self.network_domains)?;
        let selection = parse_selection(&self.provider, &self.model, &self.thinking)?;
        let mut directories = Vec::new();
        for row in &self.directories {
            let alias = row.alias.trim();
            let path = row.path.trim();
            if alias.is_empty() && path.is_empty() {
                continue;
            }
            if path.len() > MAXIMUM_PATH_BYTES || path.chars().any(char::is_control) {
                return Err(AgentError::Path);
            }
            let Some(access) = AccessMode::parse(&row.access) else {
                return Err(AgentError::Path);
            };
            directories.push(DirectoryGrant {
                alias: alias.to_owned(),
                host_path: PathBuf::from(path),
                access,
            });
        }
        Ok(AgentDraft {
            name: self.name.clone(),
            instructions: self.instructions.clone(),
            selection,
            tools: self.tools.clone(),
            network,
            directories,
            primary_directory: self.primary.clone(),
        })
    }
}

#[derive(Deserialize)]
pub(super) struct DeleteForm {
    #[serde(default)]
    pub(super) revision: String,
}

impl DeleteForm {
    pub(super) fn revision(&self) -> Result<Option<u32>, &'static str> {
        parse_revision(&self.revision)
    }
}

#[derive(Deserialize)]
pub(super) struct OrphanForm {
    #[serde(default)]
    pub(super) name: String,
}

#[derive(Clone, Copy)]
enum Field {
    Name,
    Instructions,
    Provider,
    Model,
    Thinking,
    Primary,
    Revision,
    Network,
    NetworkDomains,
    Intent,
    Tool(ToolId),
    Directory { index: usize, part: DirPart },
}

#[derive(Clone, Copy)]
enum DirPart {
    Alias,
    Path,
    Access,
}

fn parse_selection(
    provider: &str,
    model: &str,
    thinking: &str,
) -> Result<Option<ModelSelection>, AgentError> {
    let provider = provider.trim();
    let model = model.trim();
    let thinking = thinking.trim();
    if provider.is_empty() && model.is_empty() && thinking.is_empty() {
        return Ok(None);
    }
    let Some(provider) = ProviderKind::parse(provider) else {
        return Err(AgentError::Model);
    };
    let selected_thinking = if thinking.is_empty() {
        None
    } else {
        ThinkingEffort::new(thinking.to_owned())
    };
    if !thinking.is_empty() && selected_thinking.is_none() {
        return Err(AgentError::Model);
    }
    ModelSelection::new(provider, model.to_owned(), selected_thinking)
        .ok_or(AgentError::Model)
        .map(Some)
}

fn blank_directory() -> DirectoryDraft {
    DirectoryDraft {
        alias: String::new(),
        path: String::new(),
        access: AccessMode::ReadWrite.as_str().to_owned(),
    }
}

fn parse_field(name: &str) -> Result<Field, FormError> {
    match name {
        "name" => Ok(Field::Name),
        "instructions" => Ok(Field::Instructions),
        "provider" => Ok(Field::Provider),
        "model" => Ok(Field::Model),
        "thinking" => Ok(Field::Thinking),
        "primary" => Ok(Field::Primary),
        "revision" => Ok(Field::Revision),
        "network" => Ok(Field::Network),
        "network_domains" => Ok(Field::NetworkDomains),
        "intent" => Ok(Field::Intent),
        other => parse_indexed_field(other),
    }
}

fn parse_indexed_field(name: &str) -> Result<Field, FormError> {
    if let Some(tool) = name.strip_prefix("tool_") {
        let tool = ToolId::parse(tool).ok_or(FormError::UnknownField)?;
        return Ok(Field::Tool(tool));
    }
    let (prefix, index) = name.split_once('_').ok_or(FormError::UnknownField)?;
    let index = parse_index(index)?;
    if index >= MAXIMUM_GRANTS {
        return Err(FormError::Excessive);
    }
    let part = match prefix {
        "alias" => DirPart::Alias,
        "path" => DirPart::Path,
        "access" => DirPart::Access,
        _ => return Err(FormError::UnknownField),
    };
    Ok(Field::Directory { index, part })
}

fn parse_index(raw: &str) -> Result<usize, FormError> {
    if raw.is_empty() || (raw.len() > 1 && raw.starts_with('0')) {
        return Err(FormError::Index);
    }
    if !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(FormError::Index);
    }
    raw.parse().map_err(|_| FormError::Index)
}

fn parse_intent(raw: &str) -> Result<FormIntent, FormError> {
    match raw {
        "save" => Ok(FormIntent::Save),
        "add-directory" => Ok(FormIntent::AddDirectory),
        other => {
            let (action, index) = other.split_once(':').ok_or(FormError::Intent)?;
            if action != "remove-directory" {
                return Err(FormError::Intent);
            }
            let index = parse_index(index)?;
            if index >= MAXIMUM_GRANTS {
                return Err(FormError::Index);
            }
            Ok(FormIntent::RemoveDirectory(index))
        }
    }
}

fn collect_directories(
    fields: Vec<(usize, DirPart, String)>,
) -> Result<Vec<DirectoryDraft>, FormError> {
    let count = dense_count(fields.iter().map(|(index, _, _)| *index))?;
    let mut directories = vec![blank_directory(); count];
    for (index, part, value) in fields {
        let row = &mut directories[index];
        match part {
            DirPart::Alias => row.alias = value,
            DirPart::Path => row.path = value,
            DirPart::Access => row.access = value,
        }
    }
    Ok(directories)
}

fn dense_count(indices: impl Iterator<Item = usize>) -> Result<usize, FormError> {
    let mut max = None;
    let mut seen = Vec::new();
    for index in indices {
        if !seen.contains(&index) {
            seen.push(index);
        }
        max = Some(max.map_or(index, |current: usize| current.max(index)));
    }
    let Some(max) = max else {
        return Ok(0);
    };
    let count = max.checked_add(1).ok_or(FormError::Excessive)?;
    if seen.len() != count {
        return Err(FormError::Sparse);
    }
    Ok(count)
}

fn is_checked(value: &str) -> bool {
    matches!(value, "on" | "true" | "1")
}

fn parse_revision(raw: &str) -> Result<Option<u32>, &'static str> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    if raw == "0" || (raw.len() > 1 && raw.starts_with('0')) {
        return Err(REVISION_MESSAGE);
    }
    if !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(REVISION_MESSAGE);
    }
    raw.parse().map(Some).map_err(|_| REVISION_MESSAGE)
}

#[cfg(test)]
mod tests;
