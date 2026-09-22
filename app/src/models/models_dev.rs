mod background;
mod catalogue;

pub(crate) use background::{TITLE_INPUT_TOKENS, TITLE_OUTPUT_TOKENS};

use std::{
    fs,
    path::PathBuf,
    sync::RwLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::providers::{ProviderKind, ThinkingEffort};
use catalogue::{
    MAXIMUM_CACHE_BYTES, MAXIMUM_SOURCE_BYTES, SOURCE_URL, Snapshot, USER_AGENT, bounded_body,
    content_type_is, filter_source, model_count, parse_snapshot,
};

pub use catalogue::run_catalogue_utility;
const REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const BUNDLED: &[u8] = include_bytes!("../../catalogue/models-dev-v1.json");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RefreshResult {
    Updated,
    Unchanged,
    Failed,
    Skipped,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RefreshPolicy {
    IfDue,
    OnStart,
    Force,
}

pub(crate) struct ModelsDevCatalogue {
    path: Option<PathBuf>,
    active: RwLock<Snapshot>,
    refresh: tokio::sync::Mutex<()>,
    invalidations: tokio::sync::broadcast::Sender<()>,
    client: reqwest::Client,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelMetadata {
    pub(crate) id: String,
    pub(crate) attachment: bool,
    pub(crate) image_input: bool,
    pub(crate) context_limit: u64,
    /// Published maximum output tokens. Zero means the catalogue is silent.
    pub(crate) output_limit: u64,
}

/// Millionths of a US dollar per million tokens. Missing categories stay unknown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ModelPrices {
    pub(crate) input: Option<u64>,
    pub(crate) output: Option<u64>,
    pub(crate) cache_read: Option<u64>,
    pub(crate) cache_write: Option<u64>,
}

impl ModelsDevCatalogue {
    pub(crate) fn open(path: PathBuf) -> Result<Self, String> {
        let bundled = parse_snapshot(BUNDLED)
            .map_err(|_| "The bundled model catalogue is invalid.".to_owned())?;
        tracing::info!(
            source = "bundled",
            model_count = model_count(&bundled),
            "model capability catalogue loaded"
        );
        let active = match crate::storage::read_private_bounded(&path, MAXIMUM_CACHE_BYTES) {
            Ok(bytes) => match parse_snapshot(&bytes) {
                Ok(snapshot) if local_is_newer(&bundled, &snapshot) => {
                    tracing::info!(
                        source = "local",
                        model_count = model_count(&snapshot),
                        "model capability catalogue loaded"
                    );
                    snapshot
                }
                Ok(_) => bundled,
                Err(()) => {
                    tracing::warn!(
                        source = "local",
                        "corrupt model capability catalogue ignored"
                    );
                    if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
                        let _ = crate::storage::remove_private(&path);
                    }
                    bundled
                }
            },
            Err(_) => {
                if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
                    tracing::warn!(
                        source = "local",
                        "unreadable model capability catalogue ignored"
                    );
                    let _ = crate::storage::remove_private(&path);
                }
                bundled
            }
        };
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|_| "The model catalogue client is unavailable.".to_owned())?;
        let (invalidations, _) = tokio::sync::broadcast::channel(16);
        Ok(Self {
            path: Some(path),
            active: RwLock::new(active),
            refresh: tokio::sync::Mutex::new(()),
            invalidations,
            client,
        })
    }

    #[cfg(test)]
    pub(crate) fn bundled() -> Self {
        let active = parse_snapshot(BUNDLED).expect("bundled catalogue");
        let (invalidations, _) = tokio::sync::broadcast::channel(16);
        Self {
            path: None,
            active: RwLock::new(active),
            refresh: tokio::sync::Mutex::new(()),
            invalidations,
            client: reqwest::Client::new(),
        }
    }

    pub(crate) fn models(&self, kind: ProviderKind) -> Vec<ModelMetadata> {
        let active = self.read();
        active
            .providers
            .iter()
            .find(|provider| provider.id == kind.as_str())
            .map(|provider| {
                provider
                    .models
                    .iter()
                    .filter(|model| model.supports_tools)
                    .map(model_metadata)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn model(&self, kind: ProviderKind, id: &str) -> Option<ModelMetadata> {
        let active = self.read();
        active
            .providers
            .iter()
            .find(|provider| provider.id == kind.as_str())
            .and_then(|provider| {
                provider
                    .models
                    .iter()
                    .find(|model| model.id == id && model.supports_tools)
            })
            .map(model_metadata)
    }

    pub(crate) fn title_model(
        &self,
        kind: ProviderKind,
    ) -> Option<crate::providers::ModelSelection> {
        let active = self.read();
        let provider = active
            .providers
            .iter()
            .find(|provider| provider.id == kind.as_str())?;
        let model = background::select(&provider.models, time::OffsetDateTime::now_utc().date())?;
        Some(crate::providers::ModelSelection {
            provider: kind,
            model: model.id.clone(),
            thinking: ["none", "minimal", "low"]
                .iter()
                .find(|effort| model.efforts.iter().any(|value| value == **effort))
                .and_then(|effort| ThinkingEffort::new((*effort).to_owned())),
        })
    }

    pub(crate) fn context_limit(&self, kind: ProviderKind, id: &str) -> Option<u64> {
        self.model(kind, id)
            .map(|model| model.context_limit)
            .filter(|limit| *limit > 0)
    }

    /// Published maximum output tokens for a model. Unknown capacity stays
    /// `None` so callers do not turn it into a percentage or a budget.
    pub(crate) fn output_limit(&self, kind: ProviderKind, id: &str) -> Option<u64> {
        self.model(kind, id)
            .map(|model| model.output_limit)
            .filter(|limit| *limit > 0)
    }

    /// Whether the model accepts image input. `None` means the catalogue has
    /// no entry for the model, so the capability is unknown and must not be
    /// treated as supported.
    pub(crate) fn supports_images(&self, kind: ProviderKind, id: &str) -> Option<bool> {
        self.model(kind, id).map(|model| model.image_input)
    }

    /// Prices are independent of title-model eligibility and tool support.
    pub(crate) fn prices(&self, kind: ProviderKind, id: &str) -> Option<ModelPrices> {
        let active = self.read();
        let model = active
            .providers
            .iter()
            .find(|provider| provider.id == kind.as_str())
            .and_then(|provider| provider.models.iter().find(|model| model.id == id))?;
        let prices = model.prices?;
        Some(ModelPrices {
            input: prices.input,
            output: prices.output,
            cache_read: prices.cache_read,
            cache_write: prices.cache_write,
        })
    }

    pub(crate) fn efforts(&self, kind: ProviderKind, model: &str) -> Vec<ThinkingEffort> {
        let active = self.read();
        active
            .providers
            .iter()
            .find(|provider| provider.id == kind.as_str())
            .and_then(|provider| provider.models.iter().find(|entry| entry.id == model))
            .map(|entry| {
                entry
                    .efforts
                    .iter()
                    .filter_map(|value| ThinkingEffort::new(value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn effective_effort(
        &self,
        kind: ProviderKind,
        model: &str,
        saved: Option<&ThinkingEffort>,
    ) -> Option<ThinkingEffort> {
        let efforts = self.efforts(kind, model);
        if let Some(saved) = saved
            && efforts.contains(saved)
        {
            return Some(saved.clone());
        }
        for preferred in ["medium", "high"] {
            if let Some(value) = efforts.iter().find(|value| value.as_str() == preferred) {
                return Some(value.clone());
            }
        }
        efforts.into_iter().next()
    }

    pub(crate) fn supports(
        &self,
        kind: ProviderKind,
        model: &str,
        effort: &ThinkingEffort,
    ) -> bool {
        self.efforts(kind, model).contains(effort)
    }

    pub(crate) async fn refresh_if_due(&self) -> bool {
        self.refresh(RefreshPolicy::IfDue).await == RefreshResult::Updated
    }

    async fn refresh_on_start(&self) {
        self.refresh(RefreshPolicy::OnStart).await;
    }

    pub(crate) async fn refresh_now(&self) -> RefreshResult {
        self.refresh(RefreshPolicy::Force).await
    }

    async fn refresh(&self, policy: RefreshPolicy) -> RefreshResult {
        let _guard = self.refresh.lock().await;
        let now = unix_seconds();
        let previous = self.read().clone();
        if policy == RefreshPolicy::IfDue
            && timestamp_is_recent(previous.last_attempt_at_unix_seconds, now)
        {
            tracing::debug!(source = "local", "model capability refresh skipped");
            return RefreshResult::Skipped;
        }
        tracing::info!(source = "network", "model capability refresh started");
        let mut request = self
            .client
            .get(SOURCE_URL)
            .header(reqwest::header::ACCEPT, "application/json");
        if policy != RefreshPolicy::Force && !previous.source.etag.is_empty() {
            request = request.header(reqwest::header::IF_NONE_MATCH, &previous.source.etag);
        }
        let mut attempted = previous.clone();
        attempted.last_attempt_at_unix_seconds = now;
        let response = match request.send().await {
            Ok(response) => response,
            Err(_) => {
                self.persist_attempt(attempted);
                tracing::warn!(source = "network", "model capability transport failed");
                return RefreshResult::Failed;
            }
        };
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            attempted.checked_at_unix_seconds = now;
            self.replace(attempted, false);
            tracing::info!(source = "network", "model capability catalogue unchanged");
            return RefreshResult::Unchanged;
        }
        if response.status() != reqwest::StatusCode::OK
            || !content_type_is(&response, "application/json")
        {
            self.persist_attempt(attempted);
            tracing::warn!(
                status = response.status().as_u16(),
                "model capability HTTP response rejected"
            );
            return RefreshResult::Failed;
        }
        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let Some(body) = bounded_body(response, MAXIMUM_SOURCE_BYTES).await else {
            self.persist_attempt(attempted);
            tracing::warn!("model capability response size rejected");
            return RefreshResult::Failed;
        };
        let Ok(mut next) = filter_source(&body, &etag, now) else {
            self.persist_attempt(attempted);
            tracing::warn!("model capability schema rejected");
            return RefreshResult::Failed;
        };
        next.last_attempt_at_unix_seconds = now;
        let changed = capabilities_differ(&previous, &next);
        self.replace(next, changed);
        if changed {
            RefreshResult::Updated
        } else {
            RefreshResult::Unchanged
        }
    }

    fn persist_attempt(&self, snapshot: Snapshot) {
        self.replace(snapshot, false);
    }
    fn replace(&self, snapshot: Snapshot, changed: bool) {
        if let Some(path) = self.path.as_deref()
            && let Ok(bytes) = serde_json::to_vec_pretty(&snapshot)
            && bytes.len() <= MAXIMUM_CACHE_BYTES
            && crate::storage::write_private(path, &bytes).is_err()
        {
            tracing::warn!("model capability cache persistence failed");
        }
        *self.write() = snapshot;
        if changed {
            tracing::info!("model capability catalogue changed");
            let _ = self.invalidations.send(());
        }
    }
    fn read(&self) -> std::sync::RwLockReadGuard<'_, Snapshot> {
        self.active
            .read()
            .unwrap_or_else(|error| error.into_inner())
    }
    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Snapshot> {
        self.active
            .write()
            .unwrap_or_else(|error| error.into_inner())
    }
}

pub(crate) async fn refresh_worker(state: crate::state::AppState) {
    state.models_dev.refresh_on_start().await;
    loop {
        tokio::time::sleep(Duration::from_secs(60 * 60)).await;
        state.models_dev.refresh_if_due().await;
    }
}

fn model_metadata(model: &catalogue::Model) -> ModelMetadata {
    ModelMetadata {
        id: model.id.clone(),
        attachment: model.attachment,
        image_input: model.image_input,
        context_limit: model.limit.context,
        output_limit: model.limit.output,
    }
}

fn capabilities_differ(left: &Snapshot, right: &Snapshot) -> bool {
    serde_json::to_value(&left.providers).ok() != serde_json::to_value(&right.providers).ok()
}

fn local_is_newer(bundled: &Snapshot, local: &Snapshot) -> bool {
    local.checked_at_unix_seconds > bundled.checked_at_unix_seconds
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn timestamp_is_recent(timestamp: u64, now: u64) -> bool {
    timestamp <= now && now - timestamp < REFRESH_INTERVAL.as_secs()
}

#[cfg(test)]
mod tests;
