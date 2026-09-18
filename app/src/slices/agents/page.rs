use askama::Template;
use serde::Serialize;

use crate::agents::{AgentRecord, MAXIMUM_GRANTS, ToolId, guest_path_for};
use crate::sandbox::OrphanSandbox;

use super::forms::AgentFormState;

pub(super) const CATALOGUE_TITLE: &str = "Agents | Power Plant";
pub(super) const NEW_TITLE: &str = "New agent | Power Plant";
pub(super) const CONFIG_TITLE: &str = "Configure agent | Power Plant";

pub(super) struct AgentListItem {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) projects: Vec<AgentProjectMetadata>,
    pub(super) has_directory_ceiling: bool,
}

pub(super) struct AgentProjectMetadata {
    pub(super) name: String,
    pub(super) path: String,
}

pub(super) struct GrantRow {
    pub(super) index: usize,
    pub(super) alias: String,
    pub(super) path: String,
    pub(super) access: String,
    pub(super) guest_path: String,
    pub(super) path_locked: bool,
    pub(super) can_remove: bool,
}

pub(super) struct AgentProviderOption {
    pub(super) value: &'static str,
    pub(super) label: &'static str,
    pub(super) selected: bool,
}

pub(super) struct AgentModelOption {
    pub(super) id: String,
    pub(super) selected: bool,
}

pub(super) struct AgentEffortOption {
    pub(super) value: String,
    pub(super) label: String,
}

pub(super) struct ToolRow {
    pub(super) name: &'static str,
    pub(super) label: &'static str,
    pub(super) checked: bool,
}

#[derive(Template)]
#[template(path = "agents/templates/catalogue.html")]
pub(super) struct CatalogueView {
    pub(super) agents: Vec<AgentListItem>,
    pub(super) orphans: Vec<OrphanSandbox>,
    pub(super) error: &'static str,
}

impl CatalogueView {
    pub(super) fn from_parts(
        agents: &[AgentRecord],
        orphans: Vec<OrphanSandbox>,
        error: &'static str,
    ) -> Self {
        Self {
            agents: agents
                .iter()
                .map(|agent| AgentListItem {
                    id: agent.id.as_hex(),
                    name: agent.name.clone(),
                    has_directory_ceiling: !agent.directories.is_empty(),
                    projects: agent
                        .directories
                        .iter()
                        .map(|grant| AgentProjectMetadata {
                            name: grant.alias.clone(),
                            path: grant.host_path.to_string_lossy().into_owned(),
                        })
                        .collect(),
                })
                .collect(),
            orphans,
            error,
        }
    }
}

#[derive(Template)]
#[template(path = "agents/templates/form.html")]
pub(super) struct AgentFormView {
    pub(super) title: String,
    pub(super) lead: String,
    pub(super) section: &'static str,
    pub(super) back_href: String,
    pub(super) back_label: &'static str,
    pub(super) action: String,
    pub(super) submit: &'static str,
    pub(super) name: String,
    pub(super) instructions: String,
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) thinking: String,
    pub(super) providers: Vec<AgentProviderOption>,
    pub(super) unavailable_provider: Option<crate::providers::ProviderKind>,
    pub(super) models: Vec<AgentModelOption>,
    pub(super) model_unavailable: bool,
    pub(super) efforts: Vec<AgentEffortOption>,
    pub(super) catalogue: String,
    pub(super) primary: String,
    pub(super) network: String,
    pub(super) network_domains: String,
    pub(super) tools: Vec<ToolRow>,
    pub(super) grants: Vec<GrantRow>,
    pub(super) can_add: bool,
    pub(super) error: &'static str,
    pub(super) agent_id: String,
    pub(super) revision: String,
    pub(super) show_delete: bool,
}

