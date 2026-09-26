use crate::{conversations::ConversationId, sessions};

use super::AccessConsentStore;

fn session() -> sessions::SessionId {
    let token = sessions::generate_session_token().unwrap();
    token.id()
}

#[test]
fn draft_consent_is_single_use_and_becomes_conversation_consent() {
    let root = tempfile::tempdir().unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(root.path(), &[]).unwrap();
    let directories = vec![grant.clone()];
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(
            crate::providers::ProviderKind::Xai,
            "model".to_owned(),
            None,
        )
        .unwrap(),
        String::new(),
        Vec::new(),
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_directories(directories.clone())
    .unwrap();
    let session = session();
    let store = AccessConsentStore::new();
    let request = store
        .request_draft(session, "draft", &directories, &grant)
        .unwrap();
    let reference = store
        .approve_draft(&request, session, "draft", &directories, &grant)
        .unwrap();

    assert!(
        store
            .approve_draft(&request, session, "draft", &directories, &grant)
            .is_err()
    );
    let conversation = ConversationId::generate().unwrap();
    store
        .consume_draft(
            &reference,
            session,
            "draft",
            &settings,
            conversation,
            &directories,
        )
        .unwrap();
    assert!(store.authorised_conversation(session, conversation, &settings, &grant));
    let mut changed = settings.clone();
    changed.network = crate::agents::NetworkAccess::Public;
    assert!(!store.authorised_conversation(session, conversation, &changed, &grant));
    changed = settings.clone();
    changed.environment = crate::environments::EnvironmentId::generate().unwrap();
    assert!(!store.authorised_conversation(session, conversation, &changed, &grant));
    changed = settings.clone();
    changed.tools.push(crate::agents::ToolId::Read);
    assert!(!store.authorised_conversation(session, conversation, &changed, &grant));
    assert!(!AccessConsentStore::new().authorised_conversation(
        session,
        conversation,
        &settings,
        &grant
    ));
    assert!(
        store
            .consume_draft(
                &reference,
                session,
                "draft",
                &settings,
                conversation,
                &directories,
            )
            .is_err()
    );
    assert!(
        store
            .request_draft(session, "draft:changed-settings", &directories, &grant)
            .is_err()
    );
    store.retain_sessions(|_| false);
    assert!(!store.authorised_conversation(session, conversation, &settings, &grant));
}

#[test]
fn direct_write_consent_is_destination_bound_and_single_use() {
    let home = tempfile::tempdir().unwrap();
    let data = home.path().join("frinkworks");
    std::fs::create_dir(&data).unwrap();
    let mut grant = crate::execution::DirectoryGrant::from_selected(home.path(), &[]).unwrap();
    grant.access = crate::execution::DirectoryAccess::Write;
    assert!(crate::execution::authority::sensitive_directory(
        &grant.host_path,
        &data
    ));
    let grants = vec![grant.clone()];
    let store = AccessConsentStore::new();
    let owner = session();
    let request = store
        .request_draft(owner, "original", &grants, &grant)
        .unwrap();
    let approval = store
        .approve_draft(&request, owner, "original", &grants, &grant)
        .unwrap();
    assert!(
        store
            .approve_draft(&request, owner, "original", &grants, &grant)
            .is_err()
    );
    assert!(store.authorised_draft(&approval, owner, "original", &grants, &grant));
    assert!(!store.authorised_draft(&approval, owner, "copy", &grants, &grant));
    let mut reviewed = grant.clone();
    reviewed.access = crate::execution::DirectoryAccess::Read;
    assert!(!store.authorised_draft(&approval, owner, "original", &[reviewed.clone()], &reviewed));
    store.retain_sessions(|_| false);
    assert!(!store.authorised_draft(&approval, owner, "original", &grants, &grant));
}

