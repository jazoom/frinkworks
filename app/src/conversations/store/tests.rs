use crate::{
    agents::NetworkAccess,
    providers::{ModelSelection, ProviderKind},
    sessions::JobId,
};

use super::{
    ConversationError, ConversationStore, MAXIMUM_TITLE_BYTES, MessageStatus, TRANSCRIPT_WINDOW,
    TranscriptCursor,
};

impl ConversationStore {
    pub(crate) fn list(&self) -> Vec<super::ConversationRecord> {
        let database = self.database();
        database
            .metadata_all()
            .unwrap()
            .into_iter()
            .map(|record| database.load(&record.id).unwrap().unwrap())
            .collect()
    }

    fn create_record(
        &self,
        title: String,
        title_pending: bool,
    ) -> Result<super::ConversationRecord, ConversationError> {
        self.create_saved(
            super::ConversationId::generate().map_err(|_| ConversationError::Random)?,
            (!title_pending).then_some(title),
            None,
            Vec::new(),
        )
    }

    pub(crate) fn create_untitled(&self) -> Result<super::ConversationRecord, ConversationError> {
        self.create_record("New conversation".to_owned(), true)
    }

    pub(crate) fn create(
        &self,
        title: String,
    ) -> Result<super::ConversationRecord, ConversationError> {
        self.create_record(title, false)
    }

    pub(crate) fn begin_message(
        &self,
        id: &super::ConversationId,
        expected_revision: u32,
        selection: ModelSelection,
        request: JobId,
        text: String,
    ) -> Result<super::ConversationRecord, ConversationError> {
        let settings = crate::execution::ExecutionSettings::new(
            selection,
            String::new(),
            crate::agents::ToolId::ALL.to_vec(),
            crate::tests::test_environment_id(),
        )
        .unwrap();
        self.begin_message_with_model(
            id,
            expected_revision,
            Some(super::ConversationModelConfiguration {
                settings,
                preset: None,
            }),
            request,
            text,
        )
    }

    pub(crate) fn in_memory() -> Self {
        Self {
            database: std::sync::Mutex::new(super::sqlite::Database::in_memory().unwrap()),
            attachments: crate::conversations::attachments::AttachmentStore::in_memory(),
            uncertain: std::sync::Mutex::new(std::collections::BTreeSet::new()),
            title_updates: tokio::sync::broadcast::channel(16).0,
            questions: super::super::questions::QuestionWaiters::new(),
        }
    }
}

#[test]
fn compaction_survives_restart_with_a_later_pending_request() {
    let dir = tempfile::tempdir().unwrap();
    let store = ConversationStore::open(dir.path().to_path_buf()).unwrap();
    let mut record = store.create("Summary".to_owned()).unwrap();
    let selection = ModelSelection::new(ProviderKind::Xai, "model".to_owned(), None).unwrap();
    for text in ["First", "Retained"] {
        let job = JobId::generate().unwrap();
        record = store
            .begin_message(
                &record.id,
                record.revision,
                selection.clone(),
                job,
                text.to_owned(),
            )
            .unwrap();
        store
            .settle_message(
                &record.id,
                job,
                crate::providers::AssistantReply::from("Reply"),
                crate::conversations::MessageStatus::Complete,
                None,
            )
            .unwrap();
        record = store.get(&record.id).unwrap();
    }
    let original = record.messages.clone();
    let job = JobId::generate().unwrap();
    assert_eq!(
        store.begin_compaction(&record.id, record.revision - 1, job),
        Err(ConversationError::Conflict)
    );
    store
        .begin_compaction(&record.id, record.revision, job)
        .unwrap();
    let request = crate::conversations::RequestUsage {
        id: crate::conversations::RequestId::generate().unwrap(),
        usage: crate::providers::ModelUsage::new(ProviderKind::Xai, "model"),
        auth: crate::providers::AuthMethod::ApiKey,
        prices: None,
        sources: Vec::new(),
        advertised: Vec::new(),
    };
    store
        .record_summary_request(&record.id, job, &request)
        .unwrap();
    store
        .record_job_compaction(
            &record.id,
            job,
            crate::conversations::CompactionRecord {
                covered_through: original[1].id,
                retained_from: original[2].id,
                text: "Earlier context".to_owned(),
                requests: vec![request.clone()],
                preserve: None,
                created_at_ms: 1,
            },
        )
        .unwrap();
    let settled = store.finish_compaction(&record.id, job, None).unwrap();
    assert_eq!(settled.messages, original);
    let next = JobId::generate().unwrap();
    store
        .begin_message(
            &record.id,
            settled.revision,
            selection,
            next,
            "Next request".to_owned(),
        )
        .unwrap();
    drop(store);
    let reopened = ConversationStore::open(dir.path().to_path_buf()).unwrap();
    let loaded = reopened.get(&record.id).unwrap();
    assert_eq!(loaded.summary_requests, vec![request]);
    let projected = crate::conversations::compaction::project(
        &loaded.messages,
        None,
        loaded.compaction.as_ref(),
    )
    .unwrap();
    assert!(projected[0].text.ends_with("Earlier context"));
    assert_eq!(projected[1].text, "Retained");
    assert_eq!(projected.last().unwrap().text, "Next request");
    let context = reopened.context_record(&record.id).unwrap();
    assert!(
        !context
            .messages
            .iter()
            .any(|message| message.id == original[0].id)
    );
    assert_eq!(
        crate::conversations::compaction::project(
            &context.messages,
            None,
            context.compaction.as_ref(),
        )
        .unwrap(),
        projected
    );
}

#[test]
fn restart_preserves_explicit_saves_with_or_without_a_manual_title() {
    let dir = tempfile::tempdir().unwrap();
    let (draft, saved);
    {
        let store = ConversationStore::open(dir.path().to_path_buf()).unwrap();
        draft = store.create_untitled().unwrap();
        saved = store.create("New conversation".to_owned()).unwrap();
    }

    let reopened = ConversationStore::open(dir.path().to_path_buf()).unwrap();
    assert_eq!(reopened.get(&draft.id), Some(draft));
    assert_eq!(reopened.get(&saved.id), Some(saved));
    assert_eq!(reopened.list().len(), 2);
}

