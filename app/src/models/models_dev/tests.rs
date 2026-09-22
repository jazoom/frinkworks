use super::*;

fn effort_values(catalogue: &ModelsDevCatalogue, model: &str) -> Vec<String> {
    catalogue
        .efforts(ProviderKind::OpenaiCodex, model)
        .into_iter()
        .map(|value| value.as_str().to_owned())
        .collect()
}

fn snapshot_is_rejected(snapshot: &Snapshot) -> bool {
    let bytes = serde_json::to_vec(snapshot).expect("snapshot");
    parse_snapshot(&bytes).is_err()
}

#[test]
fn bundled_snapshot_has_required_dynamic_efforts() {
    let catalogue = ModelsDevCatalogue::bundled();
    assert_eq!(
        effort_values(&catalogue, "gpt-5.6-sol"),
        ["none", "low", "medium", "high", "xhigh", "max"]
    );
}

#[test]
fn bundled_snapshot_records_its_source_check() {
    let snapshot = parse_snapshot(BUNDLED).expect("bundled snapshot");

    assert!(snapshot.checked_at_unix_seconds > 0);
    assert_eq!(
        snapshot.last_attempt_at_unix_seconds,
        snapshot.checked_at_unix_seconds
    );
}

#[test]
fn snapshot_selection_uses_the_latest_successful_check() {
    let mut bundled = parse_snapshot(BUNDLED).expect("bundled snapshot");
    let mut local = bundled.clone();
    bundled.checked_at_unix_seconds = 20;
    local.checked_at_unix_seconds = 21;

    assert!(local_is_newer(&bundled, &local));

    local.checked_at_unix_seconds = 19;
    local.last_attempt_at_unix_seconds = 30;
    assert!(!local_is_newer(&bundled, &local));
}

#[test]
fn snapshot_rejects_duplicate_provider_identifiers() {
    let mut snapshot = parse_snapshot(BUNDLED).expect("bundled snapshot");
    snapshot.providers.push(snapshot.providers[0].clone());

    assert!(snapshot_is_rejected(&snapshot));
}

#[test]
fn snapshot_rejects_duplicate_model_identifiers() {
    let mut snapshot = parse_snapshot(BUNDLED).expect("bundled snapshot");
    let duplicate = snapshot.providers[0].models[0].clone();
    snapshot.providers[0].models.push(duplicate);

    assert!(snapshot_is_rejected(&snapshot));
}

#[test]
fn snapshot_rejects_duplicate_effort_values() {
    let mut snapshot = parse_snapshot(BUNDLED).expect("bundled snapshot");
    let model = snapshot.providers[0]
        .models
        .iter_mut()
        .find(|model| !model.efforts.is_empty())
        .expect("model with efforts");
    model.efforts.push(model.efforts[0].clone());

    assert!(snapshot_is_rejected(&snapshot));
}

#[test]
fn snapshot_requires_each_provider() {
    let mut snapshot = parse_snapshot(BUNDLED).expect("bundled snapshot");
    snapshot
        .providers
        .retain(|provider| provider.id != ProviderKind::Xai.as_str());

    assert!(snapshot_is_rejected(&snapshot));
}

#[test]
fn future_attempt_does_not_suppress_refresh() {
    assert!(!timestamp_is_recent(101, 100));
    assert!(timestamp_is_recent(100, 100));
}

#[test]
fn output_limit_is_exposed_separately_from_context_capacity() {
    let catalogue = ModelsDevCatalogue::bundled();
    let metadata = catalogue
        .model(ProviderKind::Xai, "grok-4.6")
        .expect("model");
    assert!(metadata.output_limit > 0);
    assert_eq!(
        catalogue.output_limit(ProviderKind::Xai, "grok-4.6"),
        Some(metadata.output_limit)
    );
    assert!(
        catalogue
            .output_limit(ProviderKind::Xai, "missing-model")
            .is_none()
    );
}