#[test]
fn host_consent_is_destination_bound_and_not_copied() {
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(
            crate::providers::ProviderKind::Xai,
            "model".to_owned(),
            None,
        )
        .unwrap(),
        String::new(),
        vec![crate::agents::ToolId::Run],
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_location(crate::execution::ToolLocation::Host);
    let store = AccessConsentStore::new();
    let owner = session();
    let request = store.request_host_draft(owner, "draft", &settings).unwrap();
    let reference = store
        .approve_host_draft(&request, owner, "draft", &settings)
        .unwrap();
    assert!(store.authorised_host_draft(&reference, owner, "draft", &settings));
    assert!(!store.authorised_host_draft(&reference, owner, "copy", &settings));
    let sandbox = settings
        .clone()
        .with_location(crate::execution::ToolLocation::Sandbox);
    assert!(!store.authorised_host_draft(&reference, owner, "draft", &sandbox));
    let conversation = ConversationId::generate().unwrap();
    store
        .consume_draft(&reference, owner, "draft", &settings, conversation, &[])
        .unwrap();
    assert!(store.authorised_host_conversation(owner, conversation, &settings));
    assert!(!store.authorised_host_conversation(owner, conversation, &sandbox));
    let automatic = settings
        .clone()
        .with_host_approval(crate::execution::HostApprovalPolicy::Automatic);
    assert!(!store.authorised_host_conversation(owner, conversation, &automatic));
    store.retain_sessions(|_| false);
    assert!(!store.authorised_host_conversation(owner, conversation, &settings));
}

#[test]
fn automatic_host_consent_does_not_reuse_ask_each_time_approval() {
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(
            crate::providers::ProviderKind::Xai,
            "model".to_owned(),
            None,
        )
        .unwrap(),
        String::new(),
        vec![crate::agents::ToolId::Run],
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_location(crate::execution::ToolLocation::Host);
    let automatic = settings
        .clone()
        .with_host_approval(crate::execution::HostApprovalPolicy::Automatic);
    let store = AccessConsentStore::new();
    let owner = session();
    let conversation = ConversationId::generate().unwrap();
    let request = store
        .request_host_conversation(owner, conversation, &settings)
        .unwrap();
    store
        .approve_host_conversation(&request, owner, conversation, &settings)
        .unwrap();
    assert!(store.authorised_host_conversation(owner, conversation, &settings));
    assert!(!store.authorised_host_conversation(owner, conversation, &automatic));
    let automatic_request = store
        .request_host_conversation(owner, conversation, &automatic)
        .unwrap();
    store
        .approve_host_conversation(&automatic_request, owner, conversation, &automatic)
        .unwrap();
    assert!(store.authorised_host_conversation(owner, conversation, &automatic));
    assert!(!store.authorised_host_conversation(owner, conversation, &settings));
    store.retain_sessions(|_| false);
    assert!(!store.authorised_host_conversation(owner, conversation, &automatic));
}

#[test]
fn consent_binds_session_access_and_path_but_not_native_identity() {
    let parent = tempfile::tempdir().unwrap();
    let first = parent.path().join("first");
    std::fs::create_dir(&first).unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(&first, &[]).unwrap();
    let directories = vec![grant.clone()];
    let owner = session();
    let other = session();
    let store = AccessConsentStore::new();
    let request = store
        .request_draft(owner, "draft", &directories, &grant)
        .unwrap();

    assert!(
        store
            .approve_draft(&request, other, "draft", &directories, &grant)
            .is_err()
    );
    let mut changed = grant.clone();
    changed.access = crate::execution::DirectoryAccess::Write;
    assert!(
        store
            .approve_draft(&request, owner, "draft", &[changed.clone()], &changed)
            .is_err()
    );

    let mut changed = grant.clone();
    changed.host_path = parent.path().join("other");
    assert!(
        store
            .approve_draft(&request, owner, "draft", &[changed.clone()], &changed)
            .is_err()
    );

    std::fs::rename(&first, parent.path().join("old")).unwrap();
    std::fs::create_dir(&first).unwrap();
    let replacement = crate::execution::DirectoryGrant::from_selected(&first, &[]).unwrap();
    assert_ne!(grant.id, replacement.id);
    assert_eq!(grant.revalidate(), Ok(()));
    assert!(
        store
            .approve_draft(
                &request,
                owner,
                "draft",
                std::slice::from_ref(&replacement),
                &replacement
            )
            .is_ok()
    );
}
