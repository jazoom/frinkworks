use std::fs;

use super::{Preferences, Theme};
use crate::providers::{
    MAXIMUM_FAVOURITES, MAXIMUM_MODEL_BYTES, ProviderConnection, ProviderKind, ThinkingEffort,
};
use crate::vault::ProviderVault;

impl Preferences {
    pub(crate) fn selected_provider(
        &self,
        vault: &crate::vault::ProviderVault,
    ) -> Option<super::DeskProvider> {
        self.desk_providers(vault)
            .into_iter()
            .find(|provider| provider.selected)
    }
}

#[test]
fn model_preferences_round_trip_independently_of_credentials_and_other_preferences() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preferences.json");
    let vault_path = dir.path().join("providers.json");
    let vault = ProviderVault::open(vault_path.clone()).unwrap();
    for kind in [ProviderKind::Xai, ProviderKind::Deepseek] {
        vault
            .put(ProviderConnection::with_key(
                kind,
                "test-secret",
                kind.default_model(),
            ))
            .unwrap();
    }
    let credential_bytes = fs::read(&vault_path).unwrap();
    let writer = Preferences::open(path.clone());
    writer
        .select_settings(
            ProviderKind::Xai,
            "grok-custom".to_owned(),
            ThinkingEffort::new("high".to_owned()),
        )
        .unwrap();
    writer
        .select_settings(
            ProviderKind::Deepseek,
            "deepseek-custom".to_owned(),
            ThinkingEffort::new("max".to_owned()),
        )
        .unwrap();
    writer
        .toggle_favourite(ProviderKind::Xai, "grok-custom")
        .unwrap();
    writer.set_theme(Theme::Sector7G).unwrap();
    writer.set_show_thinking(true).unwrap();
    assert_eq!(fs::read(&vault_path).unwrap(), credential_bytes);
    assert!(!fs::read_to_string(&path).unwrap().contains("test-secret"));

    let reader = Preferences::open(path.clone());
    let selected = reader.selected_provider(&vault).unwrap();
    assert_eq!(
        (
            selected.kind,
            selected.model.as_str(),
            selected.thinking.as_ref().map(ThinkingEffort::as_str)
        ),
        (ProviderKind::Deepseek, "deepseek-custom", Some("max"))
    );
    assert_eq!(reader.theme(), Theme::Sector7G);
    assert!(reader.show_thinking());
    vault
        .put(ProviderConnection::with_key(
            ProviderKind::Xai,
            "replacement-secret",
            "ignored-model",
        ))
        .unwrap();
    vault.forget(ProviderKind::Deepseek).unwrap();
    let fallback = reader.selected_provider(&vault).unwrap();
    assert_eq!(
        (
            fallback.kind,
            fallback.model.as_str(),
            fallback.thinking.as_ref().map(ThinkingEffort::as_str)
        ),
        (ProviderKind::Xai, "grok-custom", Some("high"))
    );
    assert_eq!(fallback.favourites, ["grok-custom"]);
    reader.forget_provider(ProviderKind::Deepseek).unwrap();
    assert!(
        Preferences::open(path)
            .values()
            .models
            .iter()
            .all(|entry| entry.selection.provider != ProviderKind::Deepseek)
    );
}

