use super::{ConversationId, MessageId, RequestId};

#[test]
fn identifiers_are_opaque_and_round_trip() {
    let id = ConversationId::generate().expect("identifier");
    let encoded = id.as_hex();
    assert_eq!(encoded.len(), 32);
    assert_eq!(ConversationId::parse(&encoded), Some(id));
    assert!(ConversationId::parse("project-name").is_none());
}

#[test]
fn message_identifiers_are_stable_across_projection() {
    let id = MessageId::generate().expect("identifier");
    let encoded = id.as_hex();
    assert_eq!(encoded.len(), 32);
    assert_eq!(MessageId::parse(&encoded), Some(id));
    assert!(MessageId::parse("1").is_none());
    assert_ne!(id, MessageId::generate().expect("identifier"));
}

#[test]
fn request_identifiers_round_trip() {
    let id = RequestId::generate().expect("identifier");
    let encoded = id.as_hex();
    assert_eq!(encoded.len(), 32);
    assert_eq!(RequestId::parse(&encoded), Some(id));
    assert!(RequestId::parse("usage").is_none());
    let encoded = serde_json::to_string(&id).expect("json");
    assert_eq!(
        serde_json::from_str::<RequestId>(&encoded).expect("parsed"),
        id
    );
}