#[derive(Template)]
#[template(path = "agents/templates/form.html", block = "agent_form")]
pub(super) struct AgentFormContents<'a> {
    pub(super) back_href: &'a str,
    pub(super) back_label: &'static str,
    pub(super) action: &'a str,
    pub(super) submit: &'static str,
    pub(super) name: &'a str,
    pub(super) instructions: &'a str,
    pub(super) provider: &'a str,
    pub(super) model: &'a str,
    pub(super) thinking: &'a str,
    pub(super) providers: &'a [AgentProviderOption],
    pub(super) unavailable_provider: Option<crate::providers::ProviderKind>,
    pub(super) models: &'a [AgentModelOption],
    pub(super) model_unavailable: bool,
    pub(super) efforts: &'a [AgentEffortOption],
    pub(super) catalogue: &'a str,
    pub(super) primary: &'a str,
    pub(super) network: &'a str,
    pub(super) network_domains: &'a str,
    pub(super) tools: &'a [ToolRow],
    pub(super) grants: &'a [GrantRow],
    pub(super) can_add: bool,
    pub(super) error: &'static str,
    pub(super) agent_id: &'a str,
    pub(super) revision: &'a str,
    pub(super) show_delete: bool,
}

impl AgentFormView {
    pub(super) fn create(
        app: &crate::state::AppState,
        state: AgentFormState,
        error: &'static str,
    ) -> Self {
        Self::from_state(
            app,
            "New agent",
            "/agents",
            "Create agent",
            state,
            error,
            "",
            false,
        )
    }

    pub(super) fn edit(
        app: &crate::state::AppState,
        record: &AgentRecord,
        state: AgentFormState,
        error: &'static str,
    ) -> Self {
        let mut view = Self::from_state(
            app,
            "Configure agent",
            &format!("/agents/{}/configuration", record.id.as_hex()),
            "Save agent",
            state,
            error,
            &record.id.as_hex(),
            true,
        );
        view.back_href.clear();
        view.back_label = "";
        view
    }

    #[allow(clippy::too_many_arguments)]
    fn from_state(
        app: &crate::state::AppState,
        title: &'static str,
        action: &str,
        submit: &'static str,
        state: AgentFormState,
        error: &'static str,
        agent_id: &str,
        show_delete: bool,
    ) -> Self {
        let grant_count = state.directories.len();
        let picker = agent_model_picker(app, &state.provider, &state.model, &state.thinking);
        let primary_trim = state.primary.trim().to_owned();
        Self {
            title: title.to_owned(),
            lead: "Define how this agent works and which local projects they can access."
                .to_owned(),
            section: "agents",
            back_href: "/agents".to_owned(),
            back_label: "Back to agents",
            action: action.to_owned(),
            submit,
            name: state.name,
            instructions: state.instructions,
            provider: state.provider,
            model: state.model,
            thinking: state.thinking,
            providers: picker.providers,
            unavailable_provider: picker.unavailable_provider,
            models: picker.models,
            model_unavailable: picker.model_unavailable,
            efforts: picker.efforts,
            catalogue: picker.catalogue,
            primary: state.primary,
            network: state.network,
            network_domains: state.network_domains,
            tools: ToolId::ALL
                .into_iter()
                .map(|tool| ToolRow {
                    name: tool.as_str(),
                    label: tool.label(),
                    checked: state.tools.contains(&tool),
                })
                .collect(),
            grants: state
                .directories
                .into_iter()
                .enumerate()
                .map(|(index, grant)| {
                    let guest_path = grant_guest_path(&grant.alias, &primary_trim);
                    GrantRow {
                        index,
                        alias: grant.alias,
                        path: grant.path,
                        access: grant.access,
                        guest_path,
                        path_locked: false,
                        can_remove: true,
                    }
                })
                .collect(),
            can_add: grant_count < MAXIMUM_GRANTS,
            error,
            agent_id: agent_id.to_owned(),
            revision: state.revision,
            show_delete,
        }
    }

