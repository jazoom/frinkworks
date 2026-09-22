use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

use futures_util::StreamExt;
use quick_xml::{Reader, events::Event};
use serde::{Deserialize, Serialize};

use crate::providers::{MAXIMUM_MODEL_BYTES, ProviderKind, ThinkingEffort};

pub(super) const SOURCE_URL: &str = "https://models.dev/api.json";
pub(super) const MAXIMUM_SOURCE_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAXIMUM_CACHE_BYTES: usize = 4 * 1024 * 1024;
const MAXIMUM_PROVIDER_MODELS: usize = 4_096;
const MAXIMUM_MODELS: usize = 8_192;
const MAXIMUM_ETAG_BYTES: usize = 256;
const MAXIMUM_EFFORTS: usize = 16;
const MAXIMUM_SVG_BYTES: usize = 256 * 1024;
const SOURCE_ORIGIN: &str = "models.dev";
pub(super) const USER_AGENT: &str = "PowerPlant-model-catalogue/1";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Source {
    pub(super) url: String,
    pub(super) etag: String,
    pub(super) sha256: String,
    pub(super) origin: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ModelLimit {
    pub(super) context: u64,
    /// Published maximum output tokens. Zero means the source did not publish it.
    #[serde(default)]
    pub(super) output: u64,
}

/// Millionths of a US dollar per million tokens. Absent fields stay unknown.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ModelPrices {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) input: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) output: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) cache_read: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) cache_write: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Model {
    pub(super) id: String,
    pub(super) reasoning: bool,
    #[serde(default)]
    pub(super) deprecated: bool,
    pub(super) efforts: Vec<String>,
    pub(super) attachment: bool,
    #[serde(default)]
    pub(super) image_input: bool,
    pub(super) supports_tools: bool,
    pub(super) limit: ModelLimit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) background: Option<super::background::BackgroundMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) prices: Option<ModelPrices>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Provider {
    pub(super) id: String,
    pub(super) models_dev_id: String,
    pub(super) models: Vec<Model>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    pub(super) version: u32,
    pub(super) source: Source,
    pub(super) checked_at_unix_seconds: u64,
    pub(super) last_attempt_at_unix_seconds: u64,
    #[serde(default)]
    pub(super) refresh_failed: bool,
    pub(super) providers: Vec<Provider>,
}

