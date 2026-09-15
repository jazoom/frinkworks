use std::path::Path;

use crate::{
    agents::{AccessMode, NetworkAccess},
    projects::ProjectId,
    providers::{ModelSelection, ProviderKind},
    sessions::JobId,
};

use super::{
    ConversationError, ConversationStore, MAXIMUM_CATALOGUE_BYTES, MAXIMUM_TITLE_BYTES,
    MessageStatus,
};

impl ConversationStore {
    fn create_record(
        &self,
        title: String,
        projects: Vec<ProjectId>,
        title_pending: bool,
    ) -> Result<super::ConversationRecord, ConversationError> {
        self.create_saved(
            super::ConversationId::generate().map_err(|_| ConversationError::Random)?,
            projects.first().copied(),
            (!title_pending).then_some(title),
            None,
            Vec::new(),
        )
    }

    pub(crate) fn create_untitled(
        &self,
        project: Option<ProjectId>,
    ) -> Result<super::ConversationRecord, ConversationError> {
        self.create_record(
            "New conversation".to_owned(),
            project.into_iter().collect(),
            true,
        )
    }

    pub(crate) fn create(
        &self,
        title: String,
    ) -> Result<super::ConversationRecord, ConversationError> {
        self.create_record(title, Vec::new(), false)
    }

    pub(crate) fn create_with_project(
        &self,
        title: String,
        project: ProjectId,
    ) -> Result<super::ConversationRecord, ConversationError> {
        self.create_record(title, vec![project], false)
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
            path: None,
            inner: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            title_updates: tokio::sync::broadcast::channel(16).0,
        }
    }
}

