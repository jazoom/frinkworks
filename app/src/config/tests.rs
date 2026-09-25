use super::*;

impl super::RuntimeConfig {
    pub(crate) fn development() -> Self {
        Self {
            environment: RuntimeEnvironment::Development,
            public_origin: "http://localhost:4000".to_owned(),
        }
    }
}

use super::StartupConfig;
use std::collections::HashMap;

fn development_values() -> HashMap<String, String> {
    [
        ("FRINKWORKS_ENVIRONMENT", "development"),
        ("HOME", "/home/frinkworks"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .collect()
}

#[test]
fn parses_development_defaults() {
    let config = StartupConfig::from_values(development_values()).unwrap();
    assert_eq!(config.runtime.public_origin(), "http://localhost:4000");
    assert_eq!(config.bind_address, "localhost:4000");
    assert_eq!(
        config.static_dir,
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static-development")
    );
    assert_eq!(
        config.data_dir,
        std::path::PathBuf::from("/home/frinkworks/.local/share/frinkworks")
    );
}

#[test]
fn prefers_an_absolute_data_dir() {
    let mut values = development_values();
    values.insert("FRINKWORKS_DATA_DIR".into(), "/var/lib/frinkworks".into());
    let config = StartupConfig::from_values(values).unwrap();
    assert_eq!(
        config.data_dir,
        std::path::PathBuf::from("/var/lib/frinkworks")
    );
}

#[test]
fn rejects_a_relative_data_dir() {
    let mut values = development_values();
    values.insert("FRINKWORKS_DATA_DIR".into(), "data".into());
    assert_eq!(
        StartupConfig::from_values(values).err().unwrap(),
        "FRINKWORKS_DATA_DIR must be absolute"
    );
}

#[test]
fn production_requires_origin() {
    let mut values = development_values();
    values.insert("FRINKWORKS_ENVIRONMENT".into(), "production".into());
    assert_eq!(
        StartupConfig::from_values(values).err().unwrap(),
        "FRINKWORKS_PUBLIC_ORIGIN must be set"
    );
}

#[test]
fn production_uses_production_assets() {
    let mut values = development_values();
    values.insert("FRINKWORKS_ENVIRONMENT".into(), "production".into());
    values.insert(
        "FRINKWORKS_PUBLIC_ORIGIN".into(),
        "https://frinkworks.example".into(),
    );
    let config = StartupConfig::from_values(values).unwrap();
    assert_eq!(
        config.static_dir,
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static-production")
    );
}

#[test]
fn rejects_empty_environment() {
    let mut values = development_values();
    values.insert("FRINKWORKS_ENVIRONMENT".into(), String::new());
    assert_eq!(
        StartupConfig::from_values(values).err().unwrap(),
        "FRINKWORKS_ENVIRONMENT must not be empty"
    );
}

#[test]
fn rejects_relative_static_dir() {
    let mut values = development_values();
    values.insert("FRINKWORKS_STATIC_DIR".into(), "static".into());
    assert_eq!(
        StartupConfig::from_values(values).err().unwrap(),
        "FRINKWORKS_STATIC_DIR must be absolute"
    );
}

#[test]
fn records_absolute_protected_user_roots() {
    let mut values = development_values();
    values.insert("XDG_DATA_HOME".into(), "/xdg/data".into());
    values.insert("USERPROFILE".into(), "/users/me".into());
    values.insert("LOCALAPPDATA".into(), "/users/me/AppData/Local".into());
    let config = StartupConfig::from_values(values).unwrap();
    assert_eq!(
        config.protected_user_roots,
        vec![
            std::path::PathBuf::from("/home/frinkworks"),
            std::path::PathBuf::from("/xdg/data"),
            std::path::PathBuf::from("/users/me"),
            std::path::PathBuf::from("/users/me/AppData/Local"),
        ]
    );
}

#[test]
fn ignores_relative_or_empty_protected_user_roots() {
    let mut values = development_values();
    values.insert("FRINKWORKS_DATA_DIR".into(), "/var/lib/frinkworks".into());
    values.insert("HOME".into(), "relative-home".into());
    values.insert("XDG_DATA_HOME".into(), String::new());
    values.insert("USERPROFILE".into(), "users\\me".into());
    values.insert("LOCALAPPDATA".into(), "AppData/Local".into());
    let config = StartupConfig::from_values(values).unwrap();
    assert!(config.protected_user_roots.is_empty());
}

#[test]
fn uses_the_xdg_data_root() {
    let mut values = development_values();
    values.insert("XDG_DATA_HOME".into(), "/xdg/data".into());
    let config = StartupConfig::from_values(values).unwrap();
    assert_eq!(config.data_dir, PathBuf::from("/xdg/data/frinkworks"));
}

#[test]
fn rejects_non_canonical_origin() {
    let mut values = development_values();
    values.insert(
        "FRINKWORKS_PUBLIC_ORIGIN".into(),
        "http://localhost:4000/".into(),
    );
    assert_eq!(
        StartupConfig::from_values(values).err().unwrap(),
        "FRINKWORKS_PUBLIC_ORIGIN must be canonical"
    );
}
