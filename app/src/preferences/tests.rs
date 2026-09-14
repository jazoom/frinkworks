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
fn only_known_themes_parse() {
    for theme in Theme::ALL {
        assert_eq!(Theme::parse(theme.as_str()), Some(*theme));
    }
    assert_eq!(Theme::parse("Evergreen Terrace"), None);
    assert_eq!(Theme::parse("unknown"), None);
}