#[test]
fn restart_preserves_directory_identity_and_guest_alias() {
    let dir = tempfile::tempdir().unwrap();
    let granted = tempfile::tempdir().unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(granted.path(), &[]).unwrap();
    let settings = crate::execution::ExecutionSettings::new(
        ModelSelection::new(ProviderKind::Xai, "model".to_owned(), None).unwrap(),
        String::new(),
        Vec::new(),
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_directories(vec![grant.clone()])
    .unwrap();
    let store = ConversationStore::open(dir.path().to_path_buf()).unwrap();
    let record = store
        .create_saved(
            super::ConversationId::generate().unwrap(),
            Some("Directory".to_owned()),
            Some(super::ConversationModelConfiguration {
                settings,
                preset: None,
            }),
            Vec::new(),
        )
        .unwrap();
    drop(store);

    let reopened = ConversationStore::open(dir.path().to_path_buf()).unwrap();
    assert_eq!(
        reopened
            .get(&record.id)
            .unwrap()
            .model
            .unwrap()
            .settings
            .directories,
        vec![grant]
    );
}

#[test]
fn history_beyond_former_message_byte_and_catalogue_limits_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let mut ids = Vec::new();
    {
        let store = ConversationStore::open(dir.path().to_path_buf()).unwrap();
        for _ in 0..130 {
            let record = store
                .create_saved(
                    super::ConversationId::generate().unwrap(),
                    None,
                    None,
                    Vec::new(),
                )
                .unwrap();
            ids.push(record.id);
        }
        let previous = store.get(&ids[0]).unwrap();
        let mut record = previous.clone();
        for _ in 0..600 {
            for role in [super::MessageRole::User, super::MessageRole::Assistant] {
                let id = super::MessageId::generate().unwrap();
                record.messages.push(super::ConversationMessage {
                    parent: None,
                    id,
                    role,
                    response: (role == super::MessageRole::Assistant).then_some(id),
                    final_phase: role == super::MessageRole::Assistant,
                    text: if role == super::MessageRole::User {
                        "Question".to_owned()
                    } else {
                        "x".repeat(120 * 1024)
                    },
                    attachments: Vec::new(),
                    activity: Vec::new(),
                    continuation: Vec::new(),
                    status: MessageStatus::Complete,
                    error: None,
                    request: (role == super::MessageRole::Assistant)
                        .then(|| JobId::generate().unwrap()),
                    completion: None,
                    requests: Vec::new(),
                });
            }
        }
        let last_user = record.messages.len() - 2;
        record.messages[last_user].text = "late-search-sentinel".to_owned();
        super::project_active_path(&mut record);
        store
            .persist(&mut store.database(), Some(&previous), &record)
            .unwrap();
        let job = JobId::generate().unwrap();
        store
            .begin_compaction(&record.id, record.revision, job)
            .unwrap();
        for _ in 0..300 {
            let request = crate::conversations::RequestUsage {
                id: crate::conversations::RequestId::generate().unwrap(),
                usage: crate::providers::ModelUsage::new(ProviderKind::Xai, "model"),
                auth: crate::providers::AuthMethod::ApiKey,
                prices: None,
                sources: Vec::new(),
                advertised: Vec::new(),
            };
            store
                .record_summary_request(&record.id, job, &request)
                .unwrap();
        }
        store.finish_compaction(&record.id, job, None).unwrap();
    }
    let reopened = ConversationStore::open(dir.path().to_path_buf()).unwrap();
    assert_eq!(reopened.metadata().len(), 130);
    assert!(ids.iter().all(|id| reopened.metadata_for(id).is_some()));
    let record = reopened.get(&ids[0]).unwrap();
    assert_eq!(record.messages.len(), 1200);
    assert_eq!(record.summary_requests.len(), 300);
    let first = reopened
        .tree_window(&record.id, None, Some("late-search-sentinel"), None)
        .unwrap()
        .unwrap();
    assert!(first.entries.is_empty());
    assert!(first.partial);
    let next = reopened
        .tree_window(
            &record.id,
            first.next_cursor,
            Some("late-search-sentinel"),
            None,
        )
        .unwrap()
        .unwrap();
    assert_eq!(next.entries.len(), 1);
    assert_eq!(next.entries[0].id, record.messages[1198].id);
    assert!(!next.has_more);
    assert!(
        record
            .messages
            .iter()
            .skip(1)
            .step_by(2)
            .all(|message| message.text == "x".repeat(120 * 1024))
    );
}

#[test]
fn append_and_failed_attempts_preserve_existing_rows_and_append_identity() {
    let dir = tempfile::tempdir().unwrap();
    let store = ConversationStore::open(dir.path().to_path_buf()).unwrap();
    let record = store.create("History".to_owned()).unwrap();
    let job = JobId::generate().unwrap();
    store
        .begin_message_with_model(&record.id, record.revision, None, job, "First".to_owned())
        .unwrap();
    store
        .settle_message(&record.id, job, "Reply", MessageStatus::Complete, None)
        .unwrap();
    let record = store.get(&record.id).unwrap();
    let connection = rusqlite::Connection::open(dir.path().join("conversations.sqlite3")).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER protect_update BEFORE UPDATE ON messages WHEN OLD.sequence < 2
         BEGIN SELECT RAISE(ABORT, 'old message changed'); END;
         CREATE TRIGGER protect_delete BEFORE DELETE ON messages WHEN OLD.sequence < 2
         BEGIN SELECT RAISE(ABORT, 'old message deleted'); END;",
        )
        .unwrap();
    let job = JobId::generate().unwrap();
    let started = store
        .begin_message_with_model(&record.id, record.revision, None, job, "Next".to_owned())
        .unwrap();
    let pending = started.messages.last().unwrap().id.as_hex();
    let sequence = || {
        connection
            .query_row(
                "SELECT sequence FROM messages WHERE id = ?1",
                [&pending],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
    };
    let original_sequence = sequence();
    let mut reply = crate::providers::AssistantReply::default();
    reply.push_response("Progress");
    reply.start_tool(
        "unique-call".to_owned(),
        "list".to_owned(),
        serde_json::json!({}),
    );
    store.checkpoint_output(&record.id, job, reply).unwrap();
    store
        .record_provider_failure(
            &record.id,
            job,
            "Failed request".into(),
            &"Progress".into(),
            "Provider failure".to_owned(),
        )
        .unwrap();
    assert_eq!(sequence(), original_sequence);
    store
        .checkpoint_output(&record.id, job, "More progress")
        .unwrap();
    store
        .settle_message(&record.id, job, "Final", MessageStatus::Complete, None)
        .unwrap();
    let final_record = store.get(&record.id).unwrap();
    assert_eq!(&final_record.messages[..2], record.messages.as_slice());
    assert_eq!(final_record.messages[3].status, MessageStatus::Failed);
    assert_eq!(final_record.messages[4].text, "Final");
}

#[test]
fn automatic_titles_preserve_manual_edits_and_command_revisions() {
    let store = ConversationStore::in_memory();
    let record = store.create_untitled().unwrap();
    let job = JobId::generate().unwrap();
    let started = store
        .begin_message(
            &record.id,
            record.revision,
            ModelSelection::new(ProviderKind::Deepseek, "deepseek-v4-flash".to_owned(), None)
                .unwrap(),
            job,
            "Fix the parser".to_owned(),
        )
        .unwrap();
    assert_eq!(started.title, "New conversation");
    assert!(store.claim_title(&record.id).is_none());
    store
        .settle_message(
            &record.id,
            job,
            String::new(),
            MessageStatus::Failed,
            Some("The request failed.".to_owned()),
        )
        .unwrap();
    assert!(store.claim_title(&record.id).is_none());
    let record = store.get(&record.id).unwrap();
    assert_eq!(record.title, "New conversation");
    let job = JobId::generate().unwrap();
    store
        .begin_message_with_model(
            &record.id,
            record.revision,
            None,
            job,
            "Retry the parser repair".to_owned(),
        )
        .unwrap();
    store
        .settle_message(
            &record.id,
            job,
            "Done".to_owned(),
            MessageStatus::Complete,
            None,
        )
        .unwrap();
    let mut updates = store.subscribe_titles();
    let claimed = store.claim_title(&record.id).unwrap();
    assert_eq!(claimed.title, "New conversation");
    assert_eq!(store.get(&record.id).unwrap().title, "New conversation");
    assert!(updates.try_recv().is_err());
    assert_eq!(
        super::super::titles::exchange(&claimed),
        Some(("Retry the parser repair", "Done"))
    );
    assert!(store.claim_title(&record.id).is_none());
    store
        .save_automatic_title(&record.id, claimed.revision, "Parser repair".to_owned())
        .unwrap();
    assert_eq!(store.get(&record.id).unwrap().revision, claimed.revision);
    assert_eq!(store.get(&record.id).unwrap().title, "Parser repair");
    assert!(updates.try_recv().is_ok());
    assert!(updates.try_recv().is_err());
    let renamed = store
        .rename(&record.id, claimed.revision, "My title".to_owned())
        .unwrap();
    assert_eq!(
        store.save_automatic_title(&record.id, claimed.revision, "Late title".to_owned()),
        Err(ConversationError::Conflict)
    );
    assert_eq!(store.get(&record.id).unwrap().title, renamed.title);

    let record = store.create_untitled().unwrap();
    let record = store
        .rename(&record.id, record.revision, "New conversation".to_owned())
        .unwrap();
    assert!(!record.title_pending);
    assert!(store.claim_title(&record.id).is_none());
}

#[test]
fn workflow_reservations_leave_the_normal_model_selection_unchanged() {
    let store = ConversationStore::in_memory();
    for model in [
        None,
        Some(super::ConversationModelConfiguration::direct(
            ModelSelection::new(
                crate::providers::ProviderKind::Xai,
                "grok-4.6".to_owned(),
                None,
            )
            .expect("model"),
            crate::tests::test_environment_id(),
        )),
    ] {
        let mut conversation = store.create("Workflow".to_owned()).expect("conversation");
        if let Some(model) = model.clone() {
            conversation = store
                .update_execution_settings(&conversation.id, conversation.revision, model.settings)
                .expect("selection");
        }
        let started = store
            .begin_message_with_model(
                &conversation.id,
                conversation.revision,
                None,
                JobId::generate().expect("job"),
                "Brief".to_owned(),
            )
            .expect("reserve");
        assert_eq!(started.model, model);
    }
}

