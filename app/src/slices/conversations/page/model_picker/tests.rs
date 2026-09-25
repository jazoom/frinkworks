use super::*;
use crate::providers::{ProviderConnection, ProviderKind};

#[test]
fn catalogue_patches_keep_retired_capabilities_without_new_choices() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    state
        .vault
        .put(ProviderConnection::with_key(
            ProviderKind::Deepseek,
            "test-key",
            "deepseek-v4-flash",
        ))
        .unwrap();
    for selected in ["", "deepseek-v4-flash"] {
        let picker = ModelPicker::new(
            &state.vault,
            &state.preferences,
            &state.models_dev,
            "deepseek",
            selected,
            "high",
        );
        let catalogue: serde_json::Value = serde_json::from_str(&picker.catalogue).unwrap();
        let retired = catalogue["deepseek"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["id"] == "deepseek-v4-flash")
            .unwrap();
        assert_eq!(retired["deprecated"], true);
        assert_eq!(retired["image_input"], true);
        assert!(!retired["efforts"].as_array().unwrap().is_empty());
        if !selected.is_empty() {
            assert!(picker.model_deprecated);
            assert!(!picker.model_unavailable);
            assert_eq!(picker.model, selected);
        }
    }
}