    pub(super) fn contents(&self) -> AgentFormContents<'_> {
        AgentFormContents {
            back_href: &self.back_href,
            back_label: self.back_label,
            action: &self.action,
            submit: self.submit,
            name: &self.name,
            instructions: &self.instructions,
            provider: &self.provider,
            model: &self.model,
            thinking: &self.thinking,
            providers: &self.providers,
            unavailable_provider: self.unavailable_provider,
            models: &self.models,
            model_unavailable: self.model_unavailable,
            efforts: &self.efforts,
            catalogue: &self.catalogue,
            primary: &self.primary,
            network: &self.network,
            network_domains: &self.network_domains,
            tools: &self.tools,
            grants: &self.grants,
            can_add: self.can_add,
            error: self.error,
            agent_id: &self.agent_id,
            revision: &self.revision,
            show_delete: self.show_delete,
        }
    }
}

struct AgentPicker {
    providers: Vec<AgentProviderOption>,
    unavailable_provider: Option<crate::providers::ProviderKind>,
    models: Vec<AgentModelOption>,
    model_unavailable: bool,
    efforts: Vec<AgentEffortOption>,
    catalogue: String,
}

#[derive(Serialize)]
struct AgentCatalogueModel {
    id: String,
    default_effort: String,
    efforts: Vec<AgentCatalogueEffort>,
}

#[derive(Serialize)]
struct AgentCatalogueEffort {
    value: String,
    label: String,
}

fn grant_guest_path(alias: &str, primary: &str) -> String {
    let alias = alias.trim();
    if alias.is_empty() {
        return String::new();
    }
    guest_path_for(alias, primary.trim())
}

fn agent_model_picker(
    app: &crate::state::AppState,
    provider: &str,
    model: &str,
    thinking: &str,
) -> AgentPicker {
    let connections = app.preferences.desk_providers(&app.vault);
    let mut catalogue: std::collections::BTreeMap<&str, Vec<AgentCatalogueModel>> =
        std::collections::BTreeMap::new();
    for connection in &connections {
        let models = app
            .models_dev
            .models(connection.kind)
            .into_iter()
            .map(|item| {
                let efforts = app
                    .models_dev
                    .efforts(connection.kind, &item.id)
                    .into_iter()
                    .map(|effort| AgentCatalogueEffort {
                        value: effort.as_str().to_owned(),
                        label: effort.label(),
                    })
                    .collect::<Vec<_>>();
                let default_effort = app
                    .models_dev
                    .effective_effort(connection.kind, &item.id, connection.thinking.as_ref())
                    .map(|effort| effort.as_str().to_owned())
                    .unwrap_or_default();
                AgentCatalogueModel {
                    id: item.id,
                    default_effort,
                    efforts,
                }
            })
            .collect();
        catalogue.insert(connection.kind.as_str(), models);
    }
    let selected_kind = crate::providers::ProviderKind::parse(provider.trim());
    let connected = selected_kind
        .is_some_and(|kind| connections.iter().any(|connection| connection.kind == kind));
    let models = selected_kind
        .filter(|_| connected)
        .map(|kind| app.models_dev.models(kind))
        .unwrap_or_default();
    let model_options = models
        .into_iter()
        .map(|item| AgentModelOption {
            selected: item.id == model,
            id: item.id,
        })
        .collect::<Vec<_>>();
    let model_unavailable =
        !model.is_empty() && !model_options.iter().any(|option| option.id == model);
    let mut efforts = selected_kind
        .filter(|_| connected)
        .map(|kind| {
            app.models_dev
                .efforts(kind, model)
                .into_iter()
                .map(|effort| AgentEffortOption {
                    value: effort.as_str().to_owned(),
                    label: effort.label(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !thinking.is_empty() && !efforts.iter().any(|effort| effort.value == thinking) {
        efforts.push(AgentEffortOption {
            value: thinking.to_owned(),
            label: format!("Unavailable · {thinking}"),
        });
    }
    AgentPicker {
        providers: connections
            .into_iter()
            .map(|connection| AgentProviderOption {
                value: connection.kind.as_str(),
                label: connection.kind.label(),
                selected: connection.kind.as_str() == provider.trim(),
            })
            .collect(),
        unavailable_provider: selected_kind.filter(|_| !connected),
        models: model_options,
        model_unavailable,
        efforts,
        catalogue: serde_json::to_string(&catalogue)
            .expect("agent catalogue options contain only strings"),
    }
}
