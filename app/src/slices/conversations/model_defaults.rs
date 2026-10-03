use super::page::model_picker::ModelDefaults;
use axum::{Form, extract::State, response::Response};
use hypergraft::{PatchGraft, PatchStatus};
use serde::Deserialize;

use crate::{
    error::AppResult,
    providers::{ProviderKind, ThinkingEffort},
    sessions::RequiredSession,
    state::AppState,
};

#[derive(Deserialize)]
pub(super) struct DefaultForm {
    kind: String,
    provider: String,
    model: String,
    #[serde(default)]
    thinking: String,
}

pub(super) async fn save(
    State(state): State<AppState>,
    _session: RequiredSession,
    _graft: PatchGraft,
    Form(form): Form<DefaultForm>,
) -> AppResult<Response> {
    let result = save_preference(&state, &form);
    let (status, message) = match result {
        Ok(()) => (
            PatchStatus::Ok,
            if form.kind == "model" {
                "Default model saved for new conversations."
            } else {
                "Default thinking level saved for new conversations. Models use this level when available."
            },
        ),
        Err(message) => (PatchStatus::UnprocessableEntity, message),
    };
    Ok(hypergraft::PatchSet::new()
        .with_children(
            "conversation-model-defaults",
            &ModelDefaults::new(&state.preferences, &state.vault, message),
        )?
        .respond(status)?)
}

fn save_preference(state: &AppState, form: &DefaultForm) -> Result<(), &'static str> {
    let provider = ProviderKind::parse(form.provider.trim())
        .filter(|provider| state.vault.contains(*provider))
        .ok_or("Choose a model from a connected provider.")?;
    let model = form.model.trim();
    state
        .models_dev
        .model(provider, model)
        .filter(|model| !model.deprecated)
        .ok_or("Choose an available model from a connected provider.")?;
    let result = match form.kind.as_str() {
        "model" => state
            .preferences
            .set_default_model(provider, model.to_owned()),
        "thinking" => {
            let effort = ThinkingEffort::new(form.thinking.trim().to_owned())
                .filter(|effort| state.models_dev.supports(provider, model, effort))
                .ok_or("Choose a thinking level that this model supports.")?;
            state.preferences.set_default_thinking(effort)
        }
        _ => return Err("Choose a model or thinking default."),
    };
    result.map_err(|_| "Frinkworks cannot save the default. Try again.")
}

#[cfg(test)]
mod tests;
