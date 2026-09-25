use super::*;

#[test]
fn seed_contracts_round_trip_without_snapshot_or_commit_authority() {
    for seed in production_seeds(crate::tests::test_environment_id()) {
        let bytes = serde_json::to_vec(&seed.definition.to_file()).unwrap();
        let loaded = WorkflowDefinition::from_file_bytes(&bytes).unwrap();
        assert_eq!(loaded, seed.definition);
        assert!(loaded.attempt_bound() <= crate::workflows::definition::MAXIMUM_RUN_ATTEMPTS);
        let json = String::from_utf8(bytes).unwrap();
        for obsolete in [
            "candidate",
            "commit-policy",
            "apply-changes",
            "commit-candidate",
        ] {
            assert!(!json.contains(obsolete));
        }
        assert_eq!(seed.key, SeedKey::parse(seed.key.as_str()).unwrap());
    }
}

#[test]
fn untrusted_seed_keys_are_bounded() {
    for raw in ["", "../escape", "bad key", "9invalid", &"x".repeat(33)] {
        assert!(SeedKey::parse(raw).is_none());
    }
}
