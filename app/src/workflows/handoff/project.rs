use crate::{
    agents::{AccessMode, AuthorityOrigin, EffectiveAuthority, PolicyGrant},
    execution::{DirectoryAccess, DirectoryGrant, ExecutionSettings},
    state::AppState,
};

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectAuthority {
    revision: u32,
    project_revision: u32,
    aliases: Vec<String>,
    #[serde(with = "settings_file")]
    pub(crate) settings: ExecutionSettings,
}

impl ProjectAuthority {
    pub(crate) fn capture(
        authority: &EffectiveAuthority,
        mut settings: ExecutionSettings,
    ) -> Result<Self, &'static str> {
        let mut directories = Vec::new();
        for root in authority.policy.grants() {
            let mut grant = DirectoryGrant::from_selected(&root.host_path, &directories)
                .map_err(|_| "A workflow directory is unavailable.")?;
            if root.alias != "project" {
                grant.alias = root.alias.clone();
            }
            grant.access = if root.access.is_writable() {
                DirectoryAccess::ReviewBeforeApply
            } else {
                DirectoryAccess::ReadOnly
            };
            directories.push(grant);
        }
        settings.directories = directories;
        settings.tools = authority.tools.clone();
        settings.network = authority.network.clone();
        settings.location = crate::execution::ToolLocation::Sandbox;
        let settings = ExecutionSettings::from_file(settings.to_file())
            .ok_or("The project authority exceeds the directory bounds.")?;
        Ok(Self {
            revision: authority.revision,
            project_revision: authority.project_revision,
            aliases: authority
                .policy
                .grants()
                .iter()
                .map(|root| root.alias.clone())
                .collect(),
            settings,
        })
    }

    pub(crate) fn valid(&self) -> bool {
        self.revision > 0
            && self.project_revision > 0
            && self.aliases.first().is_some_and(|alias| alias == "project")
            && self.aliases.len() == self.settings.directories.len()
            && self
                .aliases
                .iter()
                .zip(&self.settings.directories)
                .skip(1)
                .all(|(alias, root)| alias == &root.alias)
            && self
                .settings
                .directories
                .iter()
                .skip(1)
                .all(|root| root.access == DirectoryAccess::ReadOnly)
            && self
                .settings
                .directories
                .iter()
                .all(|root| root.access != DirectoryAccess::DirectWrite)
            && self.settings.location == crate::execution::ToolLocation::Sandbox
    }

    pub(crate) fn approved(
        &self,
        state: &AppState,
        run: &super::WorkflowRun,
        session: crate::sessions::SessionId,
    ) -> bool {
        run.conversation_id.is_some_and(|owner| {
            state.sessions.contains_live(&session)
                && state
                    .access_consent
                    .authorised_launch(run.id, session, owner, &self.settings)
        })
    }

    pub(crate) fn resolve(
        &self,
        state: &AppState,
        run: &super::WorkflowRun,
    ) -> Result<EffectiveAuthority, &'static str> {
        if !self.valid() {
            return Err("The pinned project authority is invalid.");
        }
        let project = run
            .project_id
            .and_then(|id| state.projects.get(&id))
            .ok_or("The pinned project is unavailable.")?;
        if project.revision != self.project_revision
            || self.settings.directories[0].host_path != project.host_path
        {
            return Err("The pinned project changed.");
        }
        let mut grants = Vec::new();
        for (root, alias) in self.settings.directories.iter().zip(&self.aliases) {
            root.revalidate()
                .map_err(|_| "A pinned workflow directory changed identity.")?;
            grants.push(PolicyGrant {
                alias: alias.clone(),
                guest_path: crate::agents::guest_path_for(alias, "project"),
                host_path: root.host_path.clone(),
                access: if root.access == DirectoryAccess::ReadOnly {
                    AccessMode::ReadOnly
                } else {
                    AccessMode::ReadWrite
                },
            });
        }
        Ok(EffectiveAuthority {
            origin: AuthorityOrigin::Conversation {
                conversation_id: run
                    .conversation_id
                    .ok_or("The owner conversation is unavailable.")?,
            },
            revision: self.revision,
            project_id: project.id,
            project_revision: project.revision,
            grant_alias: "project".to_owned(),
            grant_access: grants[0].access,
            tools: self.settings.tools.clone(),
            network: self.settings.network.clone(),
            policy: crate::agents::DirectoryPolicy::from_grants(grants, "project".to_owned()),
        })
    }
}

mod settings_file {
    use crate::execution::{ExecutionSettings, ExecutionSettingsFile};
    use serde::{Deserialize, Serialize};

    pub(super) fn serialize<S: serde::Serializer>(
        settings: &ExecutionSettings,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        settings.to_file().serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ExecutionSettings, D::Error> {
        ExecutionSettings::from_file(ExecutionSettingsFile::deserialize(deserializer)?)
            .ok_or_else(|| serde::de::Error::custom("Invalid pinned project settings"))
    }
}
