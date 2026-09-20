use serde::{Deserialize, Serialize};

use crate::{
    agents::{NetworkAccess, ToolId},
    environments::EnvironmentId,
    providers::ModelSelection,
};

use super::{DirectoryGrant, ExecutionSettings, HostApprovalPolicy, ToolLocation};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SettingsOverrides {
    pub(crate) model: Option<ModelSelection>,
    pub(crate) instructions: Option<String>,
    pub(crate) tools: Option<Vec<ToolId>>,
    pub(crate) environment: Option<EnvironmentId>,
    pub(crate) network: Option<NetworkAccess>,
    pub(crate) directories: Option<Vec<DirectoryGrant>>,
    pub(crate) location: Option<ToolLocation>,
    pub(crate) host_approval: Option<HostApprovalPolicy>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct SettingsOverridesFile {
    model: Option<ModelSelection>,
    instructions: Option<String>,
    tools: Option<Vec<String>>,
    environment: Option<String>,
    network: Option<NetworkFile>,
    directories: Option<Vec<super::settings::DirectoryGrantFile>>,
    location: Option<String>,
    host_approval: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NetworkFile {
    mode: String,
    domains: Vec<String>,
}

impl SettingsOverrides {
    pub(crate) fn all(settings: ExecutionSettings) -> Self {
        Self {
            model: Some(settings.model),
            instructions: Some(settings.instructions),
            tools: Some(settings.tools),
            environment: Some(settings.environment),
            network: Some(settings.network),
            directories: Some(settings.directories),
            location: Some(settings.location),
            host_approval: Some(settings.host_approval),
        }
    }

    pub(crate) fn resolve(&self, defaults: &ExecutionSettings) -> ExecutionSettings {
        ExecutionSettings {
            model: self.model.clone().unwrap_or_else(|| defaults.model.clone()),
            instructions: self
                .instructions
                .clone()
                .unwrap_or_else(|| defaults.instructions.clone()),
            tools: self.tools.clone().unwrap_or_else(|| defaults.tools.clone()),
            environment: self.environment.unwrap_or(defaults.environment),
            network: self
                .network
                .clone()
                .unwrap_or_else(|| defaults.network.clone()),
            directories: self
                .directories
                .clone()
                .unwrap_or_else(|| defaults.directories.clone()),
            location: self.location.unwrap_or(defaults.location),
            host_approval: self.host_approval.unwrap_or(defaults.host_approval),
        }
    }

    pub(crate) fn validate(&self) -> Option<()> {
        super::settings::validate_text_and_tools(
            self.instructions.as_deref().unwrap_or_default(),
            self.tools.as_deref().unwrap_or_default(),
        )?;
        if let Some(network) = &self.network {
            network.clone().validate().ok()?;
        }
        if let Some(directories) = &self.directories {
            super::validate_directories(directories).ok()?;
        }
        Some(())
    }

    pub(crate) fn to_file(&self) -> SettingsOverridesFile {
        SettingsOverridesFile {
            model: self.model.clone(),
            instructions: self.instructions.clone(),
            tools: self
                .tools
                .as_ref()
                .map(|tools| tools.iter().map(|tool| tool.as_str().to_owned()).collect()),
            environment: self.environment.map(|environment| environment.as_hex()),
            network: self.network.as_ref().map(|network| NetworkFile {
                mode: network.as_str().to_owned(),
                domains: network.domains().to_vec(),
            }),
            directories: self.directories.as_ref().map(|grants| {
                grants
                    .iter()
                    .map(super::settings::DirectoryGrantFile::from)
                    .collect()
            }),
            location: self.location.map(|location| location.as_str().to_owned()),
            host_approval: self.host_approval.map(|policy| policy.as_str().to_owned()),
        }
    }

    pub(crate) fn from_file(file: SettingsOverridesFile) -> Option<Self> {
        let value = Self {
            model: file.model,
            instructions: file.instructions,
            tools: match file.tools {
                Some(tools) => Some(
                    tools
                        .iter()
                        .map(|tool| ToolId::parse(tool))
                        .collect::<Option<Vec<_>>>()?,
                ),
                None => None,
            },
            environment: match file.environment {
                Some(environment) => Some(EnvironmentId::parse(&environment)?),
                None => None,
            },
            network: match file.network {
                Some(network) => Some(
                    NetworkAccess::parse_form(&network.mode, &network.domains.join("\n")).ok()?,
                ),
                None => None,
            },
            directories: match file.directories {
                Some(grants) => Some(
                    grants
                        .into_iter()
                        .map(super::settings::DirectoryGrantFile::into_grant)
                        .collect::<Option<Vec<_>>>()?,
                ),
                None => None,
            },
            location: match file.location {
                Some(location) => Some(ToolLocation::parse(&location)?),
                None => None,
            },
            host_approval: match file.host_approval {
                Some(policy) => Some(HostApprovalPolicy::parse(&policy)?),
                None => None,
            },
        };
        value.validate()?;
        Some(value)
    }
}
