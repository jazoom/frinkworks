use askama::Template;

use crate::{
    agents::NetworkAccess,
    execution::{ExecutionSettings, ToolLocation},
    preferences::{CompactionPreference, Theme},
    providers::ThinkingEffort,
    state::AppState,
};

pub(super) const TITLE: &str = "Settings | Power Plant";
pub(super) const RESET_STATUS_TITLE: &str = "Reset local data | Power Plant";
pub(super) const CONFIRMATION_ABSENT: &str =
    "Select the confirmation checkbox to reset local data.";
pub(super) const CONFIRMATION_DUPLICATED: &str = "That form includes a duplicate field.";
pub(super) const CONFIRMATION_MALFORMED: &str = "That form is not valid.";
pub(super) const WORKFLOW_BUSY: &str = "A workflow is still running. Wait until it finishes.";
pub(super) const RECORD_FAILED: &str = "Power Plant could not record the reset. Try again.";
pub(super) const COMPACTION_MALFORMED: &str = "That compaction form is not valid.";
pub(super) const COMPACTION_RANGE: &str =
    "Enter a whole percentage from 1 through 100 for automatic compaction.";
pub(super) const COMPACTION_FAILED: &str = "Power Plant cannot save the compaction preference.";
pub(super) const DEFAULTS_CLEAR_FAILED: &str =
    "Power Plant cannot clear the saved conversation defaults.";

pub(super) struct DefaultsField {
    pub(super) label: &'static str,
    pub(super) value: String,
}

#[derive(Template)]
#[template(path = "settings/templates/conversation_defaults.html")]
pub(super) struct ConversationDefaultsSection {
    defaults_exists: bool,
    defaults_fields: Vec<DefaultsField>,
    defaults_error: Option<&'static str>,
}

impl ConversationDefaultsSection {
    pub(super) fn new(state: &AppState, error: Option<&'static str>) -> Self {
        let Some(settings) = state.preferences.conversation_defaults() else {
            return Self {
                defaults_exists: false,
                defaults_fields: Vec::new(),
                defaults_error: error,
            };
        };
        Self {
            defaults_exists: true,
            defaults_fields: defaults_fields(state, &settings),
            defaults_error: error,
        }
    }
}

fn defaults_fields(state: &AppState, settings: &ExecutionSettings) -> Vec<DefaultsField> {
    let mut fields = vec![
        DefaultsField {
            label: "Model",
            value: format!(
                "{} · {}",
                settings.model.provider.label(),
                settings.model.model
            ),
        },
        DefaultsField {
            label: "Thinking effort",
            value: settings
                .model
                .thinking
                .as_ref()
                .map(ThinkingEffort::as_str)
                .unwrap_or("Default")
                .to_owned(),
        },
        DefaultsField {
            label: "Instructions",
            value: if settings.instructions.trim().is_empty() {
                "None".to_owned()
            } else {
                settings.instructions.clone()
            },
        },
        DefaultsField {
            label: "Tools",
            value: if settings.tools.is_empty() {
                "No tools".to_owned()
            } else {
                settings
                    .tools
                    .iter()
                    .map(|tool| tool.label())
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        },
        DefaultsField {
            label: "Environment",
            value: state
                .environments
                .get(&settings.environment)
                .map(|record| record.name)
                .unwrap_or_else(|| "Unavailable environment".to_owned()),
        },
        DefaultsField {
            label: "Network",
            value: match &settings.network {
                NetworkAccess::None => "Off".to_owned(),
                NetworkAccess::Public => "Public internet".to_owned(),
                NetworkAccess::Restricted(domains) => {
                    format!("Restricted: {}", domains.join(", "))
                }
            },
        },
        DefaultsField {
            label: "Where tools run",
            value: match settings.location {
                ToolLocation::Sandbox => "Sandbox".to_owned(),
                ToolLocation::Host => {
                    let approval = crate::slices::execution_settings::page::host_approval_label(
                        settings.host_approval,
                    );
                    format!("This computer · {approval}")
                }
            },
        },
    ];
    // Runtime consent and host approval are not stored. Each conversation
    // requests them again, so they never appear in this snapshot.
    for grant in &settings.directories {
        fields.push(DefaultsField {
            label: "Directory",
            value: format!(
                "{} · {}",
                grant.host_path.display(),
                crate::slices::execution_settings::page::directory_access_label(grant.access)
            ),
        });
    }
    fields
}

#[derive(Template)]
#[template(path = "settings/templates/index.html")]
pub(super) struct SettingsPage {
    theme: &'static str,
    themes: &'static [Theme],
    error: Option<&'static str>,
    compaction_enabled: bool,
    compaction_threshold: u8,
    compaction_error: Option<&'static str>,
    catalogue_status: Option<&'static str>,
    catalogue_error: Option<&'static str>,
    reset_error: Option<&'static str>,
    defaults_exists: bool,
    defaults_fields: Vec<DefaultsField>,
    defaults_error: Option<&'static str>,
}

impl SettingsPage {
    pub(super) fn new(state: &AppState) -> Self {
        let defaults = ConversationDefaultsSection::new(state, None);
        Self {
            theme: state.preferences.theme().as_str(),
            themes: Theme::ALL,
            error: None,
            compaction_enabled: state.preferences.compaction().enabled,
            compaction_threshold: state.preferences.compaction().threshold,
            compaction_error: None,
            catalogue_status: None,
            catalogue_error: None,
            reset_error: None,
            defaults_exists: defaults.defaults_exists,
            defaults_fields: defaults.defaults_fields,
            defaults_error: defaults.defaults_error,
        }
    }
}

#[derive(Template)]
#[template(path = "settings/templates/theme.html")]
pub(super) struct ThemeSetting {
    theme: &'static str,
    themes: &'static [Theme],
    error: Option<&'static str>,
}

impl ThemeSetting {
    pub(super) fn new(theme: Theme, error: Option<&'static str>) -> Self {
        Self {
            theme: theme.as_str(),
            themes: Theme::ALL,
            error,
        }
    }
}

#[derive(Template)]
#[template(path = "settings/templates/compaction.html")]
pub(super) struct CompactionSetting {
    compaction_enabled: bool,
    compaction_threshold: u8,
    compaction_error: Option<&'static str>,
}

impl CompactionSetting {
    pub(super) fn new(compaction: CompactionPreference, error: Option<&'static str>) -> Self {
        Self {
            compaction_enabled: compaction.enabled,
            compaction_threshold: compaction.threshold,
            compaction_error: error,
        }
    }
}

#[derive(Template)]
#[template(path = "settings/templates/model_catalogue.html")]
pub(super) struct ModelCatalogueSetting {
    catalogue_status: Option<&'static str>,
    catalogue_error: Option<&'static str>,
}

impl ModelCatalogueSetting {
    pub(super) fn result(
        catalogue_status: Option<&'static str>,
        catalogue_error: Option<&'static str>,
    ) -> Self {
        Self {
            catalogue_status,
            catalogue_error,
        }
    }
}

#[derive(Template)]
#[template(path = "settings/templates/local_data.html")]
pub(super) struct LocalDataSection {
    reset_error: Option<&'static str>,
}

impl LocalDataSection {
    pub(super) fn new(reset_error: Option<&'static str>) -> Self {
        Self { reset_error }
    }
}

#[derive(Template)]
#[template(path = "settings/templates/reset_status.html")]
pub(super) struct ResetStatusPage;