#[test]
fn invalid_model_preferences_default_without_rewriting_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preferences.json");
    let valid = serde_json::json!({
        "version": 1, "theme": "sector-7-g", "show_thinking": true,
        "selected_provider": "xai",
        "models": [{ "selection": {"provider": "xai", "model": "grok-custom", "thinking": "high"}, "favourites": [] }]
    });
    let mut cases = Vec::new();
    for model in [
        "".to_owned(),
        " spaced ".to_owned(),
        "bad\nmodel".to_owned(),
        "x".repeat(MAXIMUM_MODEL_BYTES + 1),
    ] {
        let mut value = valid.clone();
        value["models"][0]["selection"]["model"] = model.into();
        cases.push(value);
    }
    for effort in ["", "default", " high ", "bad\neffort", &"x".repeat(33)] {
        let mut value = valid.clone();
        value["models"][0]["selection"]["thinking"] = effort.into();
        cases.push(value);
    }
    for favourites in [
        vec!["same"; 2],
        vec![""],
        vec![" spaced "],
        vec!["bad\nmodel"],
        vec!["model"; MAXIMUM_FAVOURITES + 1],
    ] {
        let mut value = valid.clone();
        value["models"][0]["favourites"] = serde_json::json!(favourites);
        cases.push(value);
    }
    let mut duplicate = valid.clone();
    duplicate["models"]
        .as_array_mut()
        .unwrap()
        .push(valid["models"][0].clone());
    cases.push(duplicate);
    for provider in ["missing", "deepseek"] {
        let mut value = valid.clone();
        value["selected_provider"] = provider.into();
        cases.push(value);
    }
    let mut secret = valid.clone();
    secret["models"][0]["api_key"] = "not-allowed".into();
    cases.push(secret);
    for value in cases {
        let bytes = serde_json::to_vec(&value).unwrap();
        crate::storage::write_private(&path, &bytes).unwrap();
        let reader = Preferences::open(path.clone());
        assert!(reader.values().models.is_empty(), "{value}");
        assert_eq!(reader.theme(), Theme::System);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn maximum_favourite_lists_respect_the_cap_and_fit_within_the_file_bound() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preferences.json");
    let writer = Preferences::open(path.clone());
    for kind in ProviderKind::ALL {
        for index in 0..MAXIMUM_FAVOURITES {
            let model = format!("{index:02}{}", r#"""#.repeat(MAXIMUM_MODEL_BYTES - 2));
            assert!(writer.toggle_favourite(kind, &model).unwrap());
        }
        assert!(matches!(
            writer.toggle_favourite(kind, "one-more"),
            Err(super::FavouriteError::Full)
        ));
    }
    let reader = Preferences::open(path);
    assert_eq!(reader.values().models.len(), ProviderKind::ALL.len());
    assert!(
        reader
            .values()
            .models
            .iter()
            .all(|entry| entry.favourites.len() == MAXIMUM_FAVOURITES)
    );
    let first = reader.values().models[0].favourites[0].clone();
    assert!(!reader.toggle_favourite(ProviderKind::Xai, &first).unwrap());
    assert!(
        reader
            .toggle_favourite(ProviderKind::Xai, "one-more")
            .unwrap()
    );
}

#[test]
fn a_failed_model_preference_write_keeps_the_previous_selection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preferences.json");
    let preferences = Preferences::open(path.clone());
    preferences
        .select_settings(ProviderKind::Xai, "grok-custom".to_owned(), None)
        .unwrap();
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(
        preferences
            .select_settings(ProviderKind::Deepseek, "other".to_owned(), None)
            .is_err()
    );
    assert_eq!(
        preferences.values().selected_provider,
        Some(ProviderKind::Xai)
    );
    assert_eq!(preferences.values().models.len(), 1);
}

#[test]
fn a_saved_theme_survives_a_new_store_instance() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("preferences.json");

    let writer = Preferences::open(path.clone());
    writer.set_theme(Theme::Sector7G).expect("save theme");

    let reader = Preferences::open(path);
    assert_eq!(reader.theme(), Theme::Sector7G);
}

#[test]
fn thinking_visibility_survives_a_new_store_instance() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("preferences.json");

    let writer = Preferences::open(path.clone());
    writer.set_show_thinking(true).expect("save visibility");

    let reader = Preferences::open(path);
    assert!(reader.show_thinking());
    assert_eq!(reader.theme(), Theme::System);
}

#[test]
fn preference_updates_preserve_other_values() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("preferences.json");
    let preferences = Preferences::open(path.clone());

    preferences
        .set_theme(Theme::EvergreenTerrace)
        .expect("save theme");
    preferences
        .set_show_thinking(true)
        .expect("save visibility");

    let reader = Preferences::open(path);
    assert_eq!(reader.theme(), Theme::EvergreenTerrace);
    assert!(reader.show_thinking());
}

#[test]
fn missing_or_invalid_files_default_to_the_system_preference() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("preferences.json");
    assert_eq!(Preferences::open(path.clone()).theme(), Theme::System);

    for bytes in [
        b"not json".as_slice(),
        br#"{"version":2,"theme":"sector-7-g","show_thinking":true,"selected_provider":null,"models":[]}"#,
        br#"{"version":1,"theme":"unknown","show_thinking":true,"selected_provider":null,"models":[]}"#,
        br#"{"version":1,"theme":"sector-7-g","show_thinking":true,"selected_provider":null,"models":[],"removed-field":true}"#,
    ] {
        fs::write(&path, bytes).expect("write invalid preferences");
        assert_eq!(Preferences::open(path.clone()).theme(), Theme::System);
    }
}

