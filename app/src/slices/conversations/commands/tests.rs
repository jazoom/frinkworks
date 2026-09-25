use std::path::Path;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use tower::ServiceExt;

use super::super::tests::{
    app, command, connected, document, form_value, session_id, state_with_prompts, test_state, text,
};
use crate::{
    conversations::{
        ConversationId, ConversationModelConfiguration, ConversationRecord, MessageRole,
    },
    execution::{DirectoryGrant, ExecutionSettings, ToolLocation},
    providers::{ModelSelection, ProviderKind},
    state::AppState,
};

fn conversation_with_grants(
    state: &AppState,
    grants: Vec<DirectoryGrant>,
    location: ToolLocation,
) -> crate::conversations::ConversationRecord {
    let selection =
        ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None).expect("model");
    let settings = ExecutionSettings::new(
        selection,
        String::new(),
        Vec::new(),
        crate::tests::test_environment_id(),
    )
    .expect("settings")
    .with_directories(grants)
    .expect("directories")
    .with_location(location);
    state
        .conversations
        .create_saved(
            ConversationId::generate().expect("id"),
            Some("Commands test".to_owned()),
            Some(ConversationModelConfiguration {
                settings,
                preset: None,
            }),
            Vec::new(),
        )
        .expect("create")
}

fn conversation(state: &AppState, directory: &Path) -> (ConversationRecord, DirectoryGrant) {
    let grant = DirectoryGrant::from_selected(directory, &[]).expect("grant");
    let record = conversation_with_grants(state, vec![grant.clone()], ToolLocation::Sandbox);
    (record, grant)
}

fn json_request(path: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header(header::COOKIE, format!("frinkworks_session={token}"))
        .header(header::ACCEPT, "application/json")
        .body(Body::empty())
        .expect("request")
}

fn patch_request(path: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header(header::COOKIE, format!("frinkworks_session={token}"))
        .header(hypergraft::GRAFT_REQUEST, "patch")
        .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
        .body(Body::empty())
        .expect("request")
}

fn commands(body: &str) -> Vec<String> {
    let value: serde_json::Value = serde_json::from_str(body).expect("json");
    value["suggestions"]
        .as_array()
        .expect("suggestions")
        .iter()
        .map(|value| value["command"].as_str().expect("command").to_owned())
        .collect()
}

fn seed_global_skill(state: &AppState) {
    state
        .skills
        .create(
            "---\nname: code-review\ndescription: Review code.\n---\n\nReview the diff.\n"
                .to_owned(),
        )
        .expect("global skill");
}

fn project_fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("root");
    let skill = root.path().join(".agents/skills/probe");
    std::fs::create_dir_all(&skill).expect("skill dir");
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: probe\ndescription: Inspect a probe target.\n---\n\n# Probe\n\nRead the probe.\n",
    )
    .expect("skill file");
    root
}

fn prompt_fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("root");
    let prompts = root.path().join(".agents/prompts");
    std::fs::create_dir_all(&prompts).expect("prompts dir");
    std::fs::write(
        prompts.join("review.md"),
        "---\ndescription: Project review.\nargument-hint: <path>\n---\nProject review $1.",
    )
    .expect("prompt file");
    root
}

fn prompt_suggestions(body: &str) -> Vec<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_str(body).expect("json");
    value["suggestions"]
        .as_array()
        .expect("suggestions")
        .iter()
        .filter(|suggestion| suggestion["kind"] == "prompt")
        .cloned()
        .collect()
}

#[tokio::test]
async fn draft_preview_does_not_expose_a_known_provider_key() {
    let state = test_state();
    let token = connected(&state);
    state
        .skills
        .create(
            "---\nname: leak\ndescription: Inspect a fixture.\n---\nThe key is test-key.\n"
                .to_owned(),
        )
        .expect("skill");
    let response = app(&state)
        .oneshot(json_request(
            "/conversations/new/commands?q=/skill:leak&mode=preview",
            &token,
        ))
        .await
        .expect("preview");
    let body = text(response).await;
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert!(value["preview"].is_null());
    assert!(!body.contains("test-key"));
}