#[test]
fn model_changes_preserve_execution_authority_and_reject_stale_revisions() {
    let store = ConversationStore::in_memory();
    let record = store.create("Model controls".to_owned()).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(directory.path(), &[]).unwrap();
    let mut settings = crate::execution::ExecutionSettings::new(
        ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None).unwrap(),
        "Keep my instructions".to_owned(),
        vec![crate::agents::ToolId::Run],
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_directories(vec![grant.clone()])
    .unwrap()
    .with_location(crate::execution::ToolLocation::Host)
    .with_host_approval(crate::execution::HostApprovalPolicy::Automatic);
    let record = store
        .update_execution_settings(&record.id, record.revision, settings.clone())
        .unwrap();
    let approval = crate::conversations::DirectoryApproval::for_grant(&settings, &grant);
    let record = store
        .record_directory_approval(&record.id, record.revision, approval)
        .unwrap();
    let selection =
        ModelSelection::new(ProviderKind::Deepseek, "deepseek-chat".to_owned(), None).unwrap();
    let updated = store
        .select_model(
            &record.id,
            record.revision,
            selection.clone(),
            crate::tests::test_environment_id(),
        )
        .unwrap();
    settings.model = selection.clone();
    assert_eq!(updated.model.as_ref().unwrap().settings, settings);
    assert_eq!(updated.directory_approvals, record.directory_approvals);
    assert_eq!(
        store.select_model(
            &record.id,
            record.revision,
            selection,
            crate::tests::test_environment_id()
        ),
        Err(ConversationError::Conflict)
    );
}

#[test]
fn distinct_opaque_conversations_survive_a_restart() {
    let dir = tempfile::tempdir().expect("directory");
    let (first, second);
    {
        let store = ConversationStore::open(dir.path().to_path_buf()).expect("store");
        first = store.create("First discussion".to_owned()).expect("first");
        second = store
            .create("Second discussion".to_owned())
            .expect("second");
        assert_ne!(first.id, second.id);
        assert_eq!(first.revision, 1);
        assert_eq!(second.revision, 1);
    }
    let store = ConversationStore::open(dir.path().to_path_buf()).expect("reopen");
    assert_eq!(store.get(&first.id), Some(first));
    assert_eq!(store.get(&second.id), Some(second));
}

#[test]
fn private_catalogue_path_rejects_a_symlink_without_replacement() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("root");
        let target = root.path().join("target");
        let path = root.path().join("conversations");
        std::fs::create_dir(&target).expect("target");
        symlink(&target, &path).expect("link");

        assert_eq!(
            ConversationStore::open(path).err(),
            Some(ConversationError::Persist)
        );
        assert!(target.read_dir().expect("target contents").next().is_none());
    }
}

#[test]
fn legacy_records_fail_closed_without_replacement() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join(format!("{}.json", "0".repeat(32)));
    std::fs::write(&path, "{").expect("record");
    let original = std::fs::read(&path).expect("original");

    assert_eq!(
        ConversationStore::open(dir.path().to_path_buf()).err(),
        Some(ConversationError::Corrupt)
    );
    assert_eq!(std::fs::read(path).expect("unchanged"), original);
}

#[test]
fn stale_revisions_do_not_replace_current_records() {
    let store = ConversationStore::in_memory();
    let created = store.create("Initial".to_owned()).expect("create");
    let renamed = store
        .rename(&created.id, created.revision, "Current".to_owned())
        .expect("rename");

    assert_eq!(
        store.rename(&created.id, created.revision, "Stale".to_owned()),
        Err(ConversationError::Conflict)
    );
    assert_eq!(
        store.delete(&created.id, created.revision),
        Err(ConversationError::Conflict)
    );
    assert_eq!(store.get(&created.id), Some(renamed));
}

#[test]
fn titles_are_bounded() {
    let store = ConversationStore::in_memory();
    for title in [
        String::new(),
        "   ".to_owned(),
        "a\0b".to_owned(),
        "é".repeat(MAXIMUM_TITLE_BYTES / 2 + 1),
    ] {
        assert_eq!(store.create(title).err(), Some(ConversationError::Title));
    }
}

#[test]
fn rename_preserves_timestamp_order_after_clock_regression() {
    let dir = tempfile::tempdir().expect("directory");
    let store = ConversationStore::open(dir.path().to_path_buf()).expect("store");
    let record = store.create("Original".to_owned()).expect("create");
    let renamed = store
        .rename(&record.id, record.revision, "Renamed".to_owned())
        .expect("rename");
    assert!(renamed.updated_at_ms >= record.updated_at_ms);
    drop(store);
    let reopened = ConversationStore::open(dir.path().to_path_buf()).expect("reopen");
    assert_eq!(reopened.get(&record.id), Some(renamed));
}

#[test]
fn conversation_network_access_survives_restart() {
    let dir = tempfile::tempdir().expect("directory");
    let store = ConversationStore::open(dir.path().to_path_buf()).expect("store");
    let record = store.create("Discussion".to_owned()).expect("conversation");
    let updated = store
        .set_network(
            &record.id,
            record.revision,
            NetworkAccess::Restricted(vec!["example.com".to_owned()]),
        )
        .expect("network");
    drop(store);

    let store = ConversationStore::open(dir.path().to_path_buf()).expect("reopen");
    assert_eq!(
        store.get(&record.id).expect("record").network,
        updated.network
    );
}

#[test]
fn conversation_network_access_rejects_stale_and_active_changes() {
    let store = ConversationStore::in_memory();
    let record = store.create("Discussion".to_owned()).expect("conversation");
    let updated = store
        .set_network(&record.id, record.revision, NetworkAccess::Public)
        .expect("network");
    assert_eq!(
        store.set_network(&record.id, record.revision, NetworkAccess::None),
        Err(ConversationError::Conflict)
    );

    let request = JobId::generate().expect("request");
    let selection =
        ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None).expect("selection");
    store
        .begin_message(
            &record.id,
            updated.revision,
            selection,
            request,
            "Question".to_owned(),
        )
        .expect("active message");
    assert_eq!(
        store.set_network(&record.id, updated.revision + 1, NetworkAccess::None),
        Err(ConversationError::Active)
    );
}

#[test]
fn active_request_rejects_stale_settlement() {
    let store = ConversationStore::in_memory();
    let record = store.create("Discussion".to_owned()).expect("conversation");
    let request = JobId::generate().expect("request");
    let selection =
        ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None).expect("selection");
    store
        .begin_message(
            &record.id,
            record.revision,
            selection,
            request,
            "Question".to_owned(),
        )
        .expect("begin");

    assert_eq!(
        store.settle_message(
            &record.id,
            JobId::generate().expect("stale request"),
            "Wrong reply".to_owned(),
            MessageStatus::Complete,
            None,
        ),
        Err(ConversationError::Conflict)
    );
    let current = store.get(&record.id).expect("current");
    assert_eq!(current.active_job, Some(request));
    assert!(current.messages.last().expect("assistant").text.is_empty());
}

#[test]
fn append_output_leaves_other_records_and_control_revisions_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let store = ConversationStore::open(dir.path().to_path_buf()).unwrap();
    let other = store.create("Other".to_owned()).unwrap();
    let record = store.create("Active".to_owned()).unwrap();
    let job = JobId::generate().unwrap();
    let record = store
        .begin_message_with_model(
            &record.id,
            record.revision,
            None,
            job,
            "Question".to_owned(),
        )
        .unwrap();
    store.append_output(&record.id, job, "Partial").unwrap();
    let current = store.get(&record.id).unwrap();
    assert_eq!(current.revision, record.revision);
    assert_eq!(current.messages.last().unwrap().text, "Partial");
    assert_eq!(store.get(&other.id).unwrap(), other);
    drop(store);
    let reopened = ConversationStore::open(dir.path().to_path_buf()).unwrap();
    let recovered = reopened.get(&record.id).unwrap();
    assert_eq!(recovered.active_job, None);
    assert_eq!(recovered.messages.last().unwrap().text, "Partial");
}

