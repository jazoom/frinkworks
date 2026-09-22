use super::*;

const BUNDLED: &[u8] = include_bytes!("../../../../catalogue/models-dev-v1.json");

#[test]
fn background_metadata_rejects_unknown_prices_and_non_production_routes() {
    let fixture = serde_json::json!({
        "attachment": false, "tool_call": true,
        "modalities": {"input": ["text"], "output": ["text"]},
        "limit": {"context": 4096, "output": 128},
        "release_date": "2026-09-01",
        "cost": {"input": 0.1, "output": 0.2},
        "experimental": {"modes": {"fast": {}}}
    });
    let model: SourceModel = serde_json::from_value(fixture.clone()).unwrap();
    assert!(background_metadata("ordinary", &model).is_some());
    assert!(background_metadata("ordinary:free", &model).is_none());
    for (field, value) in [
        ("cost", serde_json::Value::Null),
        ("cost", serde_json::json!({"input": 0.0, "output": 0.0})),
        ("cost", serde_json::json!({"input": 0.1})),
        ("release_date", serde_json::json!("unknown")),
        ("experimental", serde_json::json!(true)),
        ("status", serde_json::json!("beta")),
        ("status", serde_json::json!("deprecated")),
    ] {
        let mut value_fixture = fixture.clone();
        value_fixture[field] = value;
        let model: SourceModel = serde_json::from_value(value_fixture).unwrap();
        assert!(background_metadata("ordinary", &model).is_none(), "{field}");
    }
}

#[test]
fn canonical_validation_rejects_unknown_fields_and_order_changes() {
    let value: serde_json::Value = serde_json::from_slice(BUNDLED).expect("catalogue");
    let mut unknown = value.clone();
    unknown["providers"][0]["models"][0]["agent_compatible"] = serde_json::Value::Bool(true);
    let unknown = serde_json::to_vec(&unknown).expect("unknown");
    assert!(parse_snapshot(&unknown).is_err());
    assert_eq!(
        validate_canonical_catalogue(&unknown).expect_err("unknown field"),
        "The checked-in catalogue is invalid."
    );

    let mut reordered = value;
    reordered["providers"]
        .as_array_mut()
        .expect("providers")
        .swap(0, 1);
    assert_eq!(
        validate_canonical_catalogue(&serde_json::to_vec(&reordered).expect("reordered"))
            .expect_err("provider order"),
        "The checked-in catalogue is not canonical."
    );
}

#[test]
fn source_filter_uses_fallback_identifiers_and_deduplicates_efforts() {
    let mut source = serde_json::Map::new();
    for kind in ProviderKind::ALL {
        source.insert(
            models_dev_id(kind).to_owned(),
            serde_json::json!({
                "models": {
                    kind.default_model(): {
                        "attachment": true,
                        "reasoning": true,
                        "reasoning_options": [{
                            "type": "effort",
                            "values": [null, "default", "high", "high"]
                        }],
                        "tool_call": true,
                        "modalities": {"input": ["text"], "output": ["text"]},
                        "limit": {"context": 128000, "input": 120000, "output": 8000}
                    }
                }
            }),
        );
    }

    let openai_models = source
        .get_mut(models_dev_id(ProviderKind::OpenaiCodex))
        .and_then(|provider| provider.get_mut("models"))
        .and_then(serde_json::Value::as_object_mut)
        .expect("OpenAI models");
    for (id, tool_call, input, output, status) in [
        (
            "audio-output",
            true,
            serde_json::json!(["text"]),
            serde_json::json!(["text", "audio"]),
            serde_json::Value::Null,
        ),
        (
            "no-text-input",
            true,
            serde_json::json!(["image"]),
            serde_json::json!(["text"]),
            serde_json::Value::Null,
        ),
        (
            "no-tools",
            false,
            serde_json::json!(["text"]),
            serde_json::json!(["text"]),
            serde_json::Value::Null,
        ),
        (
            "deprecated",
            true,
            serde_json::json!(["text"]),
            serde_json::json!(["text"]),
            serde_json::json!("deprecated"),
        ),
    ] {
        openai_models.insert(
            id.to_owned(),
            serde_json::json!({
                "attachment": false,
                "tool_call": tool_call,
                "modalities": {"input": input, "output": output},
                "status": status,
                "limit": {"context": 64000, "output": 4000}
            }),
        );
    }

    openai_models.insert(
        "title-only".to_owned(),
        serde_json::json!({
            "attachment": false, "tool_call": false,
            "modalities": {"input": ["text"], "output": ["text"]},
            "limit": {"context": 4096, "output": 128},
            "release_date": "2026-09-01", "cost": {"input": 0.1, "output": 0.2}
        }),
    );

    let snapshot = filter_source(
        &serde_json::to_vec(&source).expect("source"),
        "W/\"fixture\"",
        42,
    )
    .expect("filtered source");

    assert_eq!(snapshot.checked_at_unix_seconds, 42);
    assert!(snapshot.providers.iter().all(|provider| {
        let model = provider
            .models
            .iter()
            .find(|model| model.id == ProviderKind::parse(&provider.id).unwrap().default_model())
            .expect("default model");
        model.efforts == ["high"]
            && model.attachment
            && model.limit.context == 128_000
            && model.limit.output == 8_000
    }));
    let openai = snapshot
        .providers
        .iter()
        .find(|provider| provider.id == ProviderKind::OpenaiCodex.as_str())
        .expect("OpenAI provider");
    assert_eq!(openai.models.len(), 2);
    let title_only = openai
        .models
        .iter()
        .find(|model| model.id == "title-only")
        .unwrap();
    assert!(!title_only.supports_tools);
    assert!(title_only.background.is_some());

    let stored = serde_json::to_value(&snapshot).expect("stored snapshot");
    let model = &stored["providers"][0]["models"][0];
    assert!(model.get("agent_compatible").is_none());
    let limit = model["limit"].as_object().expect("limit");
    assert_eq!(limit.len(), 2);
    assert_eq!(limit["output"], serde_json::json!(8_000));
}

