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