#[tokio::test]
async fn saved_lookup_suggests_global_skills() {
    let state = test_state();
    let token = connected(&state);
    seed_global_skill(&state);
    let record = conversation_with_grants(&state, Vec::new(), ToolLocation::Sandbox);
    let response = app(&state)
        .oneshot(json_request(
            &format!("/conversations/{}/commands?q=/skill:co", record.id),
            &token,
        ))
        .await
        .expect("lookup");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(
        commands(&body).contains(&"/skill:code-review".to_owned()),
        "{body}"
    );
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert!(value["preview"].is_null());
    assert_eq!(value["suggestions"][0]["source_label"], "Global");
}

#[tokio::test]
async fn saved_preview_binds_source_hash_and_rejects_unknown() {
    let state = test_state();
    let token = connected(&state);
    seed_global_skill(&state);
    let record = conversation_with_grants(&state, Vec::new(), ToolLocation::Sandbox);
    let response = app(&state)
        .oneshot(json_request(
            &format!(
                "/conversations/{}/commands?q=/skill:code-review&mode=preview",
                record.id
            ),
            &token,
        ))
        .await
        .expect("preview");
    let body = text(response).await;
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    let hash = value["preview"]["hash"].as_str().expect("hash");
    assert!(hash.starts_with("sha256:"), "{body}");
    assert_eq!(
        value["preview"]["base"].as_str().expect("base"),
        "/.agents/skills/code-review"
    );

    let response = app(&state)
        .oneshot(json_request(
            &format!(
                "/conversations/{}/commands?q=/skill:missing&mode=preview",
                record.id
            ),
            &token,
        ))
        .await
        .expect("preview");
    let body = text(response).await;
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert!(value["preview"].is_null());
    assert!(
        value["message"]
            .as_str()
            .expect("message")
            .contains("No available skill or template matches"),
        "{body}"
    );
}

#[tokio::test]
async fn project_skill_uses_the_grant_alias_without_a_host_path() {
    let state = test_state();
    let token = connected(&state);
    let root = project_fixture();
    let (record, grant) = conversation(&state, root.path());
    let response = app(&state)
        .oneshot(json_request(
            &format!("/conversations/{}/commands?q=/skill:pro", record.id),
            &token,
        ))
        .await
        .expect("lookup");
    let body = text(response).await;
    assert!(
        commands(&body).contains(&"/skill:probe".to_owned()),
        "{body}"
    );
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(
        value["suggestions"][0]["scope"].as_str().expect("scope"),
        grant.alias
    );

    let response = app(&state)
        .oneshot(json_request(
            &format!(
                "/conversations/{}/commands?q=/skill:probe&mode=preview",
                record.id
            ),
            &token,
        ))
        .await
        .expect("preview");
    let body = text(response).await;
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    let source = value["preview"]["source"].as_str().expect("source");
    assert!(
        source.contains(&format!("/{}/.agents/skills/probe", grant.alias)),
        "{body}"
    );
    assert!(source.ends_with("/SKILL.md"), "{body}");
    assert_eq!(
        value["preview"]["base"].as_str().expect("base"),
        format!("/access/{}/.agents/skills/probe", grant.alias)
    );
    assert!(
        !source.contains(&root.path().display().to_string()),
        "{body}"
    );
    assert!(
        value["preview"]["expanded"]
            .as_str()
            .expect("expanded")
            .contains("Read the probe.")
    );
}