#[derive(Deserialize)]
struct SourceProvider {
    models: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct SourceModel {
    id: Option<String>,
    attachment: bool,
    #[serde(default)]
    reasoning: bool,
    #[serde(default)]
    reasoning_options: Vec<ReasoningOption>,
    tool_call: bool,
    modalities: SourceModalities,
    #[serde(default)]
    status: Option<String>,
    limit: SourceLimit,
    #[serde(default)]
    release_date: String,
    #[serde(default)]
    cost: Option<SourceCost>,
    #[serde(default)]
    experimental: serde_json::Value,
}

#[derive(Deserialize)]
struct SourceCost {
    input: Option<f64>,
    output: Option<f64>,
    reasoning: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

#[derive(Deserialize)]
struct SourceLimit {
    context: u64,
    #[serde(default)]
    output: u64,
}

#[derive(Deserialize)]
struct SourceModalities {
    input: Vec<String>,
    output: Vec<String>,
}

#[derive(Deserialize)]
struct ReasoningOption {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    values: serde_json::Value,
}

pub(super) fn filter_source(bytes: &[u8], etag: &str, now: u64) -> Result<Snapshot, String> {
    if bytes.len() > MAXIMUM_SOURCE_BYTES {
        return Err("The source exceeds the size limit.".to_owned());
    }
    if !valid_etag(etag) {
        return Err("The source ETag is invalid.".to_owned());
    }
    let mut source: HashMap<String, serde_json::Value> = serde_json::from_slice(bytes)
        .map_err(|_| "The source must be a JSON object.".to_owned())?;
    let mut providers = Vec::new();
    let mut total = 0;
    for kind in ProviderKind::ALL {
        let source_id = models_dev_id(kind);
        let value = source
            .remove(source_id)
            .ok_or_else(|| format!("{source_id}: The provider is absent."))?;
        let provider: SourceProvider = serde_json::from_value(value)
            .map_err(|_| format!("{source_id}: The models must be a JSON object."))?;
        if provider.models.len() > MAXIMUM_PROVIDER_MODELS {
            return Err(format!(
                "{source_id}: The provider exceeds the model limit."
            ));
        }
        let mut models = Vec::new();
        for (key, value) in provider.models {
            if !bounded(&key, MAXIMUM_MODEL_BYTES) {
                return Err(format!("{source_id}: A model identifier is invalid."));
            }
            let invalid = |reason| format!("{source_id}/{key}: {reason}");
            let model: SourceModel = serde_json::from_value(value)
                .map_err(|_| invalid("A model field is absent or has an invalid type."))?;
            let id = model.id.as_deref().unwrap_or(&key);
            if id != key {
                return Err(invalid("The model identifier does not match its key."));
            }
            if !valid_modalities(&model.modalities) {
                return Err(invalid("The model modalities are invalid."));
            }
            if !matches!(
                model.status.as_deref(),
                None | Some("alpha" | "beta" | "deprecated")
            ) {
                return Err(invalid("The model status is unknown."));
            }
            let background = background_metadata(id, &model);
            if (!model.tool_call && background.is_none())
                || !model.modalities.input.iter().any(|value| value == "text")
                || model.modalities.output.as_slice() != ["text"]
            {
                continue;
            }
            let mut efforts = Vec::new();
            for option in &model.reasoning_options {
                if option.kind != "effort" {
                    continue;
                }
                let values = option
                    .values
                    .as_array()
                    .ok_or_else(|| invalid("The effort values must be an array."))?;
                for value in values {
                    if value.is_null() {
                        continue;
                    }
                    let value = value
                        .as_str()
                        .ok_or_else(|| invalid("Each effort must be text or null."))?;
                    if value == "default" {
                        continue;
                    }
                    if ThinkingEffort::new(value.to_owned()).is_none() {
                        return Err(invalid("An effort exceeds the text bounds."));
                    }
                    if !efforts.iter().any(|effort| effort == value) {
                        efforts.push(value.to_owned());
                    }
                    if efforts.len() > MAXIMUM_EFFORTS {
                        return Err(invalid("The model exceeds the effort limit."));
                    }
                }
            }
            models.push(Model {
                id: id.to_owned(),
                reasoning: model.reasoning,
                deprecated: model.status.as_deref() == Some("deprecated"),
                efforts,
                attachment: model.attachment,
                image_input: model
                    .modalities
                    .input
                    .iter()
                    .any(|modality| modality == "image"),
                supports_tools: model.tool_call,
                limit: ModelLimit {
                    context: model.limit.context,
                    output: model.limit.output,
                },
                background,
                prices: model_prices(model.cost.as_ref()),
            });
        }
        models.sort_by(|left, right| left.id.cmp(&right.id));
        total += models.len();
        providers.push(Provider {
            id: kind.as_str().to_owned(),
            models_dev_id: source_id.to_owned(),
            models,
        });
    }
    if total > MAXIMUM_MODELS {
        return Err("The catalogue exceeds the model limit.".to_owned());
    }
    providers.sort_by(|left, right| left.id.cmp(&right.id));
    let sha256 = {
        use sha2::Digest;
        crate::hex::encode(&sha2::Sha256::digest(bytes))
    };
    Ok(Snapshot {
        version: 1,
        source: Source {
            url: SOURCE_URL.to_owned(),
            etag: etag.to_owned(),
            sha256,
            origin: SOURCE_ORIGIN.to_owned(),
        },
        checked_at_unix_seconds: now,
        last_attempt_at_unix_seconds: now,
        refresh_failed: false,
        providers,
    })
}

pub(super) fn parse_snapshot(bytes: &[u8]) -> Result<Snapshot, ()> {
    if bytes.len() > MAXIMUM_CACHE_BYTES {
        return Err(());
    }
    let snapshot: Snapshot = serde_json::from_slice(bytes).map_err(|_| ())?;
    validate_snapshot(&snapshot)?;
    Ok(snapshot)
}

const PRICE_SCALE: f64 = 1_000_000.0;

/// Convert a models.dev USD-per-million-tokens price to millionths of a dollar.
pub(super) fn token_price(value: f64) -> Option<u64> {
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let scaled = value * PRICE_SCALE;
    if scaled >= u64::MAX as f64 {
        return None;
    }
    Some(scaled.round() as u64)
}

fn model_prices(cost: Option<&SourceCost>) -> Option<ModelPrices> {
    let cost = cost?;
    let prices = ModelPrices {
        input: cost.input.and_then(token_price),
        output: cost.output.and_then(token_price),
        cache_read: cost.cache_read.and_then(token_price),
        cache_write: cost.cache_write.and_then(token_price),
    };
    if prices.input.is_none()
        && prices.output.is_none()
        && prices.cache_read.is_none()
        && prices.cache_write.is_none()
    {
        None
    } else {
        Some(prices)
    }
}

fn background_metadata(
    id: &str,
    model: &SourceModel,
) -> Option<super::background::BackgroundMetadata> {
    if model.status.is_some() || model.experimental == true || super::background::unsuitable(id) {
        return None;
    }
    let cost = model.cost.as_ref()?;
    let metadata = super::background::BackgroundMetadata {
        release_date: model.release_date.clone(),
        input_cost: cost.input?,
        output_cost: cost.output?.max(cost.reasoning.unwrap_or(0.0)),
        output_limit: model.limit.output,
    };
    metadata.valid().then_some(metadata)
}

fn validate_snapshot(snapshot: &Snapshot) -> Result<(), ()> {
    if snapshot.version != 1
        || snapshot.source.url != SOURCE_URL
        || snapshot.source.origin != SOURCE_ORIGIN
        || !valid_etag(&snapshot.source.etag)
    {
        return Err(());
    }
    let mut total = 0;
    let mut provider_ids = HashSet::new();
    for provider in &snapshot.providers {
        let Some(kind) = ProviderKind::parse(&provider.id) else {
            return Err(());
        };
        if !provider_ids.insert(kind) {
            return Err(());
        }
        if provider.models_dev_id != models_dev_id(kind)
            || provider.models.len() > MAXIMUM_PROVIDER_MODELS
        {
            return Err(());
        }
        let mut model_ids = HashSet::new();
        for model in &provider.models {
            let mut efforts = HashSet::new();
            if !bounded(&model.id, MAXIMUM_MODEL_BYTES)
                || (!model.supports_tools && model.background.is_none())
                || model.background.as_ref().is_some_and(|metadata| {
                    model.deprecated
                        || !metadata.valid()
                        || super::background::unsuitable(&model.id)
                })
                || !model_ids.insert(&model.id)
                || model.efforts.len() > MAXIMUM_EFFORTS
                || model.efforts.iter().any(|effort| {
                    ThinkingEffort::new(effort.clone()).is_none() || !efforts.insert(effort)
                })
                || model.prices.as_ref().is_some_and(|prices| {
                    prices.input.is_none()
                        && prices.output.is_none()
                        && prices.cache_read.is_none()
                        && prices.cache_write.is_none()
                })
            {
                return Err(());
            }
        }
        total += provider.models.len();
    }
    if total > MAXIMUM_MODELS
        || ProviderKind::ALL
            .iter()
            .any(|kind| !provider_ids.contains(kind))
    {
        return Err(());
    }
    Ok(())
}

fn canonical_snapshot(mut snapshot: Snapshot) -> Result<Snapshot, ()> {
    validate_snapshot(&snapshot)?;
    snapshot
        .providers
        .sort_by(|left, right| left.id.cmp(&right.id));
    for provider in &mut snapshot.providers {
        provider
            .models
            .sort_by(|left, right| left.id.cmp(&right.id));
    }
    Ok(snapshot)
}

fn validate_canonical_catalogue(bytes: &[u8]) -> Result<Snapshot, &'static str> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| "The checked-in catalogue contains invalid JSON.")?;
    let snapshot = parse_snapshot(bytes).map_err(|()| "The checked-in catalogue is invalid.")?;
    let canonical =
        canonical_snapshot(snapshot).map_err(|()| "The checked-in catalogue is invalid.")?;
    let canonical_value = serde_json::to_value(&canonical)
        .map_err(|_| "The checked-in catalogue cannot be serialised.")?;
    if value != canonical_value {
        return Err("The checked-in catalogue is not canonical.");
    }
    Ok(canonical)
}

pub(super) async fn bounded_body(response: reqwest::Response, maximum: usize) -> Option<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return None;
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.ok()?;
        if bytes.len().checked_add(chunk.len())? > maximum {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    Some(bytes)
}

fn validate_svg(bytes: &[u8]) -> Result<(), &'static str> {
    if bytes.len() > MAXIMUM_SVG_BYTES {
        return Err("A provider SVG exceeds the size limit.");
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "A provider SVG is not UTF-8.")?;
    let start = text.trim_start();
    let prefix = start.get(..4).unwrap_or(start);
    if !prefix.eq_ignore_ascii_case("<svg")
        || !start
            .as_bytes()
            .get(4)
            .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'>')
    {
        return Err("A provider SVG has no SVG root.");
    }

    let mut reader = Reader::from_str(text);
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                let name = element.local_name();
                if name.as_ref().eq_ignore_ascii_case(b"script")
                    || name.as_ref().eq_ignore_ascii_case(b"foreignObject")
                {
                    return Err("A provider SVG contains unsafe content.");
                }
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|_| "A provider SVG is invalid.")?;
                    let key = attribute.key.local_name();
                    if key.as_ref().len() > 2 && key.as_ref()[..2].eq_ignore_ascii_case(b"on") {
                        return Err("A provider SVG contains an event attribute.");
                    }
                    if key.as_ref().eq_ignore_ascii_case(b"href")
                        || key.as_ref().eq_ignore_ascii_case(b"src")
                    {
                        let value = attribute
                            .decode_and_unescape_value(reader.decoder())
                            .map_err(|_| "A provider SVG is invalid.")?;
                        let value = value.trim_start();
                        if ["http:", "https:", "data:"]
                            .iter()
                            .any(|scheme| starts_ascii_case_insensitive(value, scheme))
                        {
                            return Err("A provider SVG contains an external resource.");
                        }
                    }
                }
            }
            Ok(Event::DocType(_)) => return Err("A provider SVG contains a document type."),
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Err("A provider SVG is invalid."),
        }
    }
    Ok(())
}

