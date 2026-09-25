use super::*;

#[test]
fn kind_and_payload_changes_create_different_artefact_hashes() {
    let (left, _, left_hash) = encode_plan("# Plan\n", None).expect("left");
    let (_right, _, right_hash) = encode_plan("# Other\n", None).expect("right");
    assert_ne!(left_hash, right_hash);
    let object = ObjectHash::of(&left);
    assert_ne!(object.as_str(), left_hash.as_str());
    let review = encode_review(ReviewVerdict::Approved, "# Plan\n", None).expect("review");
    assert_ne!(review.2, left_hash);
    assert_ne!(review.1, ObjectHash::of(&left));
}

#[test]
fn stable_order_creates_the_same_hash() {
    let (_, _, first) = encode_plan("Hello\n", None).expect("first");
    let (_, _, second) = encode_plan("Hello\r\n", None).expect("second");
    assert_eq!(first, second);
    let (bytes, _, _) = encode_plan("caf\u{e9}", None).expect("unicode");
    assert!(
        String::from_utf8(bytes)
            .expect("utf8")
            .contains("caf\u{e9}")
    );
}

#[test]
fn payload_bounds_and_nul_text_are_rejected() {
    assert_eq!(
        encode_plan("bad\u{0000}text", None).err(),
        Some(PayloadError::Text)
    );
    let huge = "a".repeat(MAXIMUM_PLAN_BYTES + 1);
    assert_eq!(encode_plan(&huge, None).err(), Some(PayloadError::Bound));
    assert_eq!(
        encode_plan("use sk-secret", Some("sk-secret")).err(),
        Some(PayloadError::Credential)
    );
    assert!(ArtefactHash::parse("sha256:zz").is_none());
}

#[test]
fn plan_decisions_bind_an_exact_plan_and_use_a_distinct_payload() {
    let (_, _, plan) = encode_plan("# Plan\n", None).expect("plan");
    let (bytes, _, decision_hash) = encode_plan_decision(
        plan,
        crate::workflows::gates::PlanDecisionKind::Accepted,
        None,
        1,
        None,
    )
    .expect("decision");
    assert_ne!(plan, decision_hash);
    let TypedPayload::PlanDecision(decision) =
        parse_typed_payload(ArtefactKind::PlanDecision, &bytes).expect("parsed decision")
    else {
        panic!("plan decision payload");
    };
    assert_eq!(decision.plan, plan.as_str());
    assert_eq!(
        decision.decision,
        crate::workflows::gates::PlanDecisionKind::Accepted
    );
    assert_eq!(
        parse_typed_payload(ArtefactKind::PlanDecision, br#"{"format-version":1,"plan":"bad","decision":"accepted","note":null,"decided-at-ms":1}"#).err(),
        Some(PayloadError::Format)
    );
}

#[test]
fn duplicate_fields_are_rejected() {
    let bytes = br#"{"format-version":1,"markdown":"a","markdown":"b"}"#;
    assert_eq!(
        parse_typed_payload(ArtefactKind::Plan, bytes).err(),
        Some(PayloadError::DuplicateField)
    );
}