#[test]
fn source_filter_rejects_an_explicit_empty_identifier() {
    let mut source = serde_json::Map::new();
    for kind in ProviderKind::ALL {
        source.insert(
            models_dev_id(kind).to_owned(),
            serde_json::json!({
                "models": {
                    kind.default_model(): {
                        "id": "",
                        "attachment": false,
                        "tool_call": true,
                        "modalities": {"input": ["text"], "output": ["text"]},
                        "limit": {"context": 128000, "output": 8000}
                    }
                }
            }),
        );
    }

    assert!(filter_source(&serde_json::to_vec(&source).expect("source"), "", 0).is_err());
}

#[test]
fn svg_validation_rejects_active_and_external_content() {
    assert!(
        validate_svg(br#"<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0"/></svg>"#).is_ok()
    );
    assert!(validate_svg(br#"<svg><script /></svg>"#).is_err());
    assert!(validate_svg(br#"<svg><image href="&#x68;ttps://example.test/a" /></svg>"#).is_err());
    assert!(validate_svg(br#"<svg><path onload="run()" /></svg>"#).is_err());
}

#[test]
fn token_price_rejects_invalid_and_overflowing_values() {
    assert_eq!(token_price(0.0), Some(0));
    assert_eq!(token_price(1.25), Some(1_250_000));
    assert!(token_price(f64::NAN).is_none());
    assert!(token_price(f64::INFINITY).is_none());
    assert!(token_price(-0.1).is_none());
    assert!(token_price(1e30).is_none());
}

#[test]
fn source_prices_keep_unknown_values_and_cache_rates() {
    let mut source = serde_json::Map::new();
    for kind in ProviderKind::ALL {
        let mut model = serde_json::json!({
            "attachment": false,
            "tool_call": true,
            "modalities": {"input": ["text"], "output": ["text"]},
            "limit": {"context": 128000, "output": 8000}
        });
        if kind == ProviderKind::Xai {
            model["cost"] = serde_json::json!({
                "input": 3.0,
                "output": 15.0,
                "cache_read": 0.3,
                "cache_write": 3.75,
                "reasoning": f64::NAN
            });
        }
        if kind == ProviderKind::Deepseek {
            model["cost"] = serde_json::json!({"input": -1.0, "output": 2.0});
        }
        source.insert(
            models_dev_id(kind).to_owned(),
            serde_json::json!({"models": {kind.default_model(): model}}),
        );
    }
    let snapshot = filter_source(
        &serde_json::to_vec(&source).expect("source"),
        "W/\"prices\"",
        1,
    )
    .expect("filtered source");
    let xai = snapshot
        .providers
        .iter()
        .find(|provider| provider.id == ProviderKind::Xai.as_str())
        .expect("xai")
        .models
        .iter()
        .find(|model| model.id == ProviderKind::Xai.default_model())
        .expect("model");
    let prices = xai.prices.expect("prices");
    assert_eq!(prices.input, Some(3_000_000));
    assert_eq!(prices.output, Some(15_000_000));
    assert_eq!(prices.cache_read, Some(300_000));
    assert_eq!(prices.cache_write, Some(3_750_000));
    let deepseek = snapshot
        .providers
        .iter()
        .find(|provider| provider.id == ProviderKind::Deepseek.as_str())
        .expect("deepseek")
        .models
        .iter()
        .find(|model| model.id == ProviderKind::Deepseek.default_model())
        .expect("model");
    assert_eq!(
        deepseek.prices.expect("output only").output,
        Some(2_000_000)
    );
    assert!(deepseek.prices.unwrap().input.is_none());
}

#[test]
fn atomic_write_replaces_a_file_without_temporary_files() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("catalogue.json");
    fs::write(&path, b"old").expect("old file");

    atomic_write(&path, b"new").expect("atomic write");

    assert_eq!(fs::read(&path).expect("new file"), b"new");
    assert_eq!(fs::read_dir(directory.path()).expect("entries").count(), 1);
}