fn starts_ascii_case_insensitive(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|value| value.eq_ignore_ascii_case(prefix))
}

fn models_dev_id(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::OpenaiCodex => "openai",
        _ => kind.as_str(),
    }
}

fn valid_etag(value: &str) -> bool {
    value.len() <= MAXIMUM_ETAG_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() || byte == b' ')
}

fn bounded(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

fn valid_modalities(modalities: &SourceModalities) -> bool {
    [modalities.input.as_slice(), modalities.output.as_slice()]
        .into_iter()
        .all(|values| {
            !values.is_empty()
                && values.len() <= 5
                && values.iter().all(|value| {
                    matches!(value.as_str(), "text" | "audio" | "image" | "video" | "pdf")
                })
                && values
                    .iter()
                    .enumerate()
                    .all(|(index, value)| !values[..index].contains(value))
        })
}

pub(super) fn model_count(snapshot: &Snapshot) -> usize {
    snapshot
        .providers
        .iter()
        .map(|provider| provider.models.len())
        .sum()
}

pub async fn run_catalogue_utility(command: &str) -> Result<(), Box<dyn std::error::Error>> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let catalogue_path = manifest.join("catalogue/models-dev-v1.json");
    let logo_directory = manifest.join("public/images/providers");
    match command {
        "check" => check_repository(&catalogue_path, &logo_directory),
        "update" => update_repository(&catalogue_path, &logo_directory).await,
        _ => Err(format!("Unknown command: {command}").into()),
    }
}

