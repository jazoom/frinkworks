mod page;

#[cfg(test)]
mod tests;

use axum::{
    Form, Router,
    extract::{State, rejection::FormRejection},
    response::Response,
    routing::post,
};
use hypergraft::PatchGraft;

use crate::{error::AppResult, state::AppState};

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/thinking-visibility", post(update_thinking_visibility))
}

#[derive(Default, serde::Deserialize)]
struct ThinkingVisibilityForm {
    #[serde(default)]
    show_thinking: bool,
}

async fn update_thinking_visibility(
    State(state): State<AppState>,
    _session: crate::sessions::RequiredSession,
    _graft: PatchGraft,
    form: Result<Form<ThinkingVisibilityForm>, FormRejection>,
) -> AppResult<Response> {
    let Ok(Form(form)) = form else {
        return thinking_visibility_patch(
            hypergraft::PatchStatus::UnprocessableEntity,
            state.preferences.show_thinking(),
            Some("Choose whether to show thinking."),
        );
    };
    if let Err(error) = state.preferences.set_show_thinking(form.show_thinking) {
        crate::error::trace_operation_failure("store thinking visibility preference", &error);
        return thinking_visibility_patch(
            hypergraft::PatchStatus::UnprocessableEntity,
            state.preferences.show_thinking(),
            Some("Power Plant could not save this preference. Try again."),
        );
    }
    thinking_visibility_patch(hypergraft::PatchStatus::Ok, form.show_thinking, None)
}

fn thinking_visibility_patch(
    status: hypergraft::PatchStatus,
    show_thinking: bool,
    error: Option<&'static str>,
) -> AppResult<Response> {
    Ok(hypergraft::outcome::children_patch(
        status,
        "thinking-visibility",
        &page::ThinkingVisibilityControl {
            show_thinking,
            thinking_visibility_error: error,
        },
    )?)
}
