use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{Next, from_fn, from_fn_with_state},
    response::Response,
};
use tower_http::{
    classify::ServerErrorsFailureClass,
    compression::{
        CompressionLayer,
        predicate::{DefaultPredicate, NotForContentType, Predicate},
    },
    services::ServeDir,
    trace::TraceLayer,
};
use tracing_subscriber::prelude::*;

use crate::{
    assets::AssetPaths,
    config::{RuntimeEnvironment, StartupConfig},
    models, security, sessions, slices,
    state::{self, AppState},
};

#[cfg(feature = "dev")]
mod development;

pub(crate) async fn run(log_level: tracing::Level) -> Result<(), Box<dyn std::error::Error>> {
    let tracing_filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(log_level.into())
        .from_env_lossy();
    let application_events = tracing_subscriber::filter::filter_fn(|metadata| {
        is_application_trace_target(metadata.target())
    });
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_filter(tracing_filter)
                .with_filter(application_events),
        )
        .try_init()
        .ok();
    let config = StartupConfig::from_environment()
        .map_err(|error| format!("configuration error: {error}"))?;
    let bind_address = config.bind_address.clone();
    let static_dir = config.static_dir.clone();
    let assets = AssetPaths::load(&static_dir)
        .map_err(|error| format!("asset configuration error: {error}"))?;
    let (config, local_data) =
        crate::local_data::prepare(config).map_err(|error| format!("startup error: {error}"))?;
    let state = state::build(config, assets, local_data)
        .await
        .map_err(|error| format!("startup error: {error}"))?;
    tokio::spawn(sessions::purge_expired_sessions(state.clone()));
    tokio::spawn(models::models_dev::refresh_worker(state.clone()));
    let environment = state.config.environment();
    let public_origin = state.config.public_origin().to_owned();
    let app = build_router(state, static_dir.clone());
    #[cfg(feature = "dev")]
    let app = development::with_live_reload(app, static_dir);
    let listener = tokio::net::TcpListener::bind(&bind_address).await?;
    tracing::info!(?environment);
    tracing::info!(
        "Power Plant is available at:\n********************************\n{public_origin}\n********************************"
    );
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

fn is_application_trace_target(target: &str) -> bool {
    target == "powerplant" || target.starts_with("powerplant::")
}

fn request_route<B>(request: &axum::http::Request<B>) -> &str {
    request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(axum::extract::MatchedPath::as_str)
        .unwrap_or_else(|| match request.uri().path() {
            path @ ("/static" | "/static/") => path,
            path if path.starts_with("/static/") => path,
            _ => "unmatched",
        })
}

fn build_router(state: AppState, static_dir: std::path::PathBuf) -> Router {
    let live_endpoint =
        hypergraft::live::LiveEndpoint::with_default_path(state.config.public_origin())
            .expect("the public origin is a canonical HTTP origin");
    let live_guard = sessions::LiveSessionGuard::new(state.sessions.clone(), state.vault.clone());
    let live = hypergraft::live::service(
        live_endpoint,
        hypergraft::live::LiveSocketConfig::default(),
        slices::live_router(),
        live_guard,
    );
    let browser = slices::router()
        .merge(live)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            sessions::resolve_session,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            security::enforce_origin,
        ))
        .layer(from_fn(hypergraft::middleware::classify));

    Router::new()
        .merge(browser)
        .nest("/static", static_files(state.clone(), static_dir))
        .fallback(|| async { (StatusCode::NOT_FOUND, "Not found") })
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &axum::http::Request<_>| {
                    tracing::debug_span!(
                        "http.request",
                        method = %request.method(),
                        route = request_route(request)
                    )
                })
                .on_response(
                    |response: &axum::response::Response,
                     latency: std::time::Duration,
                     _span: &tracing::Span| {
                        tracing::debug!(
                            status = response.status().as_u16(),
                            latency_ms = latency.as_millis(),
                            "request completed"
                        );
                    },
                )
                .on_failure(
                    |failure: ServerErrorsFailureClass,
                     latency: std::time::Duration,
                     _span: &tracing::Span| match failure {
                        ServerErrorsFailureClass::StatusCode(status) => tracing::error!(
                            status = status.as_u16(),
                            latency_ms = latency.as_millis(),
                            "request completed with server error"
                        ),
                        ServerErrorsFailureClass::Error(_) => tracing::error!(
                            latency_ms = latency.as_millis(),
                            "request failed before a response was completed"
                        ),
                    },
                ),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            security::add_security_headers,
        ))
        .with_state(state)
}

fn static_files(state: AppState, static_dir: std::path::PathBuf) -> Router<AppState> {
    let compress = CompressionLayer::new().compress_when(
        DefaultPredicate::new()
            .and(NotForContentType::const_new("font/"))
            .and(NotForContentType::const_new("application/font-")),
    );
    Router::new()
        .fallback_service(ServeDir::new(static_dir))
        .layer(compress)
        .layer(from_fn_with_state(
            state,
            cache_fingerprinted_production_assets,
        ))
}

async fn cache_fingerprinted_production_assets(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_owned();
    let mut response = next.run(request).await;
    if !response.status().is_success() {
        return response;
    }
    if state.config.environment() == RuntimeEnvironment::Production
        && is_fingerprinted_asset_path(&path)
    {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    } else if state.config.environment() == RuntimeEnvironment::Development
        && is_development_bundle_path(&path)
    {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

fn is_development_bundle_path(path: &str) -> bool {
    matches!(
        path,
        "/static/assets/main.js"
            | "/static/assets/main.css"
            | "/assets/main.js"
            | "/assets/main.css"
    )
}

fn is_fingerprinted_asset_path(path: &str) -> bool {
    let relative = path
        .strip_prefix("/static/assets/")
        .or_else(|| path.strip_prefix("/assets/"))
        .filter(|rest| !rest.is_empty() && !rest.contains(".."));
    let Some(relative) = relative else {
        return false;
    };
    let name = relative.rsplit('/').next().unwrap_or(relative);
    let Some((stem, _ext)) = name.rsplit_once('.') else {
        return false;
    };
    stem.rsplit_once('-').is_some_and(|(_, hash)| {
        hash.len() >= 8 && hash.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}
