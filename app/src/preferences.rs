use std::{fs, io, path::PathBuf, sync::Mutex};

use serde::{Deserialize, Serialize};

use crate::{
    providers::{
        MAXIMUM_FAVOURITES, ModelSelection, ProviderKind, ThinkingEffort, model_is_bounded,
    },
    vault::ProviderVault,
};

const FILE_VERSION: u32 = 1;
const MAXIMUM_FILE_BYTES: usize = 512 * 1024;

#[cfg(test)]
mod tests;

/// Last-resort automatic compaction. The percentage compares input occupancy
/// with the published context capacity. Output reservation does not affect it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CompactionPreference {
    pub(crate) enabled: bool,
    pub(crate) threshold: u8,
}

impl Default for CompactionPreference {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 95,
        }
    }
}

impl CompactionPreference {
    pub(crate) fn new(enabled: bool, threshold: u8) -> Option<Self> {
        (1..=100)
            .contains(&threshold)
            .then_some(Self { enabled, threshold })
    }
}

fn default_automatic_compaction() -> bool {
    CompactionPreference::default().enabled
}

fn default_compaction_threshold() -> u8 {
    CompactionPreference::default().threshold
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Theme {
    #[default]
    System,
    Springfield,
    EvergreenTerrace,
    Leftorium,
    Stonecutters,
    Sector7G,
}

impl Theme {
    pub(crate) const ALL: &[Self] = &[
        Self::System,
        Self::Springfield,
        Self::EvergreenTerrace,
        Self::Leftorium,
        Self::Stonecutters,
        Self::Sector7G,
    ];

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Springfield => "springfield",
            Self::EvergreenTerrace => "evergreen-terrace",
            Self::Leftorium => "leftorium",
            Self::Stonecutters => "stonecutters",
            Self::Sector7G => "sector-7-g",
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::System => "System preference",
            Self::Springfield => "Springfield",
            Self::EvergreenTerrace => "Evergreen Terrace",
            Self::Leftorium => "Leftorium",
            Self::Stonecutters => "Stonecutters",
            Self::Sector7G => "Sector 7-G",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|theme| theme.as_str() == value)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PreferencesFile {
    version: u32,
    theme: String,
    show_thinking: bool,
    #[serde(default = "default_automatic_compaction")]
    automatic_compaction: bool,
    #[serde(default = "default_compaction_threshold")]
    compaction_threshold: u8,
    #[serde(deserialize_with = "crate::storage::required_option")]
    selected_provider: Option<ProviderKind>,
    models: Vec<ProviderPreference>,
    conversation_defaults: Option<crate::execution::ExecutionSettingsFile>,
}

#[derive(Clone, Default)]
struct PreferenceValues {
    theme: Theme,
    show_thinking: bool,
    compaction: CompactionPreference,
    selected_provider: Option<ProviderKind>,
    models: Vec<ProviderPreference>,
    conversation_defaults: Option<crate::execution::ExecutionSettings>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProviderPreference {
    selection: ModelSelection,
    favourites: Vec<String>,
}

impl PreferenceValues {
    fn provider(&mut self, kind: ProviderKind) -> &mut ProviderPreference {
        let index = self
            .models
            .iter()
            .position(|entry| entry.selection.provider == kind)
            .unwrap_or_else(|| {
                self.models.push(ProviderPreference {
                    selection: ModelSelection {
                        provider: kind,
                        model: kind.default_model().to_owned(),
                        thinking: None,
                    },
                    favourites: Vec::new(),
                });
                self.models.len() - 1
            });
        &mut self.models[index]
    }

    fn is_valid(&self) -> bool {
        (1..=100).contains(&self.compaction.threshold)
            && self.models.len() <= ProviderKind::ALL.len()
            && self.selected_provider.is_none_or(|kind| {
                self.models
                    .iter()
                    .any(|entry| entry.selection.provider == kind)
            })
            && self.models.iter().enumerate().all(|(index, entry)| {
                let selection = &entry.selection;
                ModelSelection::new(
                    selection.provider,
                    selection.model.clone(),
                    selection.thinking.clone(),
                )
                .as_ref()
                    == Some(selection)
                    && !self.models[..index]
                        .iter()
                        .any(|previous| previous.selection.provider == selection.provider)
                    && entry.favourites.len() <= MAXIMUM_FAVOURITES
                    && entry.favourites.iter().enumerate().all(|(index, model)| {
                        model_is_canonical(model) && !entry.favourites[..index].contains(model)
                    })
            })
    }
}

pub(crate) struct DeskProvider {
    pub(crate) kind: ProviderKind,
    pub(crate) model: String,
    pub(crate) thinking: Option<ThinkingEffort>,
    pub(crate) selected: bool,
    pub(crate) favourites: Vec<String>,
}

#[derive(Debug)]
pub(crate) enum FavouriteError {
    Full,
    Persist(PreferenceError),
}

#[derive(Debug)]
pub(crate) struct PreferenceError;

impl std::fmt::Display for PreferenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("preference persist failed")
    }
}

