use super::{ModelsDevCatalogue, ProviderOption, ProviderVault};

#[cfg(test)]
mod tests;

#[derive(Clone, serde::Serialize)]
pub(crate) struct EffortOption {
    pub(crate) value: String,
    pub(crate) label: String,
}

#[derive(Clone, serde::Serialize)]
pub(crate) struct ModelOption {
    pub(crate) id: String,
    favourite: bool,
    deprecated: bool,
    /// Whether the catalogue records image-input support for this model.
    image_input: bool,
    efforts: Vec<EffortOption>,
    default_effort: String,
}

pub(crate) struct ModelPicker {
    pub(crate) providers: Vec<ProviderOption>,
    pub(crate) unavailable_provider: Option<crate::providers::ProviderKind>,
    pub(crate) catalogue: String,
    pub(crate) efforts: Vec<EffortOption>,
    pub(crate) model: String,
    pub(crate) model_unavailable: bool,
    pub(crate) model_deprecated: bool,
    pub(crate) notice: String,
    pub(crate) provider_empty: bool,
    /// Known image-input support for the selected model. `None` means the
    /// catalogue has no entry, so the capability is unknown and must not be
    /// presented as supported.
    pub(crate) selected_image_input: Option<bool>,
    pub(crate) thinking: String,
}

impl ModelPicker {
    pub(crate) fn new(
        vault: &ProviderVault,
        preferences: &crate::preferences::Preferences,
        catalogue: &ModelsDevCatalogue,
        provider: &str,
        model: &str,
        thinking: &str,
    ) -> Self {
        let connections = preferences.desk_providers(vault);
        let catalogue_models: std::collections::BTreeMap<_, Vec<_>> = connections
            .iter()
            .map(|connection| {
                // Catalogue patches also retain metadata for retired selections in open tabs.
                let models = catalogue
                    .known_models(connection.kind)
                    .into_iter()
                    .map(|model| ModelOption {
                        favourite: connection.favourites.contains(&model.id),
                        deprecated: model.deprecated,
                        image_input: model.image_input,
                        efforts: catalogue
                            .efforts(connection.kind, &model.id)
                            .into_iter()
                            .map(|effort| EffortOption {
                                value: effort.as_str().to_owned(),
                                label: effort.label().to_owned(),
                            })
                            .collect(),
                        default_effort: catalogue
                            .effective_effort(
                                connection.kind,
                                &model.id,
                                connection.thinking.as_ref(),
                            )
                            .map(|effort| effort.as_str().to_owned())
                            .unwrap_or_default(),
                        id: model.id,
                    })
                    .collect();
                (connection.kind.as_str(), models)
            })
            .collect();
        let models = catalogue_models.get(provider).cloned().unwrap_or_default();
        let mut efforts = models
            .iter()
            .find(|option| option.id == model)
            .map(|option| option.efforts.clone())
            .unwrap_or_default();
        if thinking.is_empty() && !efforts.is_empty() {
            efforts.insert(
                0,
                EffortOption {
                    value: String::new(),
                    label: "Default".to_owned(),
                },
            );
        }
        if !thinking.is_empty() && !efforts.iter().any(|effort| effort.value == thinking) {
            efforts.push(EffortOption {
                value: thinking.to_owned(),
                label: format!("Unavailable · {thinking}"),
            });
        }
        let model_unavailable =
            !model.is_empty() && !models.iter().any(|option| option.id == model);
        let model_deprecated = models
            .iter()
            .any(|option| option.id == model && option.deprecated);
        let provider_empty = connections
            .iter()
            .any(|connection| connection.kind.as_str() == provider)
            && !models.iter().any(|option| !option.deprecated);
        let selected_image_input = crate::providers::ProviderKind::parse(provider)
            .and_then(|kind| catalogue.model(kind, model))
            .map(|metadata| metadata.image_input);
        Self {
            unavailable_provider: crate::providers::ProviderKind::parse(provider).filter(|kind| {
                !connections
                    .iter()
                    .any(|connection| connection.kind == *kind)
            }),
            catalogue: serde_json::to_string(&catalogue_models)
                .expect("catalogue options contain only strings"),
            providers: connections
                .into_iter()
                .map(|connection| ProviderOption {
                    value: connection.kind.as_str(),
                    label: connection.kind.label(),
                    selected: connection.kind.as_str() == provider,
                })
                .collect(),
            efforts,
            model: model.to_owned(),
            model_unavailable,
            model_deprecated,
            notice: String::new(),
            provider_empty,
            selected_image_input,
            thinking: thinking.to_owned(),
        }
    }
}