#[test]
fn retired_defaults_keep_metadata_and_fallback_never_crosses_providers() {
    let catalogue = ModelsDevCatalogue::bundled();
    let kind = ProviderKind::Deepseek;
    let mut snapshot = catalogue.read().clone();
    let provider = snapshot
        .providers
        .iter_mut()
        .find(|provider| provider.id == kind.as_str())
        .unwrap();
    let mut retired = provider
        .models
        .iter()
        .find(|model| model.id == kind.default_model())
        .unwrap()
        .clone();
    retired.deprecated = true;
    retired.background = None;
    let mut replacement = retired.clone();
    replacement.id = "replacement".to_owned();
    replacement.deprecated = false;
    provider.models = vec![retired.clone(), replacement.clone()];
    catalogue.replace(snapshot.clone(), true);
    assert_eq!(catalogue.model(kind, &retired.id).unwrap().id, retired.id);
    assert!(catalogue.model(kind, &retired.id).unwrap().deprecated);
    assert_eq!(
        catalogue.preferred_model(kind, &retired.id),
        Some(replacement.id.clone())
    );
    assert_eq!(catalogue.models(kind).len(), 1);
    assert_eq!(
        catalogue.output_limit(kind, &retired.id),
        Some(retired.limit.output)
    );
    snapshot
        .providers
        .iter_mut()
        .find(|provider| provider.id == kind.as_str())
        .unwrap()
        .models
        .clear();
    catalogue.replace(snapshot, true);
    assert_eq!(catalogue.preferred_model(kind, &retired.id), None);
    assert!(!catalogue.models(ProviderKind::Xai).is_empty());
}

#[test]
fn draft_defaults_prefer_saved_then_configured_then_lexical_model() {
    let catalogue = ModelsDevCatalogue::bundled();
    let kind = ProviderKind::Deepseek;
    let mut snapshot = catalogue.read().clone();
    let provider = snapshot
        .providers
        .iter_mut()
        .find(|provider| provider.id == kind.as_str())
        .unwrap();
    let configured = provider
        .models
        .iter()
        .find(|model| model.id == kind.default_model())
        .unwrap()
        .clone();
    let mut first = configured.clone();
    first.id = "a".to_owned();
    let mut saved = configured.clone();
    saved.id = "z".to_owned();
    provider.models = vec![saved.clone(), first.clone(), configured.clone()];
    catalogue.replace(snapshot.clone(), true);
    assert_eq!(catalogue.preferred_model(kind, "z"), Some(saved.id));
    assert_eq!(
        catalogue.preferred_model(kind, "absent"),
        Some(configured.id)
    );
    snapshot
        .providers
        .iter_mut()
        .find(|provider| provider.id == kind.as_str())
        .unwrap()
        .models
        .pop();
    catalogue.replace(snapshot, true);
    assert_eq!(catalogue.preferred_model(kind, "absent"), Some(first.id));
}

#[test]
fn failed_refresh_keeps_capabilities_and_persists_its_status() {
    let catalogue = ModelsDevCatalogue::bundled();
    let original = catalogue.read().clone();
    let mut attempted = original.clone();
    attempted.last_attempt_at_unix_seconds += 1;
    catalogue.persist_attempt(attempted);
    let failed = catalogue.read().clone();
    assert!(!capabilities_differ(&original, &failed));
    assert!(local_is_newer(&original, &failed));
    let stored = parse_snapshot(&serde_json::to_vec(&failed).unwrap()).unwrap();
    assert!(stored.refresh_failed);
    assert_eq!(
        stored.checked_at_unix_seconds,
        original.checked_at_unix_seconds
    );
}

#[test]
fn prices_do_not_require_title_or_tool_eligibility() {
    let catalogue = ModelsDevCatalogue::bundled();
    assert!(
        catalogue
            .model(ProviderKind::Openrouter, "aion-labs/aion-rp-llama-3.1-8b")
            .is_none()
    );
    assert_eq!(
        catalogue.prices(ProviderKind::Openrouter, "aion-labs/aion-rp-llama-3.1-8b"),
        Some(ModelPrices {
            input: Some(800_000),
            output: Some(1_600_000),
            cache_read: None,
            cache_write: None,
        })
    );
    assert!(
        catalogue
            .prices(ProviderKind::Openrouter, "missing-model")
            .is_none()
    );
}