impl std::error::Error for PreferenceError {}

pub(crate) struct Preferences {
    path: Option<PathBuf>,
    values: Mutex<PreferenceValues>,
}

impl Preferences {
    pub(crate) fn open(path: PathBuf) -> Self {
        Self {
            values: Mutex::new(load(&path)),
            path: Some(path),
        }
    }

    #[cfg(test)]
    pub(crate) fn in_memory() -> Self {
        Self {
            path: None,
            values: Mutex::new(PreferenceValues::default()),
        }
    }

    pub(crate) fn theme(&self) -> Theme {
        self.values().theme
    }

    pub(crate) fn set_theme(&self, theme: Theme) -> Result<(), PreferenceError> {
        self.update(|values| values.theme = theme)
    }

    pub(crate) fn show_thinking(&self) -> bool {
        self.values().show_thinking
    }

    pub(crate) fn set_show_thinking(&self, show: bool) -> Result<(), PreferenceError> {
        self.update(|values| values.show_thinking = show)
    }

    pub(crate) fn compaction(&self) -> CompactionPreference {
        self.values().compaction
    }

    pub(crate) fn set_compaction(
        &self,
        preference: CompactionPreference,
    ) -> Result<(), PreferenceError> {
        if CompactionPreference::new(preference.enabled, preference.threshold).is_none() {
            return Err(PreferenceError);
        }
        self.update(|values| values.compaction = preference)
    }

