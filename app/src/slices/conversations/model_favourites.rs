#[cfg(test)]
mod tests;

use askama::Template;
use axum::{Form, extract::State, response::Response};
use hypergraft::{PatchGraft, PatchStatus};
use serde::Deserialize;

use crate::{
    error::AppResult, preferences::FavouriteError, providers::ProviderKind,
    sessions::RequiredSession, state::AppState,
};

#[derive(Deserialize)]
pub(super) struct FavouriteForm {
    provider: String,
    model: String,
}

#[derive(Template)]
#[template(path = "conversations/templates/model_catalogue.html")]
struct FavouriteContents {
    catalogue: String,
    error: &'static str,
}

pub(super) async fn toggle(
    State(state): State<AppState>,
    _session: RequiredSession,
    _graft: PatchGraft,
    Form(form): Form<FavouriteForm>,
) -> AppResult<Response> {
    let provider = ProviderKind::parse(form.provider.trim());
    let model = form.model.trim();
    let error = match provider.filter(|kind| {
        state.vault.contains(*kind) && state.models_dev.model(*kind, model).is_some()
    }) {
        None => "Choose a model from a connected provider.",
        Some(provider) => match state.preferences.toggle_favourite(provider, model) {
            Ok(_) => "",
            Err(FavouriteError::Full) => "The favourites list is full. Remove a favourite first.",
            Err(FavouriteError::Persist(_)) => {
                "Frinkworks could not save the favourite. Try again."
            }
        },
    };
    let picker = super::page::model_picker::ModelPicker::new(
        &state.vault,
        &state.preferences,
        &state.models_dev,
        "",
        "",
        "",
    );
    Ok(hypergraft::PatchSet::new()
        .with_children(
            "conversation-model-catalogue",
            &FavouriteContents {
                catalogue: picker.catalogue,
                error,
            },
        )?
        .respond(if error.is_empty() {
            PatchStatus::Ok
        } else {
            PatchStatus::UnprocessableEntity
        })?)
}
