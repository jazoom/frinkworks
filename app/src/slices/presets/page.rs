use super::PresetForm;
use crate::slices::execution_settings::page::{ToolOption, tool_options};
use crate::{presets::PresetProvenance, providers::ProviderKind, state::AppState};
use askama::Template;
use serde::Serialize;

pub(super) struct OptionView {
    value: String,
    label: String,
    selected: bool,
}

pub(super) struct RecordView {
    id: String,
    name: String,
    revision: u32,
    source: String,
}

pub(super) struct ModelChoice {
    id: String,
    selected: bool,
}

pub(super) struct EffortChoice {
    value: String,
    label: String,
}

#[derive(Serialize)]
struct PresetCatalogueModel {
    id: String,
    default_effort: String,
    efforts: Vec<PresetCatalogueEffort>,
}

#[derive(Serialize)]
struct PresetCatalogueEffort {
    value: String,
    label: String,
}

#[derive(Template)]
#[template(path = "presets/templates/index.html")]
pub(super) struct PresetsPage {
    form: PresetForm,
    records: Vec<RecordView>,
    providers: Vec<OptionView>,
    environments: Vec<OptionView>,
    directories: Vec<String>,
    model_status: &'static str,
    models: Vec<ModelChoice>,
    model_unavailable: bool,
    efforts: Vec<EffortChoice>,
    catalogue: String,
    instructions: String,
    tool_options: Vec<ToolOption>,
    job_active: bool,
    show_form: bool,
    error: &'static str,
    context: Option<(String, String)>,
    selected_preset: String,
    destinations: Vec<(String, String)>,
}

impl PresetsPage {
    pub(super) fn new(
        state: &AppState,
        mut form: PresetForm,
        error: &'static str,
        show_form: bool,
    ) -> Self {
        if form.network.is_empty() {
            form.network = "none".to_owned();
        }
        let providers = ProviderKind::ALL
            .into_iter()
            .map(|kind| OptionView {
                value: kind.as_str().to_owned(),
                label: format!(
                    "{}{}",
                    kind.label(),
                    if state.vault.contains(kind) {
                        ""
                    } else {
                        " — Unavailable"
                    }
                ),
                selected: form.provider == kind.as_str(),
            })
            .collect();
        let environments = crate::slices::execution_settings::page::environment_options(
            &state.environments,
            &state.environment_snapshots,
            crate::environments::EnvironmentId::parse(&form.environment),
        )
        .into_iter()
        .map(|option| OptionView {
            value: option.id,
            label: format!(
                "{} — {} · {}",
                option.name, option.readiness, option.availability
            ),
            selected: option.selected,
        })
        .collect();
        let directories = crate::presets::PresetId::parse(&form.preset_id)
            .and_then(|id| state.presets.get(&id))
            .map(|r| {
                r.settings
                    .directories
                    .iter()
                    .map(|g| {
                        format!(
                            "{} · {} · {}",
                            g.host_path.display(),
                            g.guest_path(),
                            if g.is_available() {
                                "Requested access only"
                            } else {
                                "Unavailable"
                            }
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let kind = ProviderKind::parse(&form.provider);
        let model_status = if form.model.is_empty() {
            ""
        } else if kind
            .and_then(|kind| state.models_dev.model(kind, &form.model))
            .is_some()
        {
            "Listed model"
        } else {
            "Model unavailable in the catalogue; requested value retained"
        };
        // The creation form stays constrained to the catalogue while retained
        // unavailable values survive revision-bound edits without substitution.
        // The list renders without the form, so it needs no catalogue payload.
        let catalogue = if show_form {
            let catalogue_models: std::collections::BTreeMap<_, Vec<PresetCatalogueModel>> =
                ProviderKind::ALL
                    .into_iter()
                    .map(|provider| {
                        let models = state
                            .models_dev
                            .models(provider)
                            .into_iter()
                            .map(|item| {
                                let efforts = state
                                    .models_dev
                                    .efforts(provider, &item.id)
                                    .into_iter()
                                    .map(|effort| PresetCatalogueEffort {
                                        value: effort.as_str().to_owned(),
                                        label: effort.label(),
                                    })
                                    .collect();
                                PresetCatalogueModel {
                                    default_effort: state
                                        .models_dev
                                        .effective_effort(provider, &item.id, None)
                                        .map(|effort| effort.as_str().to_owned())
                                        .unwrap_or_default(),
                                    id: item.id,
                                    efforts,
                                }
                            })
                            .collect();
                        (provider.as_str(), models)
                    })
                    .collect();
            serde_json::to_string(&catalogue_models)
                .expect("preset catalogue options contain only strings")
        } else {
            "{}".to_owned()
        };
        let models = kind
            .map(|kind| state.models_dev.models(kind))
            .unwrap_or_default()
            .into_iter()
            .map(|item| ModelChoice {
                selected: item.id == form.model,
                id: item.id,
            })
            .collect::<Vec<_>>();
        let model_unavailable =
            !form.model.is_empty() && !models.iter().any(|option| option.id == form.model);
        let mut efforts = kind
            .map(|kind| state.models_dev.efforts(kind, &form.model))
            .unwrap_or_default()
            .into_iter()
            .map(|effort| EffortChoice {
                value: effort.as_str().to_owned(),
                label: effort.label(),
            })
            .collect::<Vec<_>>();
        if !form.thinking.is_empty() && !efforts.iter().any(|effort| effort.value == form.thinking)
        {
            efforts.push(EffortChoice {
                value: form.thinking.clone(),
                label: format!("Unavailable · {}", form.thinking),
            });
        }
        Self {
            model_status,
            models,
            model_unavailable,
            efforts,
            catalogue,
            instructions: form.instructions.clone(),
            tool_options: tool_options(&form.tools()),
            form,
            records: state
                .presets
                .list()
                .into_iter()
                .map(|r| RecordView {
                    id: r.id.as_hex(),
                    name: r.name,
                    revision: r.revision,
                    source: match r.provenance {
                        PresetProvenance::Conversation { title, .. } => {
                            format!("From conversation: {title}")
                        }
                        PresetProvenance::Draft => "Independent settings".to_owned(),
                    },
                })
                .collect(),
            providers,
            environments,
            directories,
            job_active: false,
            show_form,
            error,
            context: None,
            selected_preset: String::new(),
            destinations: Vec::new(),
        }
    }

    pub(super) fn with_destination(
        mut self,
        state: &AppState,
        conversation: &str,
        preset: &str,
    ) -> Self {
        self.context = crate::conversations::ConversationId::parse(conversation)
            .and_then(|id| state.conversations.get(&id))
            .map(|record| (record.id.as_hex(), record.title));
        if preset.is_empty() {
            return self;
        }
        let Some(preset) =
            crate::presets::PresetId::parse(preset).and_then(|id| state.presets.get(&id))
        else {
            self.error = "That preset is no longer available. Choose another.";
            return self;
        };
        self.selected_preset = preset.name;
        if self.context.is_none() {
            let mut conversations = state.conversations.list();
            conversations.sort_by_key(|record| std::cmp::Reverse(record.updated_at_ms));
            self.destinations = conversations
                .into_iter()
                .map(|record| {
                    (
                        record.title,
                        format!(
                            "/presets?conversation={}&preset={}",
                            record.id.as_hex(),
                            preset.id.as_hex()
                        ),
                    )
                })
                .collect();
        }
        self
    }

    fn model_form(&self) -> &'static str {
        "preset-editor"
    }
}