#[test]
fn uncertain_replacement_blocks_settlement_and_new_work() {
    let store = ConversationStore::in_memory();
    let record = store.create("Uncertain".to_owned()).unwrap();
    let job = JobId::generate().unwrap();
    let record = store
        .begin_message_with_model(
            &record.id,
            record.revision,
            None,
            job,
            "Question".to_owned(),
        )
        .unwrap();
    store.uncertain.lock().unwrap().insert(record.id);
    assert_eq!(
        store.settle_message(&record.id, job, "Reply", MessageStatus::Complete, None),
        Err(ConversationError::Unsettled)
    );
    assert_eq!(
        store.rename(&record.id, record.revision, "Changed".to_owned()),
        Err(ConversationError::Unsettled)
    );
    assert_eq!(store.get(&record.id).unwrap(), record);
}

#[test]
fn completed_and_interrupted_messages_survive_restart() {
    let dir = tempfile::tempdir().expect("directory");
    let store = ConversationStore::open(dir.path().to_path_buf()).expect("store");
    let mut record = store.create("Discussion".to_owned()).expect("conversation");
    let selection =
        ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None).expect("model");
    for complete in [true, false] {
        let request = JobId::generate().expect("request");
        record = store
            .begin_message(
                &record.id,
                record.revision,
                selection.clone(),
                request,
                "First line\n\tSecond line".to_owned(),
            )
            .expect("multiline message");
        assert_eq!(
            store.delete(&record.id, record.revision),
            Err(ConversationError::Active)
        );
        let mut reply = crate::providers::AssistantReply::default();
        reply.push_response("Partial");
        reply.push_thinking("");
        reply.start_tool(
            "call-1".to_owned(),
            "list".to_owned(),
            serde_json::json!({}),
        );
        reply.finish_tool(
            "call-1",
            crate::providers::ToolOutput {
                resource: None,
                label: "list".to_owned(),
                output: "Empty workspace".to_owned(),
                command: None,
            },
        );
        reply.push_response(" reply");
        store
            .append_output(&record.id, request, reply)
            .expect("partial");
        if complete {
            store
                .settle_message(
                    &record.id,
                    request,
                    "Complete reply".to_owned(),
                    MessageStatus::Complete,
                    None,
                )
                .expect("settle");
        }
        record = store.get(&record.id).expect("current");
    }
    drop(store);
    let store = ConversationStore::open(dir.path().to_path_buf()).expect("restart");
    let recovered = store.get(&record.id).expect("recovered");
    assert_eq!(recovered.active_job, None);
    assert_eq!(recovered.messages[1].status, MessageStatus::Complete);
    assert_eq!(recovered.messages[1].text, "Complete reply");
    assert_eq!(recovered.messages[3].status, MessageStatus::Interrupted);
    assert_eq!(recovered.messages[3].text, "Partial reply");
    use crate::providers::AssistantActivity;
    assert!(
        matches!(recovered.messages[3].activity.as_slice(), [AssistantActivity::Response(_), AssistantActivity::Thinking(text), AssistantActivity::ToolCall { result: Some(_), .. }, AssistantActivity::Response(_)] if text.is_empty())
    );
    assert_eq!(
        store.settle_message(
            &record.id,
            record.active_job.expect("old request"),
            "Stale".to_owned(),
            MessageStatus::Complete,
            None,
        ),
        Err(ConversationError::Conflict)
    );
}

#[test]
fn command_outcomes_survive_restart_with_stream_identity() {
    use crate::execution::command::{CommandChunk, CommandStream, CommandTermination};
    let dir = tempfile::tempdir().expect("directory");
    let store = ConversationStore::open(dir.path().to_path_buf()).expect("store");
    let record = store.create("Commands".to_owned()).expect("conversation");
    let selection =
        ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None).expect("model");
    let request = JobId::generate().expect("request");
    let record = store
        .begin_message(
            &record.id,
            record.revision,
            selection,
            request,
            "Run it".to_owned(),
        )
        .expect("start");
    let mut reply = crate::providers::AssistantReply::default();
    reply.push_tool(crate::providers::ToolOutput {
        resource: None,
        label: "run `exit 3`".to_owned(),
        output: "The command exited with code 3.".to_owned(),
        command: Some(crate::execution::CommandResult::new(
            vec![
                CommandChunk {
                    stream: CommandStream::Stdout,
                    text: "out".to_owned(),
                },
                CommandChunk {
                    stream: CommandStream::Stderr,
                    text: "err".to_owned(),
                },
            ],
            CommandTermination::Exited(3),
        )),
    });
    store
        .settle_message(&record.id, request, reply, MessageStatus::Complete, None)
        .expect("settle");
    drop(store);
    let store = ConversationStore::open(dir.path().to_path_buf()).expect("restart");
    let recovered = store.get(&record.id).expect("recovered");
    let command = recovered.messages[1]
        .activity
        .iter()
        .find_map(|activity| match activity {
            crate::providers::AssistantActivity::Tool(tool) => tool.command.clone(),
            _ => None,
        })
        .expect("command outcome");
    assert_eq!(command.exit_code(), Some(3));
    assert_eq!(command.chunks[0].stream, CommandStream::Stdout);
    assert_eq!(command.chunks[1].stream, CommandStream::Stderr);
}

#[test]
fn transcript_progress_preserves_revision_without_weakening_request_identity() {
    let store = ConversationStore::in_memory();
    let record = store.create("Discussion".to_owned()).unwrap();
    let request = JobId::generate().unwrap();
    let started = store
        .begin_message_with_model(
            &record.id,
            record.revision,
            None,
            request,
            "Question".to_owned(),
        )
        .unwrap();
    for text in ["Partial", "Partial", "Complete"] {
        store.append_output(&record.id, request, text).unwrap();
        let current = store.get(&record.id).unwrap();
        assert_eq!(current.revision, started.revision);
        assert_eq!(current.messages.last().unwrap().text, text);
    }
    assert_eq!(
        store.append_output(&record.id, JobId::generate().unwrap(), "Wrong request"),
        Err(ConversationError::Conflict)
    );
    store
        .settle_message(
            &record.id,
            request,
            "Complete",
            MessageStatus::Complete,
            None,
        )
        .unwrap();
    assert_eq!(
        store.get(&record.id).unwrap().revision,
        started.revision + 1
    );
    assert_eq!(
        store.append_output(&record.id, request, "Late output"),
        Err(ConversationError::Conflict)
    );
}

#[test]
fn message_bounds_apply_before_persistence() {
    let store = ConversationStore::in_memory();
    let record = store.create("Discussion".to_owned()).expect("conversation");
    let selection =
        ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None).expect("model");
    let request = JobId::generate().expect("request");
    for text in [
        "x".repeat(super::MAXIMUM_MESSAGE_BYTES + 1),
        "invalid\0text".to_owned(),
    ] {
        assert_eq!(
            store.begin_message(
                &record.id,
                record.revision,
                selection.clone(),
                request,
                text
            ),
            Err(ConversationError::Message)
        );
    }
    let other = store.create("Other".to_owned()).expect("other");
    let other_request = JobId::generate().expect("request");
    store
        .begin_message(
            &other.id,
            other.revision,
            selection.clone(),
            other_request,
            "Review".to_owned(),
        )
        .expect("capacity for a linked review at a gate");
    store
        .begin_message(
            &record.id,
            record.revision,
            selection.clone(),
            request,
            "Question".to_owned(),
        )
        .expect("begin");
    assert_eq!(
        store.append_output(
            &record.id,
            request,
            "x".repeat(super::MAXIMUM_REPLY_BYTES + 1)
        ),
        Err(ConversationError::Message)
    );
    let mut reply = crate::providers::AssistantReply::default();
    reply.push_response(&"\u{0001}".repeat(super::MAXIMUM_REPLY_BYTES));
    reply.push_thinking(&"\u{0001}".repeat(super::MAXIMUM_REPLY_BYTES));
    for activity in [
        vec![crate::providers::AssistantActivity::Thinking(
            "invalid\0text".to_owned(),
        )],
        vec![crate::providers::AssistantActivity::Thinking(
            "x".repeat(256 * 1024 + 1),
        )],
        vec![crate::providers::AssistantActivity::Thinking(String::new()); 257],
        vec![crate::providers::AssistantActivity::Response(
            "Different text".to_owned(),
        )],
    ] {
        let invalid = crate::providers::AssistantReply {
            activity,
            ..Default::default()
        };
        assert_eq!(
            store.append_output(&record.id, request, invalid),
            Err(ConversationError::Message)
        );
    }
    store
        .append_output(&record.id, request, reply.clone())
        .expect("reserved capacity");
    store
        .settle_message(
            &record.id,
            request,
            reply.clone(),
            MessageStatus::Interrupted,
            None,
        )
        .expect("terminal capacity");
    store
        .settle_message(
            &other.id,
            other_request,
            reply,
            MessageStatus::Complete,
            None,
        )
        .expect("linked review terminal capacity");
}

