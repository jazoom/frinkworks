use super::*;

impl Database {
    pub(crate) fn in_memory() -> Result<Self, ConversationError> {
        let connection = Connection::open_in_memory().map_err(map_error)?;
        let db = Self { connection };
        db.initialise()?;
        Ok(db)
    }
}

#[test]
fn usage_index_preserves_scope_identity_and_transaction_boundaries() {
    let source = super::super::ConversationStore::in_memory();
    let first = source.create("First".to_owned()).unwrap();
    let other = source.create("Other".to_owned()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::open(dir.path()).unwrap();
    db.save(None, &first).unwrap();
    db.save(None, &other).unwrap();
    let mut usage = crate::providers::ModelUsage::new(
        crate::providers::ProviderKind::Deepseek,
        "deepseek-flash",
    );
    usage.input_tokens = Some(100);
    usage.output_tokens = Some(10);
    usage.cache_read_tokens = Some(80);
    let mut request = RequestUsage {
        id: crate::conversations::RequestId::generate().unwrap(),
        usage,
        auth: crate::providers::AuthMethod::ApiKey,
        prices: None,
        sources: Vec::new(),
        advertised: Vec::new(),
    };
    for conversation in [first.id, other.id] {
        for sequence in 0..2 {
            db.connection.execute(
                "INSERT INTO messages (conversation_id, sequence, id, message) VALUES (?1, ?2, ?3, ?4)",
                params![conversation.as_hex(), sequence, format!("message-{sequence}"),
                    serde_json::json!({"requests": [&request]}).to_string()],
            ).unwrap();
        }
    }
    // Repeated request identities across retained phases contribute once.
    assert_eq!(db.usage_totals(&first.id, &[]).unwrap().tokens.known, 110);
    request.usage.output_tokens = Some(20);
    assert_eq!(
        db.usage_totals(&first.id, &[request.clone()])
            .unwrap()
            .tokens
            .known,
        120
    );
    request.id = crate::conversations::RequestId::generate().unwrap();
    db.connection.execute(
        "INSERT INTO summary_requests (conversation_id, sequence, id, request) VALUES (?1, 0, ?2, ?3)",
        params![first.id.as_hex(), request.id.as_hex(), serde_json::to_string(&request).unwrap()],
    ).unwrap();
    assert_eq!(db.usage_totals(&first.id, &[]).unwrap().tokens.known, 230);
    assert_eq!(db.usage_totals(&other.id, &[]).unwrap().tokens.known, 110);

    db.connection.execute_batch("BEGIN").unwrap();
    db.connection.execute(
        "UPDATE messages SET message = json_set(message, '$.requests[0].usage.output-tokens', 42)
         WHERE conversation_id = ?1", [first.id.as_hex()],
    ).unwrap();
    assert_eq!(db.usage_totals(&first.id, &[]).unwrap().tokens.known, 262);
    db.connection.execute_batch("ROLLBACK").unwrap();
    assert_eq!(db.usage_totals(&first.id, &[]).unwrap().tokens.known, 230);
    request.usage.output_tokens = Some(40);
    db.connection
        .execute(
            "UPDATE summary_requests SET request = ?2 WHERE conversation_id = ?1",
            params![first.id.as_hex(), serde_json::to_string(&request).unwrap()],
        )
        .unwrap();
    db.connection
        .execute(
            "DELETE FROM messages WHERE conversation_id = ?1 AND sequence = 0",
            [first.id.as_hex()],
        )
        .unwrap();
    assert_eq!(db.usage_totals(&first.id, &[]).unwrap().tokens.known, 250);
    drop(db);
    let mut db = Database::open(dir.path()).unwrap();
    assert_eq!(db.usage_totals(&first.id, &[]).unwrap().tokens.known, 250);
    db.remove(&first.id).unwrap();
    assert_eq!(db.usage_totals(&first.id, &[]).unwrap().requests, 0);
    assert_eq!(db.usage_totals(&other.id, &[]).unwrap().tokens.known, 110);
}

#[test]
fn recorded_time_stays_scoped_and_a_live_checkpoint_never_counts_twice() {
    let dir = tempfile::tempdir().unwrap();
    let source = super::super::ConversationStore::in_memory();
    let first = source.create("First".to_owned()).unwrap();
    let other = source.create("Other".to_owned()).unwrap();
    let mut db = Database::open(dir.path()).unwrap();
    db.save(None, &first).unwrap();
    db.save(None, &other).unwrap();
    let job = crate::sessions::JobId::generate().unwrap();
    db.record_work_time(&first.id, job, 12_000).unwrap();
    db.record_work_time(&first.id, job, 5_000).unwrap();
    assert_eq!(db.work_time(&first.id, None).unwrap().total_ms, 12_000);
    assert_eq!(
        db.work_time(&first.id, Some((job, 14_000)))
            .unwrap()
            .total_ms,
        14_000
    );
    assert_eq!(db.work_time(&other.id, None).unwrap().total_ms, 0);
    drop(db);
    let mut db = Database::open(dir.path()).unwrap();
    assert_eq!(db.work_time(&first.id, None).unwrap().total_ms, 12_000);
    db.remove(&first.id).unwrap();
    assert_eq!(db.work_time(&first.id, None).unwrap().total_ms, 0);
}

#[test]
fn full_disk_rolls_back_metadata_and_entries_without_replay() {
    let source = super::super::ConversationStore::in_memory();
    let initial = source.create("Disk full".to_owned()).unwrap();
    let next = source
        .begin_message_with_model(
            &initial.id,
            initial.revision,
            None,
            crate::sessions::JobId::generate().unwrap(),
            "x".repeat(32 * 1024),
        )
        .unwrap();
    let mut database = Database::in_memory().unwrap();
    database.save(None, &initial).unwrap();
    let pages: i64 = database
        .connection
        .query_row("PRAGMA page_count", [], |row| row.get(0))
        .unwrap();
    database
        .connection
        .pragma_update(None, "max_page_count", pages)
        .unwrap();
    assert_eq!(
        database.save(Some(&initial), &next),
        Err(ConversationError::Full)
    );
    assert_eq!(database.load(&initial.id).unwrap(), Some(initial.clone()));
    database
        .connection
        .pragma_update(None, "max_page_count", pages + 100)
        .unwrap();
    assert_eq!(database.load(&initial.id).unwrap(), Some(initial));
}

#[test]
fn busy_database_rejects_the_commit_without_replay() {
    let dir = tempfile::tempdir().unwrap();
    let mut database = Database::open(dir.path()).unwrap();
    database
        .connection
        .busy_timeout(Duration::from_millis(10))
        .unwrap();
    let source = super::super::ConversationStore::in_memory();
    let record = source.create("Busy".to_owned()).unwrap();
    let blocker = Connection::open(dir.path().join(DATABASE_NAME)).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert_eq!(database.save(None, &record), Err(ConversationError::Busy));
    blocker.execute_batch("ROLLBACK").unwrap();
    assert!(database.load(&record.id).unwrap().is_none());
}