    pub(crate) fn desk_providers(&self, vault: &ProviderVault) -> Vec<DeskProvider> {
        let providers = vault.providers();
        let values = self.values();
        let selected = values
            .selected_provider
            .filter(|selected| providers.iter().any(|(kind, _)| kind == selected))
            .or_else(|| providers.first().map(|(kind, _)| *kind));
        providers
            .into_iter()
            .map(|(kind, _)| {
                let stored = values
                    .models
                    .iter()
                    .find(|entry| entry.selection.provider == kind);
                DeskProvider {
                    kind,
                    model: stored.map_or_else(
                        || kind.default_model().to_owned(),
                        |entry| entry.selection.model.clone(),
                    ),
                    thinking: stored.and_then(|entry| entry.selection.thinking.clone()),
                    selected: selected == Some(kind),
                    favourites: stored.map_or_else(Vec::new, |entry| entry.favourites.clone()),
                }
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn select_settings(
        &self,
        kind: ProviderKind,
        model: String,
        thinking: Option<ThinkingEffort>,
    ) -> Result<(), PreferenceError> {
        let selection = ModelSelection::new(kind, model, thinking).ok_or(PreferenceError)?;
        self.update(|values| {
            values.provider(kind).selection = selection;
            values.selected_provider = Some(kind);
        })
    }

    pub(crate) fn conversation_defaults(&self) -> Option<crate::execution::ExecutionSettings> {
        self.values().conversation_defaults
    }

    pub(crate) fn set_conversation_defaults(
        &self,
        settings: crate::execution::ExecutionSettings,
    ) -> Result<(), PreferenceError> {
        let settings = crate::execution::ExecutionSettings::from_file(settings.to_file())
            .ok_or(PreferenceError)?;
        self.update(|values| {
            values.provider(settings.model.provider).selection = settings.model.clone();
            values.selected_provider = Some(settings.model.provider);
            values.conversation_defaults = Some(settings);
        })
    }

    pub(crate) fn clear_conversation_defaults(&self) -> Result<(), PreferenceError> {
        self.update(|values| values.conversation_defaults = None)
    }

    pub(crate) fn forget_provider(&self, kind: ProviderKind) -> Result<(), PreferenceError> {
        self.update(|values| {
            values
                .models
                .retain(|entry| entry.selection.provider != kind);
            if values.selected_provider == Some(kind) {
                values.selected_provider = None;
            }
        })
    }

    pub(crate) fn toggle_favourite(
        &self,
        kind: ProviderKind,
        model: &str,
    ) -> Result<bool, FavouriteError> {
        if !model_is_canonical(model) {
            return Err(FavouriteError::Persist(PreferenceError));
        }
        self.update(|values| {
            let favourites = &mut values.provider(kind).favourites;
            if let Some(index) = favourites.iter().position(|item| item == model) {
                favourites.remove(index);
                Ok(false)
            } else if favourites.len() == MAXIMUM_FAVOURITES {
                Err(FavouriteError::Full)
            } else {
                favourites.push(model.to_owned());
                Ok(true)
            }
        })
        .map_err(FavouriteError::Persist)?
    }

    fn values(&self) -> PreferenceValues {
        self.values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn update<R>(
        &self,
        change: impl FnOnce(&mut PreferenceValues) -> R,
    ) -> Result<R, PreferenceError> {
        let mut current = self
            .values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut next = current.clone();
        let result = change(&mut next);
        if !next.is_valid() {
            return Err(PreferenceError);
        }
        if let Some(path) = self.path.as_deref() {
            persist(path, &next)?;
        }
        *current = next;
        Ok(result)
    }
}

fn load(path: &std::path::Path) -> PreferenceValues {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return PreferenceValues::default();
        }
        Err(_) => return PreferenceValues::default(),
        Ok(_) => {}
    }
    let Ok(bytes) = crate::storage::read_private_bounded(path, MAXIMUM_FILE_BYTES) else {
        return PreferenceValues::default();
    };
    let Ok(file) = serde_json::from_slice::<PreferencesFile>(&bytes) else {
        return PreferenceValues::default();
    };
    if file.version != FILE_VERSION {
        return PreferenceValues::default();
    }
    let Some(theme) = Theme::parse(&file.theme) else {
        return PreferenceValues::default();
    };
    let values = PreferenceValues {
        theme,
        show_thinking: file.show_thinking,
        compaction: CompactionPreference {
            enabled: file.automatic_compaction,
            threshold: file.compaction_threshold,
        },
        selected_provider: file.selected_provider,
        models: file.models,
        conversation_defaults: file
            .conversation_defaults
            .and_then(crate::execution::ExecutionSettings::from_file),
    };
    if values.is_valid() {
        values
    } else {
        PreferenceValues::default()
    }
}

fn model_is_canonical(model: &str) -> bool {
    !model.is_empty() && model.trim() == model && model_is_bounded(model)
}

fn persist(path: &std::path::Path, values: &PreferenceValues) -> Result<(), PreferenceError> {
    let file = PreferencesFile {
        version: FILE_VERSION,
        theme: values.theme.as_str().to_owned(),
        show_thinking: values.show_thinking,
        automatic_compaction: values.compaction.enabled,
        compaction_threshold: values.compaction.threshold,
        selected_provider: values.selected_provider,
        models: values.models.clone(),
        conversation_defaults: values
            .conversation_defaults
            .as_ref()
            .map(crate::execution::ExecutionSettings::to_file),
    };
    let bytes = serde_json::to_vec_pretty(&file).map_err(|_| PreferenceError)?;
    let dir = path.parent().ok_or(PreferenceError)?;
    crate::storage::ensure_private_dir(dir).map_err(|_| PreferenceError)?;
    crate::storage::write_private(path, &bytes).map_err(|_| PreferenceError)
}