#[test]
fn invalid_message_errors_do_not_settle_the_request() {
    let store = ConversationStore::in_memory();
    let record = store.create_untitled().unwrap();
    let request = JobId::generate().unwrap();
    let started = store
        .begin_message_with_model(
            &record.id,
            record.revision,
            None,
            request,
            "Question".to_owned(),
        )
        .unwrap();
    for (status, error) in [
        (MessageStatus::Complete, "Unexpected error".to_owned()),
        (MessageStatus::Failed, String::new()),
        (MessageStatus::Failed, "   ".to_owned()),
        (MessageStatus::Failed, "Bad\nerror".to_owned()),
        (
            MessageStatus::Failed,
            "界".repeat(crate::providers::MAXIMUM_PROVIDER_DETAIL_BYTES / 3 + 1),
        ),
    ] {
        assert_eq!(
            store.settle_message(&record.id, request, String::new(), status, Some(error)),
            Err(ConversationError::Message)
        );
        assert_eq!(store.get(&record.id).unwrap(), started);
    }
}

#[test]
fn corrupted_stored_metadata_fails_closed_on_load() {
    let dir = tempfile::tempdir().expect("directory");
    let record;
    {
        let store = ConversationStore::open(dir.path().to_path_buf()).expect("store");
        record = store.create("Original".to_owned()).expect("create");
    }
    {
        let connection =
            rusqlite::Connection::open(dir.path().join("conversations.sqlite3")).expect("db");
        connection
            .execute(
                "UPDATE conversations SET metadata = '{' WHERE id = ?1",
                [record.id.as_hex()],
            )
            .expect("tamper");
    }
    let store = ConversationStore::open(dir.path().to_path_buf()).expect("reopen");
    assert!(store.get(&record.id).is_none());
    assert!(store.metadata_for(&record.id).is_none());
}