#[tokio::test]
async fn draft_scope_uses_only_submitted_grants() {
    let state = test_state();
    let token = connected(&state);
    let selected = project_fixture();

    let grant = DirectoryGrant::from_selected(selected.path(), &[]).expect("grant");
    let uri = format!(
        "/conversations/new/commands?q=/skill:pro&draft_nonce={}&directory_0={}",
        "a".repeat(64),
        form_value(&grant.form_value()),
    );
    let response = app(&state)
        .oneshot(json_request(&uri, &token))
        .await
        .expect("lookup");
    let body = text(response).await;
    assert!(
        commands(&body).contains(&"/skill:probe".to_owned()),
        "{body}"
    );
    let response = app(&state)
        .oneshot(json_request(
            &format!(
                "/conversations/new/commands?q=/skill:pro&draft_nonce={}",
                "a".repeat(64)
            ),
            &token,
        ))
        .await
        .expect("unselected root");
    assert!(commands(&text(response).await).is_empty());
}

#[tokio::test]
async fn queued_skill_keeps_its_frozen_expansion_after_a_source_edit() {
    let state = test_state();
    let token = connected(&state);
    let root = project_fixture();
    let (record, _grant) = conversation(&state, root.path());
    let skill_file = root.path().join(".agents/skills/probe/SKILL.md");

    let catalogue = super::catalogue_for_record(
        &state,
        session_id(&token),
        record.id,
        record.revision,
        record.model.as_ref(),
    );
    let expansion = super::expand("/skill:probe inspect", &catalogue, None, None).expect("expand");
    assert!(expansion.expanded.contains("Read the probe."));

    let queued = state
        .conversations
        .enqueue_with_attachments(
            &record.id,
            record.queue.revision,
            expansion.expanded.clone(),
            expansion.provenance.clone(),
            None,
            "",
            Vec::new(),
            crate::conversations::QueueDelivery::FollowUp,
            None,
        )
        .expect("queue");
    let frozen_hash = queued.queue.items[0]
        .input
        .as_ref()
        .expect("provenance")
        .source
        .content_hash
        .clone();

    std::fs::write(
        &skill_file,
        "---\nname: probe\ndescription: Inspect a probe target.\n---\n\n# Probe\n\nSecond body.\n",
    )
    .expect("rewrite");

    let reloaded = state.conversations.get(&record.id).expect("reload");
    let item = &reloaded.queue.items[0];
    assert!(item.text.contains("Read the probe."), "{}", item.text);
    assert!(!item.text.contains("Second body."));
    assert_eq!(
        item.input.as_ref().expect("provenance").source.content_hash,
        frozen_hash
    );
    let delivered = state
        .conversations
        .begin_follow_up(
            &record.id,
            reloaded.revision,
            reloaded.queue.revision,
            item.id,
            crate::sessions::JobId::generate().expect("job"),
            reloaded.model.clone(),
        )
        .expect("deliver frozen queue item");
    let user = &delivered.messages[delivered.messages.len() - 2];
    assert_eq!(user.text, expansion.expanded);
    assert_eq!(user.input, expansion.provenance);
}

#[tokio::test]
async fn project_skill_preview_rejects_links_replaced_roots_and_private_data() {
    let state = test_state();
    let root = project_fixture();
    let (record, grant) = conversation(&state, root.path());
    let token = connected(&state);
    let session = session_id(&token);
    let catalogue = || {
        super::catalogue_for_record(
            &state,
            session,
            record.id,
            record.revision,
            record.model.as_ref(),
        )
    };
    let file = root.path().join(".agents/skills/probe/SKILL.md");
    let outside = tempfile::NamedTempFile::new().expect("outside");
    std::fs::copy(&file, outside.path()).expect("copy");
    std::fs::remove_file(&file).expect("remove");
    std::os::unix::fs::symlink(outside.path(), &file).expect("link");
    assert!(catalogue().offers.is_empty());
    std::fs::remove_file(&file).expect("remove link");
    std::fs::copy(outside.path(), &file).expect("restore");
    let roots = [crate::execution::resources::EffectiveRoot {
        scope: grant.alias.clone(),
        model_path: "/access/project".to_owned(),
        host_path: Some(root.path().to_path_buf()),
        candidate_paths: Vec::new(),
    }];
    let (skills, _) = crate::execution::resources::preview_project_skills(
        &roots,
        std::slice::from_ref(&grant),
        &root.path().join(".agents"),
    );
    assert!(skills.is_empty());
    let original = root.path().join("original");
    let displaced = root.path().with_extension("displaced");
    std::fs::rename(root.path(), &displaced).expect("move root");
    std::fs::create_dir(root.path()).expect("replacement root");
    std::fs::rename(&displaced, &original).expect("retain original");
    std::fs::rename(original.join(".agents"), root.path().join(".agents"))
        .expect("replacement skill");
    assert!(catalogue().offers.is_empty());
}

