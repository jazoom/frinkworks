use crate::conversations::{ConversationError, ConversationStore};
use crate::providers::{ModelSelection, ProviderKind};
use crate::sessions::JobId;

use super::{
    MAXIMUM_QUEUE_ITEM_BYTES, MAXIMUM_QUEUE_ITEMS, QueueDelivery, QueueItemId, valid_queue,
};

fn store() -> ConversationStore {
    ConversationStore::in_memory()
}

fn selection() -> ModelSelection {
    ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None).expect("selection")
}

fn with_model(store: &ConversationStore) -> crate::conversations::ConversationRecord {
    let record = store.create("Queue".to_owned()).expect("conversation");
    let settings = crate::execution::ExecutionSettings::new(
        selection(),
        String::new(),
        crate::agents::ToolId::ALL.to_vec(),
        crate::tests::test_environment_id(),
    )
    .expect("settings");
    store
        .update_execution_settings(&record.id, record.revision, settings)
        .expect("model")
}

#[test]
fn queue_identifiers_are_opaque_and_unique() {
    let id = QueueItemId::generate().expect("identifier");
    let encoded = id.as_hex();
    assert_eq!(encoded.len(), 32);
    assert_eq!(QueueItemId::parse(&encoded), Some(id));
    assert!(QueueItemId::parse("queue-item").is_none());
    assert_ne!(id, QueueItemId::generate().expect("identifier"));
}

#[test]
fn enqueue_persists_outside_model_history() {
    let directory = tempfile::tempdir().unwrap();
    let store = ConversationStore::open(directory.path().to_owned()).unwrap();
    let record = with_model(&store);
    let queued = store
        .enqueue(
            &record.id,
            record.queue.revision,
            "Follow up later".to_owned(),
            QueueDelivery::FollowUp,
            None,
        )
        .expect("enqueue");
    assert_eq!(queued.messages.len(), 0);
    assert_eq!(queued.queue.items.len(), 1);
    assert_eq!(queued.queue.items[0].text, "Follow up later");
    assert_eq!(queued.queue.items[0].delivery, QueueDelivery::FollowUp);
    assert_eq!(queued.revision, record.revision);
    assert_ne!(queued.queue.revision, record.queue.revision);
    drop(store);
    let store = ConversationStore::open(directory.path().to_owned()).unwrap();
    let reloaded = store.get(&record.id).expect("reload");
    assert_eq!(reloaded.queue.items[0].id, queued.queue.items[0].id);
    assert!(reloaded.messages.is_empty());
}

#[test]
fn queue_bounds_reject_count_and_bytes() {
    let store = store();
    let mut record = with_model(&store);
    for index in 0..MAXIMUM_QUEUE_ITEMS {
        record = store
            .enqueue(
                &record.id,
                record.queue.revision,
                format!("item {index}"),
                QueueDelivery::FollowUp,
                None,
            )
            .expect("enqueue");
    }
    let full = store.enqueue(
        &record.id,
        record.queue.revision,
        "overflow".to_owned(),
        QueueDelivery::FollowUp,
        None,
    );
    assert_eq!(full, Err(ConversationError::Full));
    let oversized = "x".repeat(MAXIMUM_QUEUE_ITEM_BYTES + 1);
    let record = with_model(&store);
    assert_eq!(
        store.enqueue(
            &record.id,
            record.queue.revision,
            oversized,
            QueueDelivery::FollowUp,
            None,
        ),
        Err(ConversationError::Message)
    );
}

#[test]
fn aggregate_bound_leaves_existing_items_unchanged() {
    let store = store();
    let mut record = with_model(&store);
    for _ in 0..super::MAXIMUM_QUEUE_BYTES / MAXIMUM_QUEUE_ITEM_BYTES {
        record = store
            .enqueue(
                &record.id,
                record.queue.revision,
                "x".repeat(MAXIMUM_QUEUE_ITEM_BYTES),
                QueueDelivery::FollowUp,
                None,
            )
            .unwrap();
    }
    assert_eq!(
        store.enqueue(
            &record.id,
            record.queue.revision,
            "x".to_owned(),
            QueueDelivery::FollowUp,
            None
        ),
        Err(ConversationError::Full)
    );
    assert_eq!(store.get(&record.id).unwrap().queue, record.queue);
}

#[test]
fn launch_digest_separates_model_and_instruction_fields() {
    let store = store();
    let mut settings = with_model(&store).model.unwrap().settings;
    settings.model.model = "model-a".to_owned();
    settings.instructions = "bc".to_owned();
    let digest = super::launch_digest(&settings);
    settings.model.model = "model-ab".to_owned();
    settings.instructions = "c".to_owned();
    assert_ne!(digest, super::launch_digest(&settings));
}

#[test]
fn stale_queue_revision_does_not_mutate() {
    let store = store();
    let record = with_model(&store);
    store
        .enqueue(
            &record.id,
            record.queue.revision,
            "first".to_owned(),
            QueueDelivery::FollowUp,
            None,
        )
        .expect("enqueue");
    let stale = store.enqueue(
        &record.id,
        record.queue.revision,
        "second".to_owned(),
        QueueDelivery::FollowUp,
        None,
    );
    assert_eq!(stale, Err(ConversationError::Conflict));
    assert_eq!(store.get(&record.id).expect("record").queue.items.len(), 1);
}

