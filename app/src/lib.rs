#![forbid(unsafe_code)]

mod agents;
mod assets;
mod config;
mod conversations;
mod environments;
mod error;
mod execution;
mod hex;
mod local_data;
mod markdown;
mod models;
mod plan_login;
mod preferences;
mod presets;
mod providers;
mod responses;
mod sandbox;
mod security;
mod server;
mod sessions;
mod slices;
mod state;
mod storage;
mod template_filters;
#[cfg(test)]
mod tests;
mod tools;
mod vault;
mod workflows;

#[doc(hidden)]
pub use models::models_dev::run_catalogue_utility;

pub async fn run_server(log_level: tracing::Level) -> Result<(), Box<dyn std::error::Error>> {
    server::run(log_level).await
}