#[tokio::test]
async fn lookup_supports_patch_and_document_representations() {
    let state = test_state();
    let token = connected(&state);
    seed_global_skill(&state);
    let record = conversation_with_grants(&state, Vec::new(), ToolLocation::Sandbox);

    let response = app(&state)
        .oneshot(patch_request(
            &format!("/conversations/{}/commands?q=/skill:co", record.id),
            &token,
        ))
        .await
        .expect("patch");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("conversation-command-suggestions"));
    assert!(body.contains("data-command-suggestion"));

    let response = app(&state)
        .oneshot(document(
            &format!("/conversations/{}/commands?q=/skill:co", record.id),
            &token,
        ))
        .await
        .expect("document");
    assert!(response.status().is_redirection());
}

#[tokio::test]
async fn saved_lookup_suggests_and_previews_prompt_templates() {
    let (state, _directory) = state_with_prompts();
    let token = connected(&state);
    let record = conversation_with_grants(&state, Vec::new(), ToolLocation::Sandbox);

    let response = app(&state)
        .oneshot(json_request(
            &format!("/conversations/{}/commands?q=/re", record.id),
            &token,
        ))
        .await
        .expect("lookup");
    let body = text(response).await;
    assert!(commands(&body).contains(&"/review".to_owned()), "{body}");
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(value["suggestions"][0]["kind"], "prompt");
    assert_eq!(value["suggestions"][0]["hint"], "<path> [<ref>]");

    let response = app(&state)
        .oneshot(json_request(
            &format!(
                "/conversations/{}/commands?q=/review&mode=preview",
                record.id
            ),
            &token,
        ))
        .await
        .expect("preview");
    let value: serde_json::Value = serde_json::from_str(&text(response).await).expect("json");
    assert_eq!(value["preview"]["kind"], "prompt");
    assert_eq!(value["preview"]["hint"], "<path> [<ref>]");
    assert_eq!(value["preview"]["source"], "prompts/review.md");
    assert_eq!(value["preview"]["base"], "");
    assert!(
        value["preview"]["hash"]
            .as_str()
            .expect("hash")
            .starts_with("sha256:")
    );
}

#[tokio::test]
async fn new_draft_lookup_suggests_prompt_templates() {
    let (state, _directory) = state_with_prompts();
    let token = connected(&state);
    let response = app(&state)
        .oneshot(json_request(
            &format!(
                "/conversations/new/commands?q=/re&draft_nonce={}",
                "a".repeat(64)
            ),
            &token,
        ))
        .await
        .expect("lookup");
    let body = text(response).await;
    assert!(commands(&body).contains(&"/review".to_owned()), "{body}");
}

