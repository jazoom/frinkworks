use super::{ConversationDetailView, EnvironmentCatalogue};
use crate::execution::ExecutionSettings;

pub(super) struct PresetSettingView {
    pub(super) key: &'static str,
    pub(super) label: &'static str,
    pub(super) value: String,
    identity: String,
}

pub(super) struct PresetChangeView {
    pub(super) key: &'static str,
    pub(super) label: &'static str,
    pub(super) current: String,
    pub(super) requested: String,
    pub(super) requested_identity: String,
    pub(super) changed: bool,
}

pub(super) struct PresetPreviewView {
    pub(super) token: String,
    pub(super) id: String,
    pub(super) name: String,
    pub(super) rows: Vec<PresetChangeView>,
}

pub(super) fn setup_rows(
    settings: Option<&ExecutionSettings>,
    environments: &EnvironmentCatalogue,
) -> Vec<PresetSettingView> {
    let values = settings.map(|settings| {
        [
            settings.model.provider.label().to_owned(),
            settings.model.model.clone(),
            settings
                .model
                .thinking
                .as_ref()
                .map(|effort| effort.as_str().to_owned())
                .unwrap_or_else(|| "Default / not available".to_owned()),
            settings
                .tools
                .iter()
                .map(|tool| tool.label())
                .collect::<Vec<_>>()
                .join(", "),
            if settings.location == crate::execution::ToolLocation::Host {
                "This computer"
            } else {
                "Sandbox"
            }
            .to_owned(),
            crate::slices::execution_settings::page::host_approval_label(settings.host_approval)
                .to_owned(),
            environments
                .get(&settings.environment)
                .map(|record| record.name)
                .unwrap_or_else(|| {
                    format!(
                        "Unavailable environment · {}",
                        settings.environment.as_hex()
                    )
                }),
            match &settings.network {
                crate::agents::NetworkAccess::None => "Off".to_owned(),
                crate::agents::NetworkAccess::Public => "Public internet".to_owned(),
                crate::agents::NetworkAccess::Restricted(domains) => {
                    format!("Restricted: {}", domains.join(", "))
                }
            },
            settings
                .directories
                .iter()
                .enumerate()
                .map(|(index, grant)| {
                    format!(
                        "{}{}\n{} requested{}",
                        if index == 0 { "Start: " } else { "" },
                        grant.host_path.display(),
                        crate::slices::execution_settings::page::directory_access_label(
                            grant.access
                        ),
                        if grant.is_available() {
                            ""
                        } else {
                            " · Unavailable"
                        },
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
            settings.instructions.clone(),
        ]
    });
    [
        ("provider", "Provider"),
        ("model", "Model"),
        ("thinking", "Thinking effort"),
        ("tools", "Tools"),
        ("location", "Location"),
        ("host_approval", "Host command policy"),
        ("environment", "Sandbox environment"),
        ("network", "Sandbox network"),
        ("directories", "Requested directories"),
        ("instructions", "Conversation instructions"),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (key, label))| {
        let value = values
            .as_ref()
            .map(|values| values[index].clone())
            .unwrap_or_default();
        PresetSettingView {
            key,
            label,
            identity: if key == "environment" {
                settings
                    .map(|settings| settings.environment.as_hex())
                    .unwrap_or_default()
            } else {
                value.clone()
            },
            value,
        }
    })
    .collect()
}

impl ConversationDetailView {
    pub(in super::super) fn with_preset_preview(
        mut self,
        state: &crate::state::AppState,
        preview: crate::presets::PresetPreview,
    ) -> Self {
        let rows = self
            .preset_setup
            .iter()
            .zip(setup_rows(
                Some(&preview.record.settings),
                &state.environments,
            ))
            .map(|(current, requested)| PresetChangeView {
                key: requested.key,
                label: requested.label,
                changed: current.identity != requested.identity,
                current: current.value.clone(),
                requested: requested.value,
                requested_identity: requested.identity,
            })
            .collect();
        self.preset_preview = Some(PresetPreviewView {
            token: preview.token,
            id: preview.record.id.as_hex(),
            name: preview.record.name,
            rows,
        });
        self.settings_open = true;
        self
    }
}
