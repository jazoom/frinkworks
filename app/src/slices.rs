use axum::Router;

use crate::state::AppState;

mod agents;
pub(crate) mod attention;
pub(crate) mod chat;
mod connect;
pub(crate) mod conversations;
mod environments;
mod execution_settings;
mod human_gates;
mod presets;
mod prompts;
mod settings;
mod skills;
mod workflow_runs;
mod workflows;

#[cfg(test)]
mod tests;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .merge(connect::router())
        .merge(attention::router())
        .merge(conversations::router())
        .merge(agents::router())
        .merge(chat::router())
        .merge(settings::router())
        .merge(skills::router())
        .merge(prompts::router())
        .merge(presets::router())
        .merge(workflow_runs::router())
        .merge(human_gates::router())
        .merge(workflows::router())
        .merge(environments::router())
}

pub(crate) fn live_router() -> hypergraft::live::LiveRouter<AppState> {
    hypergraft::live::LiveRouter::new()
        .merge(conversations::live_router())
        .expect("live projection paths are unique")
}