#[tokio::test]
async fn project_prompt_uses_the_grant_alias_scope() {
    let state = test_state();
    let token = connected(&state);
    let root = prompt_fixture();
    let (record, grant) = conversation(&state, root.path());
    let response = app(&state)
        .oneshot(json_request(
            &format!("/conversations/{}/commands?q=/re", record.id),
            &token,
        ))
        .await
        .expect("lookup");
    let body = text(response).await;
    let prompts = prompt_suggestions(&body);
    assert_eq!(prompts.len(), 1, "{body}");
    assert_eq!(prompts[0]["command"], "/review");
    assert_eq!(prompts[0]["scope"], grant.alias);
    assert_eq!(prompts[0]["source_label"], grant.alias);
    assert_eq!(prompts[0]["description"], "Project review.");

    let response = app(&state)
        .oneshot(json_request(
            &format!(
                "/conversations/{}/commands?q=/review&mode=preview",
                record.id
            ),
            &token,
        ))
        .await
        .expect("preview");
    let value: serde_json::Value = serde_json::from_str(&text(response).await).expect("json");
    assert_eq!(value["preview"]["kind"], "prompt");
    assert_eq!(value["preview"]["scope"], grant.alias);
    let source = value["preview"]["source"].as_str().expect("source");
    assert!(
        source.contains(&format!("/{}/.agents/prompts/review.md", grant.alias)),
        "{source}"
    );
    assert!(
        !source.contains(&root.path().display().to_string()),
        "{source}"
    );
    assert!(
        value["preview"]["expanded"]
            .as_str()
            .expect("expanded")
            .contains("Project review")
    );
}

#[tokio::test]
async fn duplicate_global_and_project_prompt_names_require_a_scope() {
    let (state, _data) = state_with_prompts();
    let token = connected(&state);
    let root = prompt_fixture();
    let (record, grant) = conversation(&state, root.path());
    let response = app(&state)
        .oneshot(json_request(
            &format!("/conversations/{}/commands?q=/re", record.id),
            &token,
        ))
        .await
        .expect("lookup");
    let body = text(response).await;
    let prompts = prompt_suggestions(&body);
    assert_eq!(prompts.len(), 2, "{body}");
    for scope in ["global", grant.alias.as_str()] {
        let response = app(&state)
            .oneshot(json_request(
                &format!("/conversations/{}/commands?q=/{scope}/re", record.id),
                &token,
            ))
            .await
            .expect("qualified lookup");
        let body = text(response).await;
        let suggestions = commands(&body);
        assert_eq!(suggestions, vec![format!("/{scope}/review")], "{body}");
    }

    std::fs::write(
        root.path().join(".agents/prompts/broken.md"),
        "---\nunclosed",
    )
    .expect("invalid template");
    let response = app(&state)
        .oneshot(json_request(
            &format!("/conversations/{}/commands?q=/re", record.id),
            &token,
        ))
        .await
        .expect("partially available lookup");
    let value: serde_json::Value = serde_json::from_str(&text(response).await).expect("json");
    assert_eq!(value["unavailable"], 1);
    assert_eq!(
        value["message"],
        "Some command sources are not available for previews."
    );
    assert!(
        prompts
            .iter()
            .any(|suggestion| suggestion["command"] == "/global/review"),
        "{body}"
    );
    assert!(
        prompts
            .iter()
            .any(|suggestion| suggestion["command"] == format!("/{}/review", grant.alias)),
        "{body}"
    );

    let response = app(&state)
        .oneshot(json_request(
            &format!(
                "/conversations/{}/commands?q=/review&mode=preview",
                record.id
            ),
            &token,
        ))
        .await
        .expect("preview");
    let value: serde_json::Value = serde_json::from_str(&text(response).await).expect("json");
    assert!(value["preview"].is_null());
    assert!(
        value["message"]
            .as_str()
            .expect("message")
            .contains("More than one resource"),
        "{value}"
    );
}

fn host_conversation(state: &AppState, session: crate::sessions::SessionId) -> ConversationRecord {
    let settings = ExecutionSettings::new(
        ModelSelection::new(ProviderKind::Deepseek, "deepseek-chat".to_owned(), None)
            .expect("model"),
        String::new(),
        vec![crate::agents::ToolId::Run],
        crate::tests::test_environment_id(),
    )
    .expect("settings")
    .with_location(ToolLocation::Host)
    .with_host_approval(crate::execution::HostApprovalPolicy::Automatic);
    let record = state
        .conversations
        .create_saved(
            ConversationId::generate().expect("id"),
            Some("Host command".to_owned()),
            Some(ConversationModelConfiguration {
                settings: settings.clone(),
                preset: None,
            }),
            Vec::new(),
        )
        .expect("create");
    let request = state
        .access_consent
        .request_host_conversation(session, record.id, &settings)
        .expect("host request");
    state
        .access_consent
        .approve_host_conversation(&request, session, record.id, &settings)
        .expect("host approval");
    record
}

