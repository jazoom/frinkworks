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
