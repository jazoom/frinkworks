use askama::Template;

use crate::preferences::Theme;

pub(super) const TITLE: &str = "Settings | Power Plant";
pub(super) const RESET_STATUS_TITLE: &str = "Reset local data | Power Plant";
pub(super) const CONFIRMATION_ABSENT: &str =
    "Select the confirmation checkbox to reset local data.";
pub(super) const CONFIRMATION_DUPLICATED: &str = "That form includes a duplicate field.";
pub(super) const CONFIRMATION_MALFORMED: &str = "That form is not valid.";
pub(super) const WORKFLOW_BUSY: &str = "A workflow is still running. Wait until it finishes.";
pub(super) const RECORD_FAILED: &str = "Power Plant could not record the reset. Try again.";

#[derive(Template)]
#[template(path = "settings/templates/index.html")]
pub(super) struct SettingsPage {
    theme: &'static str,
    themes: &'static [Theme],
    error: Option<&'static str>,
    catalogue_status: Option<&'static str>,
    catalogue_error: Option<&'static str>,
    reset_error: Option<&'static str>,
}

impl SettingsPage {
    pub(super) fn new(theme: Theme) -> Self {
        Self {
            theme: theme.as_str(),
            themes: Theme::ALL,
            error: None,
            catalogue_status: None,
            catalogue_error: None,
            reset_error: None,
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
