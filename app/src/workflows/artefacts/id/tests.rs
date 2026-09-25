use super::*;

#[test]
fn exact_byte_changes_create_different_object_hashes() {
    let left = ObjectHash::of(b"plan");
    let right = ObjectHash::of(b"Plan");
    assert_ne!(left, right);
    assert_ne!(left.as_str(), right.as_str());
    assert!(left.as_str().starts_with("sha256:"));
    assert_eq!(ObjectHash::parse(&left.as_str()), Some(left));
    assert!(ObjectHash::parse(&left.as_str().replacen("sha256:", "SHA256:", 1)).is_none());
}

#[test]
fn artefact_and_object_hash_contracts_remain_separate() {
    let payload = b"{\"format-version\":1}";
    let object = ObjectHash::of(payload);
    let artefact = ArtefactHash::of(b"frinkworks.artefact.v1\0plan\0\0\0\0\x01", payload);
    assert_ne!(object.as_str(), artefact.as_str());
}