fn check_repository(
    catalogue_path: &Path,
    logo_directory: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = fs::read(catalogue_path)?;
    let snapshot = validate_canonical_catalogue(&bytes)?;
    for kind in ProviderKind::ALL {
        validate_svg(&fs::read(
            logo_directory.join(format!("{}.svg", kind.as_str())),
        )?)?;
    }
    println!("Catalogue valid: {} models", model_count(&snapshot));
    Ok(())
}

async fn update_repository(
    catalogue_path: &Path,
    logo_directory: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(USER_AGENT)
        .build()?;
    let response = client
        .get(SOURCE_URL)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await?;
    if response.status() != reqwest::StatusCode::OK
        || !content_type_is(&response, "application/json")
    {
        return Err(format!("{SOURCE_URL} returned an invalid response").into());
    }
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .map(|value| value.to_str())
        .transpose()?
        .unwrap_or("")
        .to_owned();
    let source_bytes = bounded_body(response, MAXIMUM_SOURCE_BYTES)
        .await
        .ok_or("The models.dev response exceeds the size limit.")?;
    let snapshot = filter_source(&source_bytes, &etag, super::unix_seconds())?;
    let mut output = serde_json::to_vec_pretty(&snapshot)?;
    output.push(b'\n');
    if output.len() > MAXIMUM_CACHE_BYTES {
        return Err("The filtered catalogue exceeds the size limit.".into());
    }

    let mut logos = Vec::new();
    for kind in ProviderKind::ALL {
        let source_id = models_dev_id(kind);
        let url = format!("https://models.dev/logos/{source_id}.svg");
        let response = client
            .get(&url)
            .header(reqwest::header::ACCEPT, "image/svg+xml")
            .send()
            .await?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(format!("{url} returned {}", response.status()).into());
        }
        let bytes = bounded_body(response, MAXIMUM_SVG_BYTES)
            .await
            .ok_or("A provider SVG exceeds the size limit.")?;
        validate_svg(&bytes)?;
        logos.push((kind, bytes));
    }

    atomic_write(catalogue_path, &output)?;
    for (kind, bytes) in logos {
        atomic_write(
            &logo_directory.join(format!("{}.svg", kind.as_str())),
            &bytes,
        )?;
    }
    for provider in &snapshot.providers {
        println!("{}: {} models", provider.id, provider.models.len());
    }
    println!("Source SHA-256: {}", snapshot.source.sha256);
    eprintln!("Synthetic can use the generic models.dev fallback logo.");
    Ok(())
}

pub(super) fn content_type_is(response: &reqwest::Response, expected: &str) -> bool {
    response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(';').next() == Some(expected))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    crate::storage::write_private(path, bytes).map_err(Into::into)
}

#[cfg(test)]
mod tests;