#[test]
fn conversation_defaults_keep_theme_and_requested_settings_without_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preferences.json");
    let preferences = Preferences::open(path.clone());
    preferences.set_theme(Theme::Sector7G).unwrap();
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None)
            .unwrap(),
        "Use concise explanations.".to_owned(),
        vec![crate::agents::ToolId::Run],
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_location(crate::execution::ToolLocation::Host)
    .with_host_approval(crate::execution::HostApprovalPolicy::Automatic);
    preferences
        .set_conversation_defaults(settings.clone())
        .unwrap();
    let reader = Preferences::open(path.clone());
    assert_eq!(reader.theme(), Theme::Sector7G);
    assert_eq!(reader.conversation_defaults(), Some(settings));
    let persisted: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(persisted["version"], 1);
    assert!(persisted["conversation_defaults"].get("consent").is_none());
    assert!(
        persisted["conversation_defaults"]
            .get("directory_approvals")
            .is_none()
    );
}

#[test]
fn clearing_conversation_defaults_keeps_other_preferences_and_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preferences.json");
    let vault_path = dir.path().join("providers.json");
    let vault = ProviderVault::open(vault_path.clone()).unwrap();
    vault
        .put(ProviderConnection::with_key(
            ProviderKind::Xai,
            "test-secret",
            "grok-4.6",
        ))
        .unwrap();
    let credential_bytes = fs::read(&vault_path).unwrap();
    let preferences = Preferences::open(path.clone());
    preferences.set_theme(Theme::Sector7G).unwrap();
    preferences
        .select_settings(ProviderKind::Xai, "grok-custom".to_owned(), None)
        .unwrap();
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None)
            .unwrap(),
        "Use concise explanations.".to_owned(),
        vec![crate::agents::ToolId::Read],
        crate::tests::test_environment_id(),
    )
    .unwrap();
    preferences.set_conversation_defaults(settings).unwrap();
    preferences.clear_conversation_defaults().unwrap();
    assert!(preferences.conversation_defaults().is_none());
    assert_eq!(preferences.theme(), Theme::Sector7G);
    assert_eq!(fs::read(&vault_path).unwrap(), credential_bytes);
    let reader = Preferences::open(path.clone());
    assert!(reader.conversation_defaults().is_none());
    assert_eq!(reader.theme(), Theme::Sector7G);
    assert_eq!(reader.values().selected_provider, Some(ProviderKind::Xai));
    let persisted: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert!(persisted["conversation_defaults"].is_null());
}

#[test]
fn compaction_defaults_to_enabled_at_95_percent() {
    let preferences = Preferences::in_memory();
    let default = super::CompactionPreference::default();
    assert_eq!(preferences.compaction(), default);
    assert!(default.enabled);
    assert_eq!(default.threshold, 95);
}

#[test]
fn compaction_preferences_round_trip_and_reject_out_of_range_percentages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preferences.json");
    let preferences = Preferences::open(path.clone());
    let disabled = super::CompactionPreference {
        enabled: false,
        threshold: 42,
    };
    preferences.set_compaction(disabled).unwrap();
    assert_eq!(Preferences::open(path.clone()).compaction(), disabled);
    let persisted: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(persisted["automatic_compaction"], false);
    assert_eq!(persisted["compaction_threshold"], 42);
    for threshold in [0u8, 101, u8::MAX] {
        let invalid = super::CompactionPreference {
            enabled: true,
            threshold,
        };
        assert!(preferences.set_compaction(invalid).is_err());
        assert_eq!(preferences.compaction(), disabled);
    }
}

#[test]
fn an_invalid_compaction_threshold_defaults_without_rewriting_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preferences.json");
    let bytes = br#"{"version":1,"theme":"system","show_thinking":false,"automatic_compaction":false,"compaction_threshold":0,"selected_provider":null,"models":[]}"#;
    crate::storage::write_private(&path, bytes).unwrap();
    let reader = Preferences::open(path.clone());
    assert_eq!(reader.compaction(), super::CompactionPreference::default());
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn an_absent_compaction_field_uses_the_enabled_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preferences.json");
    let bytes = br#"{"version":1,"theme":"system","show_thinking":false,"selected_provider":null,"models":[]}"#;
    crate::storage::write_private(&path, bytes).unwrap();
    let reader = Preferences::open(path);
    assert_eq!(reader.compaction(), super::CompactionPreference::default());
}

#[test]
fn only_known_themes_parse() {
    for theme in Theme::ALL {
        assert_eq!(Theme::parse(theme.as_str()), Some(*theme));
    }
    assert_eq!(Theme::parse("Evergreen Terrace"), None);
    assert_eq!(Theme::parse("unknown"), None);
}