#[test]
fn restart_preserves_directory_approvals_until_settings_change() {
    let dir = tempfile::tempdir().unwrap();
    let granted = tempfile::tempdir().unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(granted.path(), &[]).unwrap();
    let settings = crate::execution::ExecutionSettings::new(
        ModelSelection::new(ProviderKind::Xai, "model".to_owned(), None).unwrap(),
        String::new(),
        Vec::new(),
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_directories(vec![grant.clone()])
    .unwrap();
    let store = ConversationStore::open(dir.path().to_path_buf()).unwrap();
    let record = store
        .create_saved(
            super::ConversationId::generate().unwrap(),
            Some("Directory".to_owned()),
            Some(super::ConversationModelConfiguration {
                settings: settings.clone(),
                preset: None,
            }),
            vec![super::DirectoryApproval::for_grant(&settings, &grant)],
        )
        .unwrap();
    drop(store);

    let reopened = ConversationStore::open(dir.path().to_path_buf()).unwrap();
    assert!(reopened.directory_approved(&record.id, &settings, &grant));

    let mut changed_grant = grant.clone();
    changed_grant.access = crate::execution::DirectoryAccess::DirectWrite;
    let changed_settings = crate::execution::ExecutionSettings::new(
        ModelSelection::new(ProviderKind::Xai, "model".to_owned(), None).unwrap(),
        String::new(),
        Vec::new(),
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_directories(vec![changed_grant.clone()])
    .unwrap();
    assert!(!reopened.directory_approved(&record.id, &changed_settings, &changed_grant));
    assert!(!reopened.directory_approved(&record.id, &settings, &changed_grant));

    let other = tempfile::tempdir().unwrap();
    let other_grant = crate::execution::DirectoryGrant::from_selected(other.path(), &[]).unwrap();
    assert!(!reopened.directory_approved(&record.id, &settings, &other_grant));

    let changed = reopened
        .update_execution_settings(&record.id, record.revision, changed_settings)
        .unwrap();
    let restored = reopened
        .update_execution_settings(&record.id, changed.revision, settings.clone())
        .unwrap();
    assert!(restored.directory_approvals.is_empty());
    drop(reopened);
    let reopened = ConversationStore::open(dir.path().to_path_buf()).unwrap();
    assert!(!reopened.directory_approved(&record.id, &settings, &grant));

    let approved = reopened
        .record_directory_approval(
            &record.id,
            restored.revision,
            super::DirectoryApproval::for_grant(&settings, &grant),
        )
        .unwrap();
    let preset = crate::presets::PresetRecord {
        id: crate::presets::PresetId::generate().unwrap(),
        revision: 1,
        name: "Same access".to_owned(),
        settings: settings.clone(),
        provenance: crate::presets::PresetProvenance::Draft,
        created_at_ms: 1,
    };
    let replaced = reopened
        .apply_preset(&record.id, approved.revision, &preset)
        .unwrap();
    assert!(replaced.directory_approvals.is_empty());
    drop(reopened);
    let reopened = ConversationStore::open(dir.path().to_path_buf()).unwrap();
    assert!(!reopened.directory_approved(&record.id, &settings, &grant));
}

#[test]
fn message_identity_survives_restart_across_independent_conversations() {
    let dir = tempfile::tempdir().expect("directory");
    let store = ConversationStore::open(dir.path().to_path_buf()).expect("store");
    let selection =
        ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None).expect("model");
    let mut records = Vec::new();
    let mut requests = Vec::new();
    for index in 0..2 {
        let record = store
            .create(format!("Conversation {index}"))
            .expect("record");
        let request = JobId::generate().expect("request");
        store
            .begin_message(
                &record.id,
                record.revision,
                selection.clone(),
                request,
                format!("Question {index}"),
            )
            .expect("begin");
        store
            .append_output(&record.id, request, format!("Partial {index}"))
            .expect("partial");
        records.push(store.get(&record.id).expect("started"));
        requests.push(request);
    }
    drop(store);
    let store = ConversationStore::open(dir.path().to_path_buf()).expect("reopen");
    for (record, request) in records.iter().zip(&requests) {
        let recovered = store.get(&record.id).expect("recovered");
        assert_eq!(recovered.active_job, None);
        assert_eq!(recovered.messages[0].id, record.messages[0].id);
        assert_eq!(recovered.messages[1].id, record.messages[1].id);
        let assistant = recovered.messages.last().expect("assistant");
        assert_eq!(assistant.request, Some(*request));
        assert_eq!(assistant.status, MessageStatus::Interrupted);
    }
    assert_ne!(
        store.get(&records[0].id).unwrap().messages[0].id,
        store.get(&records[1].id).unwrap().messages[0].id
    );
}

#[test]
fn a_fork_record_survives_restart_with_its_provenance() {
    let dir = tempfile::tempdir().expect("directory");
    let store = ConversationStore::open(dir.path().to_path_buf()).expect("store");
    let source = store.create("Source".to_owned()).expect("source");
    let selection = ModelSelection::new(ProviderKind::Xai, "model".to_owned(), None).unwrap();
    let job = JobId::generate().unwrap();
    store
        .begin_message(
            &source.id,
            source.revision,
            selection,
            job,
            "Question".to_owned(),
        )
        .unwrap();
    store
        .settle_message(
            &source.id,
            job,
            crate::providers::AssistantReply::from("Answer"),
            MessageStatus::Complete,
            None,
        )
        .unwrap();
    let mut source = store.get(&source.id).unwrap();
    let candidate = crate::workflows::artefacts::ArtefactReference {
        id: crate::workflows::ArtefactId::generate().unwrap(),
        kind: crate::workflows::definition::ArtefactKind::CandidateRevision,
        artefact_hash: crate::workflows::artefacts::ArtefactHash::of(b"candidate", b"content"),
    };
    source.candidate_review_context = Some(super::CandidateReviewContext {
        source: super::CandidateReviewLink {
            conversation_id: Some(source.id),
            run_id: crate::workflows::RunId::generate().unwrap(),
            candidate: candidate.clone(),
            diff_base: candidate,
        },
        task_brief: "Review immutable evidence".to_owned(),
    });
    let boundary = source.messages.last().unwrap().id;
    let snapshot = crate::conversations::forks::snapshot(&source, boundary).expect("snapshot");
    let outputs = crate::execution::OutputStore::ephemeral();
    let destination = crate::conversations::ConversationId::generate().unwrap();
    let messages = crate::conversations::forks::materialise(&snapshot, destination, &outputs)
        .expect("materialise");
    let forked = store
        .create_fork(
            destination,
            Some("Fork".to_owned()),
            None,
            Vec::new(),
            messages,
            &snapshot,
        )
        .expect("fork");
    assert_eq!(forked.messages.len(), 2);
    drop(store);
    let reopened = ConversationStore::open(dir.path().to_path_buf()).expect("reopened");
    let loaded = reopened.get(&destination).expect("loaded");
    assert_eq!(
        loaded.forked_from.as_ref().map(|item| item.source),
        Some(source.id)
    );
    assert_eq!(
        loaded.forked_from.as_ref().map(|item| item.boundary),
        Some(boundary)
    );
    assert!(loaded.forked_from.as_ref().unwrap().candidate_review);
    assert_eq!(
        loaded.candidate_review_context,
        source.candidate_review_context
    );
}

fn seed_exchanges(
    store: &ConversationStore,
    id: &super::ConversationId,
    selection: ModelSelection,
    count: usize,
) -> super::ConversationRecord {
    let mut record = store.get(id).expect("record");
    for index in 0..count {
        let job = JobId::generate().unwrap();
        record = store
            .begin_message(
                &record.id,
                record.revision,
                selection.clone(),
                job,
                format!("Question {index}"),
            )
            .unwrap();
        store
            .settle_message(
                &record.id,
                job,
                format!("Reply {index}"),
                MessageStatus::Complete,
                None,
            )
            .unwrap();
        record = store.get(&record.id).unwrap();
    }
    record
}

#[test]
fn transcript_windows_bind_to_one_conversation_and_follow_append_order() {
    let store = ConversationStore::in_memory();
    let selection = ModelSelection::new(ProviderKind::Xai, "model".to_owned(), None).unwrap();
    let first = store.create("First".to_owned()).unwrap();
    let second = store.create("Second".to_owned()).unwrap();
    let first = seed_exchanges(&store, &first.id, selection.clone(), 70);
    assert_eq!(
        store.transcript_window(
            &second.id,
            Some(TranscriptCursor::Around(first.messages[0].id)),
            None
        ),
        Err(ConversationError::Entry)
    );
    let second = seed_exchanges(&store, &second.id, selection.clone(), 2);

    let (_, latest) = store
        .transcript_window(&first.id, None, None)
        .unwrap()
        .unwrap();
    assert_eq!(latest.messages.len(), TRANSCRIPT_WINDOW);
    assert_eq!(latest.total, first.messages.len());
    assert!(latest.has_before);
    assert!(!latest.has_after);
    assert_eq!(
        latest.messages.last().unwrap().id,
        first.messages.last().unwrap().id
    );

    let anchor = latest.before_anchor.expect("earlier anchor");
    let (_, earlier) = store
        .transcript_window(&first.id, Some(TranscriptCursor::Before(anchor)), None)
        .unwrap()
        .unwrap();
    assert!(earlier.has_after);
    assert!(earlier.messages.len() <= TRANSCRIPT_WINDOW);
    let earlier_ids: Vec<_> = earlier.messages.iter().map(|message| message.id).collect();

    // A later append never changes an existing window's progress or content.
    let first = seed_exchanges(&store, &first.id, selection, 1);
    let (_, repeated) = store
        .transcript_window(&first.id, Some(TranscriptCursor::Before(anchor)), None)
        .unwrap()
        .unwrap();
    assert_eq!(
        repeated
            .messages
            .iter()
            .map(|message| message.id)
            .collect::<Vec<_>>(),
        earlier_ids
    );

    // A cursor from another conversation is a foreign entry, not a position.
    let foreign = second.messages[0].id;
    assert_eq!(
        store.transcript_window(&first.id, Some(TranscriptCursor::Around(foreign)), None),
        Err(ConversationError::Entry)
    );
}

#[test]
fn transcript_windows_support_before_after_and_around_modes() {
    let store = ConversationStore::in_memory();
    let selection = ModelSelection::new(ProviderKind::Xai, "model".to_owned(), None).unwrap();
    let record = store.create("Modes".to_owned()).unwrap();
    let record = seed_exchanges(&store, &record.id, selection, 40);
    let middle = record.messages[record.messages.len() / 2].id;

    let (_, around) = store
        .transcript_window(&record.id, Some(TranscriptCursor::Around(middle)), None)
        .unwrap()
        .unwrap();
    assert_eq!(around.messages.len(), 1);
    assert_eq!(around.messages[0].id, middle);
    assert!(around.has_before && around.has_after);

    let (_, after) = store
        .transcript_window(&record.id, Some(TranscriptCursor::After(middle)), None)
        .unwrap()
        .unwrap();
    assert!(!after.messages.iter().any(|message| message.id == middle));
    assert!(after.has_before);
    let middle_index = record.messages.len() / 2;
    assert_eq!(after.messages, record.messages[middle_index + 1..]);

    let (_, before) = store
        .transcript_window(&record.id, Some(TranscriptCursor::Before(middle)), None)
        .unwrap()
        .unwrap();
    assert!(!before.messages.iter().any(|message| message.id == middle));
    assert!(before.has_after);
}

#[test]
fn active_path_parent_chain_survives_restart() {
    let dir = tempfile::tempdir().expect("directory");
    let record;
    {
        let store = ConversationStore::open(dir.path().to_path_buf()).expect("store");
        let selection = ModelSelection::new(ProviderKind::Xai, "model".to_owned(), None).unwrap();
        let created = store.create("Chain".to_owned()).expect("create");
        record = seed_exchanges(&store, &created.id, selection, 3);
    }
    assert_eq!(record.messages[0].parent, None);
    for pair in record.messages.windows(2) {
        assert_eq!(pair[1].parent, Some(pair[0].id));
    }
    let reopened = ConversationStore::open(dir.path().to_path_buf()).expect("reopen");
    let loaded = reopened.get(&record.id).expect("loaded");
    assert_eq!(loaded.messages, record.messages);
    assert_eq!(
        reopened.update(&record.id, 0, false, |record| {
            record.messages.swap(0, 1);
            Ok(())
        }),
        Err(ConversationError::Message)
    );
    assert_eq!(reopened.get(&record.id).unwrap().messages, loaded.messages);
    assert_eq!(
        reopened
            .metadata_for(&record.id)
            .and_then(|metadata| metadata.active_leaf),
        record.messages.last().map(|message| message.id)
    );
}

#[test]
fn foreign_parent_fails_closed_on_load() {
    let dir = tempfile::tempdir().expect("directory");
    let (first, second);
    {
        let store = ConversationStore::open(dir.path().to_path_buf()).expect("store");
        let selection = ModelSelection::new(ProviderKind::Xai, "model".to_owned(), None).unwrap();
        let a = store.create("First".to_owned()).expect("create");
        first = seed_exchanges(&store, &a.id, selection.clone(), 1);
        let b = store.create("Second".to_owned()).expect("create");
        second = seed_exchanges(&store, &b.id, selection, 1);
    }
    {
        let connection =
            rusqlite::Connection::open(dir.path().join("conversations.sqlite3")).expect("db");
        // Cross-conversation references are not part of the same history.
        connection
            .execute(
                "UPDATE messages SET message = json_set(message, '$.parent', ?3) \
                 WHERE conversation_id = ?1 AND id = ?2",
                rusqlite::params![
                    first.id.as_hex(),
                    first.messages[0].id.as_hex(),
                    second.messages[0].id.as_hex()
                ],
            )
            .expect("tamper");
    }
    let store = ConversationStore::open(dir.path().to_path_buf()).expect("reopen");
    assert!(store.get(&first.id).is_none());
    assert_eq!(
        store.transcript_window(&first.id, None, None),
        Err(ConversationError::Corrupt)
    );
    assert_eq!(
        store.tree_window(&first.id, None, None, None),
        Err(ConversationError::Corrupt)
    );
    assert!(store.get(&second.id).is_some());
}

#[test]
fn cyclic_parent_chain_fails_closed_without_looping() {
    let dir = tempfile::tempdir().expect("directory");
    let record;
    {
        let store = ConversationStore::open(dir.path().to_path_buf()).expect("store");
        let selection = ModelSelection::new(ProviderKind::Xai, "model".to_owned(), None).unwrap();
        let created = store.create("Cycle".to_owned()).expect("create");
        record = seed_exchanges(&store, &created.id, selection, 1);
    }
    {
        let connection =
            rusqlite::Connection::open(dir.path().join("conversations.sqlite3")).expect("db");
        // The root points at its own child, so the parent walk revisits an entry.
        connection
            .execute(
                "UPDATE messages SET message = json_set(message, '$.parent', ?3) \
                 WHERE conversation_id = ?1 AND id = ?2",
                rusqlite::params![
                    record.id.as_hex(),
                    record.messages[0].id.as_hex(),
                    record.messages[1].id.as_hex()
                ],
            )
            .expect("tamper");
    }
    let store = ConversationStore::open(dir.path().to_path_buf()).expect("reopen");
    assert!(store.get(&record.id).is_none());
    assert_eq!(
        store.transcript_window(&record.id, None, None),
        Err(ConversationError::Corrupt)
    );
}

#[test]
fn inspection_of_a_retained_sibling_never_changes_the_active_projection() {
    let dir = tempfile::tempdir().unwrap();
    let store = ConversationStore::open(dir.path().to_path_buf()).unwrap();
    let selection = ModelSelection::new(ProviderKind::Xai, "model".to_owned(), None).unwrap();
    let created = store.create("Branches".to_owned()).unwrap();
    let record = seed_exchanges(&store, &created.id, selection.clone(), 2);
    let job = JobId::generate().unwrap();
    let mut record = store
        .begin_message(
            &record.id,
            record.revision,
            selection,
            job,
            "Active input".to_owned(),
        )
        .unwrap();
    let mut sibling = record.messages[1].clone();
    sibling.id = super::MessageId::generate().unwrap();
    sibling.text = "Retained alternative".to_owned();
    sibling.request = Some(JobId::generate().unwrap());
    let connection = rusqlite::Connection::open(dir.path().join("conversations.sqlite3")).unwrap();
    connection
        .execute(
            "INSERT INTO messages (conversation_id, sequence, id, message) VALUES (?1, 6, ?2, ?3)",
            rusqlite::params![
                record.id.as_hex(),
                sibling.id.as_hex(),
                serde_json::to_string(&super::message_to_file(&sibling)).unwrap()
            ],
        )
        .unwrap();

    store
        .checkpoint_output(&record.id, job, "Active reply")
        .unwrap();
    record.messages.last_mut().unwrap().text = "Active reply".to_owned();
    assert_eq!(store.get(&record.id).unwrap().messages, record.messages);
    assert_eq!(
        store.transcript_window(
            &record.id,
            Some(super::TranscriptCursor::Around(sibling.id)),
            None
        ),
        Err(ConversationError::Entry)
    );
    let (_, window) = store
        .transcript_window(&record.id, None, Some(sibling.id))
        .unwrap()
        .unwrap();
    assert!(!window.live);
    assert_eq!(
        window.messages,
        vec![record.messages[0].clone(), sibling.clone()]
    );
    let children = store
        .tree_window(&record.id, None, None, Some(record.messages[0].id))
        .unwrap()
        .unwrap();
    assert_eq!(children.entries.len(), 2);
    assert!(children.entries[0].on_active_path);
    assert!(!children.entries[1].on_active_path);
    assert_eq!(store.get(&record.id).unwrap().messages, record.messages);

    connection.execute("UPDATE conversations SET metadata = json_set(metadata, '$.active-leaf', ?2) WHERE id = ?1", rusqlite::params![record.id.as_hex(), super::MessageId::generate().unwrap().as_hex()]).unwrap();
    assert_eq!(
        store.transcript_window(&record.id, None, None),
        Err(ConversationError::Corrupt)
    );
}

#[test]
fn tree_window_pages_by_append_order_and_filters_text() {
    let store = ConversationStore::in_memory();
    let selection = ModelSelection::new(ProviderKind::Xai, "model".to_owned(), None).unwrap();
    let created = store.create("Tree".to_owned()).expect("create");
    let record = seed_exchanges(&store, &created.id, selection, 40);

    let first = store
        .tree_window(&record.id, None, None, None)
        .unwrap()
        .expect("window");
    assert_eq!(first.entries.len(), super::TREE_PAGE);
    assert_eq!(first.total, record.messages.len());
    assert!(first.has_more);
    let cursor = first.next_cursor.expect("cursor");
    let second = store
        .tree_window(&record.id, Some(cursor), None, None)
        .unwrap()
        .expect("window");
    assert_eq!(
        second.entries.len(),
        record.messages.len() - super::TREE_PAGE
    );
    assert!(!second.has_more);

    let search = store
        .tree_window(&record.id, None, Some("Question 7"), None)
        .unwrap()
        .expect("window");
    assert!(!search.entries.is_empty());
    assert!(
        search
            .entries
            .iter()
            .all(|entry| entry.text.contains("Question 7"))
    );
    assert!(!search.partial);

    let children = store
        .tree_window(&record.id, None, None, Some(record.messages[0].id))
        .unwrap()
        .unwrap();
    assert_eq!(children.entries.len(), 1);
    assert_eq!(children.entries[0].id, record.messages[1].id);

    let first = store
        .tree_window(&record.id, None, Some("e"), None)
        .unwrap()
        .unwrap();
    assert!(first.partial);
    let next = store
        .tree_window(&record.id, first.next_cursor, Some("e"), None)
        .unwrap()
        .unwrap();
    assert!(!next.has_more);
    assert_eq!(
        first.entries.len() + next.entries.len(),
        record.messages.len()
    );
}

#[test]
fn branch_selection_retains_abandoned_descendants_and_projects_only_the_selected_path() {
    let dir = tempfile::tempdir().unwrap();
    let record;
    let selection = ModelSelection::new(ProviderKind::Xai, "model".to_owned(), None).unwrap();
    {
        let store = ConversationStore::open(dir.path().to_path_buf()).unwrap();
        let created = store.create("Branches".to_owned()).unwrap();
        record = seed_exchanges(&store, &created.id, selection.clone(), 2);
        let destination = record.messages[1].id;
        let leaf = record.messages.last().unwrap().id;
        let updated = store
            .continue_from(&record.id, record.revision, Some(leaf), destination)
            .unwrap();
        assert_eq!(
            updated
                .messages
                .iter()
                .map(|message| message.id)
                .collect::<Vec<_>>(),
            vec![record.messages[0].id, destination]
        );
        assert_eq!(updated.revision, record.revision + 1);
        // A stale revision after another mutation cannot select a branch.
        assert_eq!(
            store.continue_from(&record.id, record.revision, Some(leaf), destination),
            Err(ConversationError::Conflict)
        );
        let job = JobId::generate().unwrap();
        let next = store
            .begin_message(
                &record.id,
                updated.revision,
                selection.clone(),
                job,
                "Alternative".to_owned(),
            )
            .unwrap();
        assert_eq!(next.messages[2].parent, Some(destination));
        store
            .settle_message(
                &record.id,
                job,
                "Alternative reply",
                MessageStatus::Complete,
                None,
            )
            .unwrap();
    }
    let store = ConversationStore::open(dir.path().to_path_buf()).unwrap();
    let loaded = store.get(&record.id).unwrap();
    let texts: Vec<String> = crate::conversations::history::project(&loaded.messages, None)
        .unwrap()
        .into_iter()
        .map(|turn| turn.text)
        .collect();
    assert!(texts.iter().any(|text| text == "Alternative"));
    assert!(!texts.iter().any(|text| text == "Question 1"));
    assert!(!texts.iter().any(|text| text == "Reply 1"));
    assert_eq!(
        crate::conversations::forks::snapshot(&loaded, record.messages[3].id).err(),
        Some(crate::conversations::forks::ForkError::Missing),
    );

    let tree = store
        .tree_window(&record.id, None, None, None)
        .unwrap()
        .expect("tree");
    assert!(
        tree.entries
            .iter()
            .any(|entry| entry.text == "Question 1" && !entry.on_active_path)
    );
    assert!(
        tree.entries
            .iter()
            .any(|entry| entry.text == "Alternative" && entry.on_active_path)
    );
    assert!(tree.tips.len() >= 2);
}

#[test]
fn branch_selection_rejects_stale_queued_and_unsettled_states() {
    let store = ConversationStore::in_memory();
    let selection = ModelSelection::new(ProviderKind::Xai, "model".to_owned(), None).unwrap();
    let created = store.create("Reject branch".to_owned()).unwrap();
    let record = seed_exchanges(&store, &created.id, selection.clone(), 2);
    let destination = record.messages[1].id;
    let leaf = record.messages.last().unwrap().id;
    assert_eq!(
        store.continue_from(&record.id, record.revision + 1, Some(leaf), destination),
        Err(ConversationError::Conflict)
    );
    assert_eq!(
        store.continue_from(
            &record.id,
            record.revision,
            Some(record.messages[0].id),
            destination
        ),
        Err(ConversationError::Conflict)
    );
    assert_eq!(
        store.continue_from(
            &record.id,
            record.revision,
            Some(leaf),
            super::MessageId::generate().unwrap()
        ),
        Err(ConversationError::Entry)
    );
    store
        .enqueue(
            &record.id,
            record.queue.revision,
            "Queued".to_owned(),
            crate::conversations::QueueDelivery::FollowUp,
            None,
        )
        .unwrap();
    assert_eq!(
        store.continue_from(&record.id, record.revision, Some(leaf), destination),
        Err(ConversationError::Active)
    );
    let queued = store.get(&record.id).unwrap();
    let item = queued.queue.items[0].id;
    store
        .remove_queue_item(&record.id, queued.queue.revision, item)
        .unwrap();

    let job = JobId::generate().unwrap();
    let pending = store
        .begin_message(
            &record.id,
            record.revision,
            selection.clone(),
            job,
            "Pending".to_owned(),
        )
        .unwrap();
    let pending_id = pending.messages.last().unwrap().id;
    assert_eq!(
        store.continue_from(&record.id, pending.revision, Some(pending_id), destination),
        Err(ConversationError::Active)
    );
    store
        .settle_message(&record.id, job, "", MessageStatus::Interrupted, None)
        .unwrap();
    let settled = store.get(&record.id).unwrap();
    let interrupted = settled.messages.last().unwrap().id;
    assert_eq!(
        store.continue_from(&record.id, settled.revision, Some(interrupted), interrupted),
        Err(ConversationError::Entry)
    );
}

#[test]
fn branch_selection_restores_only_ancestor_compactions_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let store = ConversationStore::open(dir.path().to_path_buf()).unwrap();
    let selection = ModelSelection::new(ProviderKind::Xai, "model".to_owned(), None).unwrap();
    let created = store.create("Compact branch".to_owned()).unwrap();
    let record = seed_exchanges(&store, &created.id, selection, 3);
    let job = JobId::generate().unwrap();
    store
        .begin_compaction(&record.id, record.revision, job)
        .unwrap();
    let request = crate::conversations::RequestUsage {
        id: crate::conversations::RequestId::generate().unwrap(),
        usage: crate::providers::ModelUsage::new(ProviderKind::Xai, "model"),
        auth: crate::providers::AuthMethod::ApiKey,
        prices: None,
        sources: Vec::new(),
        advertised: Vec::new(),
    };
    store
        .record_summary_request(&record.id, job, &request)
        .unwrap();
    store
        .record_job_compaction(
            &record.id,
            job,
            crate::conversations::CompactionRecord {
                covered_through: record.messages[3].id,
                retained_from: record.messages[4].id,
                text: "Earlier context".to_owned(),
                requests: vec![request.clone()],
                preserve: None,
                created_at_ms: 1,
            },
        )
        .unwrap();
    let settled = store.finish_compaction(&record.id, job, None).unwrap();
    assert!(settled.compaction.is_some());

    let updated = store
        .continue_from(
            &record.id,
            settled.revision,
            settled.messages.last().map(|message| message.id),
            settled.messages[1].id,
        )
        .unwrap();
    assert!(updated.compaction.is_none());
    assert_eq!(updated.summary_requests, vec![request]);
    drop(store);
    let store = ConversationStore::open(dir.path().to_path_buf()).unwrap();
    let restored = store
        .continue_from(
            &record.id,
            updated.revision,
            updated.messages.last().map(|message| message.id),
            record.messages[3].id,
        )
        .unwrap();
    assert_eq!(restored.compaction, settled.compaction);
    let history = crate::conversations::compaction::project(
        &restored.messages,
        None,
        restored.compaction.as_ref(),
    )
    .unwrap();
    assert_eq!(history.len(), 1);
    assert!(history[0].text.contains("Earlier context"));
    let job = JobId::generate().unwrap();
    let next = store
        .begin_message(
            &record.id,
            restored.revision,
            record.model.as_ref().unwrap().settings.model.clone(),
            job,
            "New sibling".to_owned(),
        )
        .unwrap();
    let history =
        crate::conversations::compaction::project(&next.messages, None, next.compaction.as_ref())
            .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].text, "New sibling");
}

