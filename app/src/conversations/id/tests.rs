use super::{ConversationId, MessageId};

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