#[test]
fn restart_preserves_explicit_saves_with_or_without_a_manual_title() {
    let dir = tempfile::tempdir().unwrap();
    let store = ConversationStore::in_memory();
    let draft = store.create_untitled(None).unwrap();
    let saved = store.create("New conversation".to_owned()).unwrap();
    let file = super::CatalogueFile {
        version: super::CATALOGUE_VERSION,
        conversations: vec![super::record_to_file(&draft), super::record_to_file(&saved)],
    };
    write_catalogue(dir.path(), &serde_json::to_string(&file).unwrap());

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
            None,
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
fn capacity_never_evicts_saved_conversations() {
    let store = ConversationStore::in_memory();
    for _ in 0..super::MAXIMUM_CONVERSATIONS {
        store
            .create_saved(
                super::ConversationId::generate().unwrap(),
                None,
                None,
                None,
                Vec::new(),
            )
            .unwrap();
    }
    let before = store.list();
    assert_eq!(
        store.create_saved(
            super::ConversationId::generate().unwrap(),
            None,
            None,
            None,
            Vec::new()
        ),
        Err(ConversationError::Full)
    );
    assert_eq!(store.list(), before);
}

#[test]
fn automatic_titles_preserve_manual_edits_and_command_revisions() {
    let store = ConversationStore::in_memory();
    let record = store.create_untitled(None).unwrap();
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

    let record = store.create_untitled(None).unwrap();
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
                .select_model_configuration(&conversation.id, conversation.revision, model)
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

fn write_catalogue(dir: &Path, contents: &str) {
    std::fs::write(dir.join("catalogue.json"), contents).expect("catalogue");
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
fn a_project_reference_is_persisted_without_access_or_model_configuration() {
    let dir = tempfile::tempdir().expect("directory");
    let project = ProjectId::parse("0123456789abcdef0123456789abcdef").expect("project id");
    let store = ConversationStore::open(dir.path().to_path_buf()).expect("store");
    let record = store
        .create_with_project("Project discussion".to_owned(), project)
        .expect("conversation");

    assert_eq!(record.projects, vec![project]);
    assert!(record.grants.is_empty());
    assert!(record.model.is_none());
    drop(store);

    let reopened = ConversationStore::open(dir.path().to_path_buf()).expect("reopen");
    assert_eq!(
        reopened.get(&record.id).expect("record").projects,
        vec![project]
    );
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
fn corrupt_catalogues_remain_unchanged() {
    let dir = tempfile::tempdir().expect("directory");
    write_catalogue(dir.path(), "{");
    let path = dir.path().join("catalogue.json");
    let original = std::fs::read(&path).expect("original");

    assert_eq!(
        ConversationStore::open(dir.path().to_path_buf()).err(),
        Some(ConversationError::Corrupt)
    );
    assert_eq!(std::fs::read(path).expect("unchanged"), original);
}

#[test]
fn missing_network_fields_reject_the_catalogue_without_replacement() {
    for field in ["network", "network-domains"] {
        let dir = tempfile::tempdir().expect("directory");
        let store = ConversationStore::open(dir.path().to_path_buf()).expect("store");
        store.create("Discussion".to_owned()).expect("record");
        drop(store);
        let path = dir.path().join("catalogue.json");
        let mut file: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("catalogue")).expect("JSON");
        file["conversations"][0]
            .as_object_mut()
            .expect("record")
            .remove(field);
        let bytes = serde_json::to_vec(&file).expect("JSON");
        std::fs::write(&path, &bytes).expect("catalogue");
        assert_eq!(
            ConversationStore::open(dir.path().to_path_buf()).err(),
            Some(ConversationError::Corrupt)
        );
        assert_eq!(std::fs::read(path).expect("unchanged catalogue"), bytes);
    }
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
fn titles_and_catalogue_reads_are_bounded() {
    let store = ConversationStore::in_memory();
    for title in [
        String::new(),
        "   ".to_owned(),
        "a\0b".to_owned(),
        "é".repeat(MAXIMUM_TITLE_BYTES / 2 + 1),
    ] {
        assert_eq!(store.create(title).err(), Some(ConversationError::Title));
    }

    let dir = tempfile::tempdir().expect("directory");
    write_catalogue(dir.path(), &"x".repeat(MAXIMUM_CATALOGUE_BYTES + 1));
    assert_eq!(
        ConversationStore::open(dir.path().to_path_buf()).err(),
        Some(ConversationError::Corrupt)
    );
}

#[test]
fn full_catalogue_rejects_creation_without_eviction() {
    let store = ConversationStore::in_memory();
    for index in 0..super::MAXIMUM_CONVERSATIONS {
        store
            .create(format!("Conversation {index}"))
            .expect("create");
    }
    let original = store.list();
    assert_eq!(
        store.create("Overflow".to_owned()),
        Err(ConversationError::Full)
    );
    assert_eq!(store.list(), original);
}

#[test]
fn rename_preserves_timestamp_order_after_clock_regression() {
    let dir = tempfile::tempdir().expect("directory");
    let store = ConversationStore::open(dir.path().to_path_buf()).expect("store");
    let record = store.create("Original".to_owned()).expect("create");
    let mut future = record.clone();
    future.created_at_ms = u64::MAX;
    future.updated_at_ms = u64::MAX;
    store.lock().insert(record.id, future);
    let renamed = store
        .rename(&record.id, record.revision, "Renamed".to_owned())
        .expect("rename");
    drop(store);
    let reopened = ConversationStore::open(dir.path().to_path_buf()).expect("reopen");
    assert_eq!(reopened.get(&record.id), Some(renamed));
}

#[test]
fn project_associations_survive_restart() {
    let dir = tempfile::tempdir().expect("directory");
    let first = crate::projects::ProjectId::generate().expect("first project");
    let second = crate::projects::ProjectId::generate().expect("second project");
    let record;
    {
        let store = ConversationStore::open(dir.path().to_path_buf()).expect("store");
        let created = store.create("Discussion".to_owned()).expect("conversation");
        let attached = store
            .attach_project(&created.id, created.revision, first)
            .expect("attach first");
        record = store
            .attach_project(&attached.id, attached.revision, second)
            .expect("attach second");
    }
    let store = ConversationStore::open(dir.path().to_path_buf()).expect("reopen");
    assert_eq!(store.get(&record.id), Some(record));
}

#[test]
fn project_associations_are_ordered_bounded_and_revisioned() {
    let store = ConversationStore::in_memory();
    let record = store.create("Discussion".to_owned()).expect("conversation");
    let first = crate::projects::ProjectId::generate().expect("first project");
    let second = crate::projects::ProjectId::generate().expect("second project");
    let attached = store
        .attach_project(&record.id, record.revision, first)
        .expect("attach first");
    let attached = store
        .attach_project(&record.id, attached.revision, second)
        .expect("attach second");
    assert_eq!(attached.projects, vec![first, second]);
    assert_eq!(
        store.attach_project(&attached.id, attached.revision, first),
        Err(ConversationError::DuplicateProject)
    );
    assert_eq!(
        store.detach_project(&attached.id, attached.revision - 1, first),
        Err(ConversationError::Conflict)
    );
    assert_eq!(store.get(&attached.id), Some(attached.clone()));

    let mut current = attached;
    for _ in current.projects.len()..super::MAXIMUM_PROJECT_ASSOCIATIONS {
        current = store
            .attach_project(
                &current.id,
                current.revision,
                crate::projects::ProjectId::generate().expect("project"),
            )
            .expect("attach within bound");
    }
    assert_eq!(
        store.attach_project(
            &current.id,
            current.revision,
            crate::projects::ProjectId::generate().expect("overflow project"),
        ),
        Err(ConversationError::Projects)
    );
}

#[test]
fn only_one_project_can_have_writable_conversation_access() {
    let store = ConversationStore::in_memory();
    let conversation = store.create("Discussion".to_owned()).expect("conversation");
    let first = ProjectId::generate().expect("first project");
    let second = ProjectId::generate().expect("second project");
    let first_attached = store
        .attach_project(&conversation.id, conversation.revision, first)
        .expect("first attachment");
    let attached = store
        .attach_project(&conversation.id, first_attached.revision, second)
        .expect("second attachment");
    let writable = store
        .grant_writable(&attached.id, attached.revision, first, 1)
        .expect("first writable grant");
    let second_read_only = store
        .grant_read_only(&writable.id, writable.revision, second, 1)
        .expect("second read-only grant");
    assert_eq!(second_read_only.execution_target, Some(first));
    assert_eq!(second_read_only.grants[1].access, AccessMode::ReadOnly);
    assert_eq!(
        store.grant_writable(&second_read_only.id, second_read_only.revision, second, 1),
        Err(ConversationError::WriteTarget)
    );
    assert_eq!(store.get(&conversation.id), Some(second_read_only));
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
fn writable_project_grants_survive_restart_with_their_revision() {
    let dir = tempfile::tempdir().expect("directory");
    let project = crate::projects::ProjectId::generate().expect("project");
    let record;
    {
        let store = ConversationStore::open(dir.path().to_path_buf()).expect("store");
        let conversation = store.create("Discussion".to_owned()).expect("conversation");
        let attached = store
            .attach_project(&conversation.id, conversation.revision, project)
            .expect("attach");
        record = store
            .grant_writable(&attached.id, attached.revision, project, 3)
            .expect("grant");
    }
    let store = ConversationStore::open(dir.path().to_path_buf()).expect("reopen");
    let recovered = store.get(&record.id).expect("recovered");
    assert_eq!(
        recovered.grants[0].access,
        crate::agents::AccessMode::ReadWrite
    );
    assert_eq!(recovered.execution_target, Some(project));
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
        store
            .append_output(&record.id, request, "Partial reply".to_owned())
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
fn message_bounds_reserve_space_for_terminal_output() {
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
    store
        .begin_message(
            &record.id,
            record.revision,
            selection.clone(),
            request,
            "Question".to_owned(),
        )
        .expect("begin");
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
    let third = store.create("Third".to_owned()).expect("third");
    assert_eq!(
        store.begin_message(
            &third.id,
            third.revision,
            selection,
            JobId::generate().expect("request"),
            "Question".to_owned(),
        ),
        Err(ConversationError::Full)
    );
    assert_eq!(
        store.append_output(
            &record.id,
            request,
            "x".repeat(super::MAXIMUM_REPLY_BYTES + 1)
        ),
        Err(ConversationError::Message)
    );
    let reply = "\u{0001}".repeat(super::MAXIMUM_REPLY_BYTES);
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
    let record = store.create_untitled(None).unwrap();
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
fn invalid_record_fields_do_not_replace_the_catalogue() {
    let dir = tempfile::tempdir().expect("directory");
    let store = ConversationStore::open(dir.path().to_path_buf()).expect("store");
    store.create("Original".to_owned()).expect("create");
    drop(store);
    let path = dir.path().join("catalogue.json");
    let original: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
    for (field, value) in [
        ("id", serde_json::json!("../outside")),
        ("revision", serde_json::json!(0)),
        ("title", serde_json::json!(" padded ")),
        ("updated-at-ms", serde_json::json!(0)),
    ] {
        let mut invalid = original.clone();
        invalid["conversations"][0][field] = value;
        let bytes = serde_json::to_vec(&invalid).expect("encode");
        std::fs::write(&path, &bytes).expect("write");
        assert_eq!(
            ConversationStore::open(dir.path().to_path_buf()).err(),
            Some(ConversationError::Corrupt)
        );
        assert_eq!(std::fs::read(&path).expect("unchanged"), bytes);
    }
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
            None,
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