#[test]
fn phase_commits_group_one_logical_response_and_only_the_last_phase_is_final() {
    use crate::providers::{AssistantReply, CompletionReason, ToolOutput};
    let dir = tempfile::tempdir().unwrap();
    let store = ConversationStore::open(dir.path().to_path_buf()).unwrap();
    let record = store.create("Long turn".to_owned()).unwrap();
    let job = JobId::generate().unwrap();
    let selection = ModelSelection::new(ProviderKind::Xai, "model".to_owned(), None).unwrap();
    let record = store
        .begin_message(
            &record.id,
            record.revision,
            selection,
            job,
            "Work".to_owned(),
        )
        .unwrap();
    for id in ["call-1", "call-2"] {
        let mut reply = AssistantReply::default();
        reply.start_tool(
            id.to_owned(),
            "read".to_owned(),
            serde_json::json!({"path": "main.rs"}),
        );
        reply.finish_tool(
            id,
            ToolOutput {
                resource: None,
                label: "read".to_owned(),
                output: "ok".to_owned(),
                command: None,
            },
        );
        reply.completion = Some(CompletionReason::ToolCalls);
        store.settle_tool_batch(&record.id, job, &reply).unwrap();
    }
    let refreshed = store.get(&record.id).unwrap();
    let phases: Vec<_> = refreshed
        .messages
        .iter()
        .filter(|message| message.role == super::MessageRole::Assistant)
        .collect();
    assert_eq!(
        phases.len(),
        3,
        "two completed phases and one pending phase"
    );
    let anchor = phases[0].response.expect("logical response anchor");
    assert!(phases.iter().all(|phase| phase.response == Some(anchor)));
    assert!(phases[..2].iter().all(|phase| !phase.final_phase));
    assert!(!phases[2].final_phase);
    store
        .settle_message(
            &record.id,
            job,
            AssistantReply::default(),
            MessageStatus::Complete,
            None,
        )
        .unwrap();
    let settled = store.get(&record.id).unwrap();
    let phases: Vec<_> = settled
        .messages
        .iter()
        .filter(|message| message.role == super::MessageRole::Assistant)
        .collect();
    assert!(phases[..2].iter().all(|phase| !phase.final_phase));
    assert!(phases[2].final_phase);
    assert_eq!(
        store.continue_from(
            &record.id,
            settled.revision,
            Some(phases[2].id),
            phases[0].id,
        ),
        Err(ConversationError::Entry)
    );
    let window = store
        .tree_window(&record.id, None, None, None)
        .unwrap()
        .expect("tree window");
    let assistant: Vec<_> = window
        .entries
        .iter()
        .filter(|entry| entry.role == super::MessageRole::Assistant)
        .collect();
    assert!(assistant[..2].iter().all(|entry| !entry.final_phase));
    assert!(assistant[2].final_phase);
}