#[tokio::test]
async fn queue_rejects_direct_command_syntax() {
    let state = test_state();
    let token = connected(&state);
    let record = conversation_with_grants(&state, Vec::new(), ToolLocation::Sandbox);
    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/queue", record.id),
            &token,
            &format!(
                "queue_revision={}&message={}",
                record.queue.revision,
                form_value("!echo hi")
            ),
        ))
        .await
        .expect("queue");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        text(response)
            .await
            .contains("Direct commands run immediately"),
    );
    assert!(
        state
            .conversations
            .get(&record.id)
            .expect("conversation")
            .queue
            .items
            .is_empty()
    );
}

#[tokio::test]
async fn empty_command_is_rejected_without_an_entry() {
    let state = test_state();
    let token = connected(&state);
    let record = host_conversation(&state, session_id(&token));
    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/messages", record.id),
            &token,
            &format!("revision={}&message={}", record.revision, form_value("!")),
        ))
        .await
        .expect("send");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let reloaded = state.conversations.get(&record.id).expect("conversation");
    assert!(reloaded.messages.is_empty());
    assert!(reloaded.active_job.is_none());
}

#[tokio::test]
async fn direct_command_runs_without_a_provider_connection() {
    let state = test_state();
    let token = connected(&state);
    let record = host_conversation(&state, session_id(&token));
    state
        .vault
        .forget(ProviderKind::Xai)
        .expect("forget provider");
    assert!(!state.vault.has_providers());
    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/messages", record.id),
            &token,
            &format!(
                "revision={}&message={}",
                record.revision,
                form_value("!echo direct-sentinel")
            ),
        ))
        .await
        .expect("send");
    assert_eq!(response.status(), StatusCode::OK);
    for _ in 0..200 {
        let current = state.conversations.get(&record.id).expect("conversation");
        if current.active_job.is_none()
            && current
                .messages
                .iter()
                .any(|message| message.role == MessageRole::Command)
        {
            let entry = current
                .messages
                .iter()
                .find(|message| message.role == MessageRole::Command)
                .expect("command entry");
            assert!(
                entry
                    .command
                    .as_ref()
                    .and_then(|command| command.output.as_ref())
                    .is_some_and(|output| output
                        .chunks
                        .iter()
                        .any(|chunk| chunk.text.contains("direct-sentinel")))
            );
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("direct command did not settle");
}

#[tokio::test]
async fn a_direct_command_cannot_silently_replace_prompt_revision() {
    let state = test_state();
    let token = connected(&state);
    let record = host_conversation(&state, session_id(&token));
    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/messages", record.id),
            &token,
            &format!(
                "revision={}&revise_source=invalid&message={}",
                record.revision,
                form_value("!!echo excluded-sentinel")
            ),
        ))
        .await
        .expect("send");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        state.conversations.get(&record.id).expect("conversation"),
        record
    );
}

#[tokio::test]
async fn direct_command_rejects_a_second_submission_while_active() {
    let state = test_state();
    let token = connected(&state);
    let session = session_id(&token);
    let record = host_conversation(&state, session);
    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .expect("job");
    state
        .conversations
        .begin_command(
            &record.id,
            record.revision,
            None,
            job.id(),
            "echo first".to_owned(),
            true,
            "/".to_owned(),
        )
        .expect("first command");
    let revision = state
        .conversations
        .get(&record.id)
        .expect("current")
        .revision;
    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/messages", record.id),
            &token,
            &format!(
                "revision={}&message={}",
                revision,
                form_value("!echo second")
            ),
        ))
        .await
        .expect("send");
    assert_eq!(response.status(), StatusCode::CONFLICT);
}