#[test]
fn remove_keeps_the_item_when_the_identifier_is_stale() {
    let store = store();
    let record = with_model(&store);
    let queued = store
        .enqueue(
            &record.id,
            record.queue.revision,
            "keep me".to_owned(),
            QueueDelivery::FollowUp,
            None,
        )
        .expect("enqueue");
    let missing = QueueItemId::generate().expect("other");
    assert_eq!(
        store
            .remove_queue_item(&record.id, queued.queue.revision, missing)
            .expect_err("stale identity"),
        ConversationError::Conflict
    );
    assert_eq!(store.get(&record.id).expect("record").queue.items.len(), 1);
}

#[test]
fn follow_up_claim_is_exactly_once() {
    let store = store();
    let record = with_model(&store);
    let queued = store
        .enqueue(
            &record.id,
            record.queue.revision,
            "Next question".to_owned(),
            QueueDelivery::FollowUp,
            None,
        )
        .expect("enqueue");
    let item = queued.queue.items[0].id;
    let request = JobId::generate().expect("job");
    let started = store
        .begin_follow_up(
            &record.id,
            queued.revision,
            queued.queue.revision,
            item,
            request,
            record.model.clone(),
        )
        .expect("claim");
    assert_eq!(started.queue.items.len(), 0);
    assert_eq!(started.messages[0].text, "Next question");
    assert_eq!(started.active_job, Some(request));
    let duplicate = store.begin_follow_up(
        &record.id,
        started.revision,
        started.queue.revision,
        item,
        JobId::generate().expect("other job"),
        record.model.clone(),
    );
    assert_eq!(duplicate, Err(ConversationError::Active));
}

#[test]
fn follow_up_claim_holds_when_settings_change() {
    let store = store();
    let record = with_model(&store);
    let queued = store
        .enqueue(
            &record.id,
            record.queue.revision,
            "Held".to_owned(),
            QueueDelivery::FollowUp,
            None,
        )
        .expect("enqueue");
    let mut settings = record.model.as_ref().expect("model").settings.clone();
    settings.instructions = "changed".to_owned();
    let updated = store
        .update_execution_settings(&record.id, queued.revision, settings)
        .expect("settings");
    let request = JobId::generate().expect("job");
    let held = store.begin_follow_up(
        &record.id,
        updated.revision,
        queued.queue.revision,
        queued.queue.items[0].id,
        request,
        updated.model.clone(),
    );
    assert_eq!(held, Err(ConversationError::Conflict));
    let stale_preflight = store.begin_follow_up(
        &record.id,
        updated.revision,
        queued.queue.revision,
        queued.queue.items[0].id,
        request,
        record.model.clone(),
    );
    assert_eq!(stale_preflight, Err(ConversationError::Conflict));
    assert_eq!(store.get(&record.id).expect("record").model, updated.model);
    assert_eq!(store.get(&record.id).expect("record").queue.items.len(), 1);
    assert!(store.get(&record.id).expect("record").active_job.is_none());
}

#[test]
fn steering_delivery_rejects_a_stale_item_and_keeps_the_pending_assistant() {
    let store = store();
    let record = with_model(&store);
    let request = JobId::generate().expect("job");
    let started = store
        .begin_message_with_model(
            &record.id,
            record.revision,
            record.model.clone(),
            request,
            "Run tools".to_owned(),
        )
        .expect("message");
    let queued = store
        .enqueue(
            &record.id,
            started.queue.revision,
            "Use the other file".to_owned(),
            QueueDelivery::Steering,
            Some(request),
        )
        .expect("enqueue");
    let stale = QueueItemId::generate().expect("stale");
    assert_eq!(
        store
            .deliver_steering(
                &record.id,
                request,
                stale,
                crate::providers::AssistantReply {
                    text: "Working".to_owned(),
                    ..Default::default()
                },
            )
            .expect_err("stale delivery"),
        ConversationError::Conflict
    );
    let delivered = store
        .deliver_steering(
            &record.id,
            request,
            queued.queue.items[0].id,
            crate::providers::AssistantReply {
                text: "Working".to_owned(),
                ..Default::default()
            },
        )
        .expect("deliver");
    assert_eq!(delivered, "Use the other file");
    let record = store.get(&record.id).expect("record");
    assert!(record.queue.items.is_empty());
    assert_eq!(
        record.messages[1].status,
        crate::conversations::MessageStatus::Complete
    );
    assert_eq!(record.messages[2].text, "Use the other file");
    assert_eq!(
        record.messages[3].status,
        crate::conversations::MessageStatus::Pending
    );
    assert_eq!(record.active_job, Some(request));
}

#[test]
fn valid_queue_rejects_duplicate_identifiers() {
    let id = QueueItemId::generate().expect("id");
    let item = super::QueueItem {
        id,
        text: "once".to_owned(),
        attachments: Vec::new(),
        delivery: QueueDelivery::FollowUp,
        settings_digest: super::launch_digest(
            &crate::execution::ExecutionSettings::new(
                selection(),
                String::new(),
                crate::agents::ToolId::ALL.to_vec(),
                crate::tests::test_environment_id(),
            )
            .expect("settings"),
        ),
        job: None,
    };
    let queue = super::ConversationQueue {
        revision: 1,
        items: vec![item.clone(), item],
    };
    assert!(!valid_queue(&queue));
}
