use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    middleware::from_fn_with_state,
};
use tower::ServiceExt;

use crate::{
    agents::{NetworkAccess, ToolId},
    config::RuntimeConfig,
    providers::{ModelSelection, ProviderConnection, ProviderKind},
    sessions,
    state::AppState,
};

pub(super) fn test_state() -> AppState {
    crate::tests::test_state(RuntimeConfig::development())
}

pub(super) fn app(state: &AppState) -> axum::Router {
    crate::slices::router()
        .layer(from_fn_with_state(
            state.clone(),
            crate::sessions::resolve_session,
        ))
        .layer(axum::middleware::from_fn(hypergraft::middleware::classify))
        .with_state(state.clone())
}

pub(super) fn connected(state: &AppState) -> String {
    state.environments.apply_production_seeds();
    state
        .vault
        .put(ProviderConnection::with_key(
            ProviderKind::Xai,
            "test-key",
            "grok-4.6",
        ))
        .expect("provider");
    let token = sessions::generate_session_token().expect("token");
    state.sessions.insert(token.id());
    token.raw().as_str().to_owned()
}

pub(super) async fn ready_starter_environment(state: &AppState) {
    state.environments.apply_production_seeds();
    let preparation = state
        .environments
        .claim_oldest_queued()
        .expect("claim starter")
        .expect("starter preparation");
    let snapshot = crate::tests::sample_snapshot(preparation.id);
    state.environment_snapshots.mark(
        snapshot.artifact_key.clone(),
        crate::environments::snapshot::SnapshotAvailability::Available,
    );
    state
        .environments
        .finish_ready(
            &preparation.id,
            snapshot,
            crate::environments::PreparationLogRecord::empty(),
        )
        .expect("ready starter");
}

pub(super) fn session_id(token: &str) -> sessions::SessionId {
    sessions::SessionId::from_validated(&sessions::ValidatedToken::parse(token).expect("token"))
}

pub(super) fn hidden_named(body: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\"");
    let tail = &body[body.find(&marker).expect(name) + marker.len()..];
    let value = &tail[tail.find("value=\"").expect("value") + 7..];
    value[..value.find('"').expect("value end")].to_owned()
}

pub(super) fn form_value(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            b' ' => encoded.push('+'),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn cookie(token: &str) -> String {
    format!("powerplant_session={token}")
}

fn candidate_run(
    state: &AppState,
) -> (
    crate::workflows::WorkflowRun,
    crate::workflows::artefacts::ArtefactReference,
) {
    let run_id = crate::workflows::RunId::generate().expect("run");
    let pinned = crate::workflows::pin_quick_task(
        crate::agents::AccessMode::ReadOnly,
        &[ToolId::List, ToolId::Read],
        "Review the selected candidate.",
        crate::tests::test_environment_id(),
    )
    .expect("workflow");
    let mut run = crate::workflows::WorkflowRun::create(
        run_id,
        1,
        Some(crate::agents::AgentId::generate().expect("agent")),
        crate::workflows::RunKind::Configured,
        pinned.clone(),
        crate::tests::test_environment_set(&pinned.definition),
    );
    let content = b"Review candidate files.";
    let file = state
        .workflow_artefacts
        .publish(content)
        .expect("file object");
    let candidate = crate::workflows::artefacts::candidate::CandidateRevisionArtefact {
        format_version: crate::workflows::artefacts::CANDIDATE_SCHEMA,
        candidate_hash: crate::workflows::artefacts::candidate::hash_entries(&[
            crate::workflows::artefacts::candidate::CandidateEntry {
                path: "AGENTS.md".to_owned(),
                kind: crate::workflows::artefacts::candidate::CandidateEntryKind::Regular {
                    executable: false,
                    mode: 0o644,
                    bytes: content.len() as u64,
                    blob: file,
                },
            },
        ]),
        ordinary: false,
        repository: Some(crate::workflows::artefacts::candidate::RepositoryAnchor {
            object_format: crate::workflows::artefacts::candidate::GitObjectFormat::Sha1,
            head: None,
        }),
        git_admin: Some(
            crate::workflows::artefacts::candidate::GitAdministrativeFingerprint::parse(
                &crate::workflows::artefacts::ObjectHash::of(b"git-admin").as_str(),
            )
            .expect("git fingerprint"),
        ),
        exclusions: Vec::new(),
        entries: vec![crate::workflows::artefacts::candidate::CandidateEntry {
            path: "AGENTS.md".to_owned(),
            kind: crate::workflows::artefacts::candidate::CandidateEntryKind::Regular {
                executable: false,
                mode: 0o644,
                bytes: content.len() as u64,
                blob: file,
            },
        }],
    };
    let bytes = candidate.manifest_bytes().expect("manifest");
    let object = state
        .workflow_artefacts
        .publish(&bytes)
        .expect("manifest object");
    let record = crate::workflows::artefacts::ArtefactRecord {
        id: crate::workflows::ArtefactId::generate().expect("artefact"),
        kind: crate::workflows::definition::ArtefactKind::CandidateRevision,
        artefact_hash: crate::workflows::artefacts::artefact_hash_for(
            crate::workflows::definition::ArtefactKind::CandidateRevision,
            candidate.format_version,
            &bytes,
        ),
        object_hash: object,
        payload_bytes: bytes.len() as u64,
        created_at_ms: 1,
        provenance: crate::workflows::artefacts::ArtefactProvenance {
            run_id,
            producer: crate::workflows::artefacts::ArtefactProducer::RunSourceCapture,
            inputs: Vec::new(),
        },
        summary: crate::workflows::artefacts::ArtefactSummary::Candidate {
            candidate: candidate.candidate_hash,
            entries: 1,
            bytes: content.len() as u64,
            disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
        },
    };
    let reference = crate::workflows::artefacts::ArtefactReference {
        id: record.id,
        kind: record.kind,
        artefact_hash: record.artefact_hash,
    };
    run.record_initial_candidate(record)
        .expect("initial candidate");
    state.workflow_runs.create(run.clone()).expect("run");
    (run, reference)
}

pub(super) fn document(path: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header(header::COOKIE, cookie(token))
        .body(Body::empty())
        .expect("request")
}

pub(super) fn navigation(path: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header(header::COOKIE, cookie(token))
        .header(hypergraft::GRAFT_REQUEST, "navigation")
        .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
        .body(Body::empty())
        .expect("request")
}

pub(super) fn command(path: &str, token: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header(header::COOKIE, cookie(token))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(hypergraft::GRAFT_REQUEST, "patch")
        .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
        .body(Body::from(body.to_owned()))
        .expect("request")
}

pub(super) async fn text(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body")
            .to_vec(),
    )
    .expect("text")
}

#[tokio::test]
async fn the_conversation_document_binds_the_thinking_visibility_patch_target() {
    let state = test_state();
    state.preferences.set_show_thinking(true).expect("thinking");
    let token = connected(&state);

    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/conversations/new")
                .header(header::COOKIE, cookie(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("settings");
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();

    let thinking_start = text.find("id=\"thinking-visibility\"").expect("thinking");
    let thinking_end = text[thinking_start..]
        .find("</form>")
        .map(|offset| thinking_start + offset)
        .expect("thinking form");
    let thinking_section = &text[thinking_start..thinking_end];
    assert!(thinking_section.contains("action=\"/thinking-visibility\""));
    assert!(thinking_section.contains("name=\"show_thinking\""));
    assert!(thinking_section.contains("checked"));
}

#[tokio::test]
async fn candidate_review_uses_immutable_selection_without_source_approval() {
    let mut state = test_state();
    let backend = crate::providers::tests::ScriptedBackend::accept();
    state.chat = std::sync::Arc::new(crate::providers::ChatBackend::Scripted(backend.clone()));
    let token = connected(&state);
    let (run, candidate) = candidate_run(&state);
    let path = format!(
        "/conversations/candidate-review?run={}&candidate={}&diff_base={}",
        run.id, candidate.id, candidate.id
    );
    let preview = app(&state)
        .oneshot(document(&path, &token))
        .await
        .expect("preview");
    assert_eq!(preview.status(), StatusCode::OK);

    let unknown = crate::workflows::ArtefactId::generate().expect("unknown artefact");
    let rejected = app(&state)
        .oneshot(command(
            "/conversations/candidate-review",
            &token,
            &format!(
                "run={}&candidate={}&diff_base={}&brief=Review&provider=xai&model=grok-4.6&thinking=medium",
                run.id, unknown, candidate.id
            ),
        ))
        .await
        .expect("candidate substitution");
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    assert!(
        text(rejected)
            .await
            .contains("selected candidate is unavailable")
    );
    assert!(state.conversations.list().is_empty());

    let started = app(&state)
        .oneshot(command(
            "/conversations/candidate-review",
            &token,
            &format!(
                "run={}&candidate={}&diff_base={}&brief={}&provider=xai&model=grok-4.6&thinking=medium",
                run.id,
                candidate.id,
                candidate.id,
                form_value("Review only this candidate."),
            ),
        ))
        .await
        .expect("start review");
    let started_status = started.status();
    let started_body = text(started).await;
    assert_eq!(started_status, StatusCode::OK, "{started_body}");
    let review = state
        .conversations
        .list()
        .pop()
        .expect("review conversation");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while state
            .conversations
            .get(&review.id)
            .expect("review")
            .active_job
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("review settlement");
    let history = backend.last_history();
    assert!(
        history
            .iter()
            .any(|turn| turn.text.contains("Review candidate files."))
    );
    assert!(
        history
            .iter()
            .any(|turn| turn.text.contains("Selected immutable candidate:"))
    );
    assert_eq!(state.workflow_runs.get(&run.id).expect("source"), run);
    let mut follow_up = state.conversations.get(&review.id).expect("review");
    if let Some(model) = follow_up.model.as_mut() {
        model.settings.tools = vec![crate::agents::ToolId::Read];
    }
    let result = super::start_message(
        &state,
        session_id(&token),
        follow_up.clone(),
        follow_up.revision,
        follow_up.model.expect("model"),
        "Read the host worktree instead.".to_owned(),
    )
    .await;
    assert!(matches!(result, Err(super::StartMessageError::User(
        hypergraft::PatchStatus::Conflict, message,
    )) if message.contains("immutable evidence")));
}

#[test]
fn candidate_review_rejects_changed_hashes_and_secret_instructions() {
    let state = test_state();
    let (run, candidate) = candidate_run(&state);
    let mut changed = candidate.clone();
    changed.artefact_hash = crate::workflows::artefacts::ArtefactHash::of(b"test", b"different");
    assert!(
        super::job::validate_candidate_review(&state, &run, &changed, &candidate, None,).is_err()
    );
    assert!(
        super::job::validate_candidate_review(
            &state,
            &run,
            &candidate,
            &candidate,
            Some("Review candidate files."),
        )
        .is_err()
    );
}

#[tokio::test]
async fn catalogue_uses_document_and_navigation_without_creating_a_conversation() {
    let state = test_state();
    let token = connected(&state);

    let document_response = app(&state)
        .oneshot(document("/conversations", &token))
        .await
        .expect("document");
    assert_eq!(document_response.status(), StatusCode::OK);
    let document_body = text(document_response).await;
    assert!(document_body.contains("Work history"));
    assert_eq!(document_body.matches("id=\"chat-main\"").count(), 1);

    let navigation_response = app(&state)
        .oneshot(navigation("/conversations", &token))
        .await
        .expect("navigation");
    assert_eq!(navigation_response.status(), StatusCode::OK);
    let navigation_body = text(navigation_response).await;
    assert!(navigation_body.contains("operation=\"children\" target=\"chat-main\""));
    assert!(state.conversations.list().is_empty());

    let new_page = app(&state)
        .oneshot(document("/conversations/new", &token))
        .await
        .expect("new page");
    assert_eq!(new_page.status(), StatusCode::OK);
    assert!(state.conversations.list().is_empty());
}

#[tokio::test]
async fn conversation_states_share_document_navigation_and_detail_patch_controls() {
    let state = test_state();
    let token = connected(&state);
    let record = state.conversations.create("Saved".to_owned()).unwrap();
    for (path, lifecycle, model_form) in [
        (
            "/conversations/new".to_owned(),
            "new",
            "conversation-composer",
        ),
        (
            format!("/conversations/{}", record.id),
            "saved",
            "conversation-model-form",
        ),
    ] {
        let patch = Request::builder()
            .uri(&path)
            .header(header::COOKIE, cookie(&token))
            .header("graft-request", "patch")
            .header(header::ACCEPT, "text/vnd.hypergraft.patches+html")
            .body(Body::empty())
            .unwrap();
        for (request, target) in [
            (document(&path, &token), None),
            (navigation(&path, &token), Some("chat-main")),
            (patch, Some("conversation-detail")),
        ] {
            let response = app(&state).oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = text(response).await;
            if let Some(target) = target {
                assert!(body.contains(&format!("target=\"{target}\"")));
            }
            if target != Some("conversation-detail") {
                assert_eq!(body.matches("data-island=\"conversation\"").count(), 1);
            }
            assert!(body.contains(&format!("data-conversation-state=\"{lifecycle}\"")));
            for id in [
                "transcript",
                "conversation-composer",
                "conversation-model-controls",
                "conversation-model-search",
                "conversation-settings",
            ] {
                assert_eq!(body.matches(&format!("id=\"{id}\"")).count(), 1);
            }
            assert!(body.contains(&format!("form=\"{model_form}\"")));
            assert!(!body.contains("formaction=\"\""));
            assert!(body.contains("data-conversation-model-catalogue=\"{&#34;xai&#34;:"));
            assert!(!body.contains("&#34;deepseek&#34;:"));
        }
    }
    assert_eq!(state.conversations.list(), vec![record]);
}

#[tokio::test]
async fn saved_model_commands_accept_disabled_effort_and_reject_stale_revision() {
    let state = test_state();
    let token = connected(&state);
    let record = state.conversations.create("Saved".to_owned()).unwrap();
    let model = state
        .models_dev
        .models(ProviderKind::Xai)
        .into_iter()
        .find(|model| {
            state
                .models_dev
                .efforts(ProviderKind::Xai, &model.id)
                .is_empty()
        })
        .unwrap();
    let path = format!("/conversations/{}/model", record.id);
    let fields = format!(
        "revision={}&provider=xai&model={}",
        record.revision,
        form_value(&model.id)
    );
    let response = app(&state)
        .oneshot(command(&path, &token, &fields))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("target=\"conversation-detail\""));
    let updated = state.conversations.get(&record.id).unwrap();
    assert_eq!(
        updated.model.as_ref().unwrap().settings.model.model,
        model.id
    );
    assert!(
        updated
            .model
            .as_ref()
            .unwrap()
            .settings
            .model
            .thinking
            .is_none()
    );
    let remembered = state
        .preferences
        .desk_providers(&state.vault)
        .into_iter()
        .find(|provider| provider.selected)
        .unwrap();
    assert_eq!(remembered.model, "grok-4.6");
    assert!(remembered.thinking.is_none());
    state
        .preferences
        .select_settings(ProviderKind::Xai, "grok-4.6".to_owned(), None)
        .unwrap();
    let response = app(&state)
        .oneshot(command(&path, &token, &fields))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(state.conversations.get(&record.id).unwrap(), updated);
    assert_eq!(
        state
            .preferences
            .desk_providers(&state.vault)
            .into_iter()
            .find(|provider| provider.selected)
            .unwrap()
            .model,
        "grok-4.6"
    );
}

#[tokio::test]
async fn local_model_selection_does_not_write_global_preferences() {
    let mut state = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.preferences = std::sync::Arc::new(crate::preferences::Preferences::open(
        dir.path().to_path_buf(),
    ));
    let token = connected(&state);
    let record = state.conversations.create("Saved".to_owned()).unwrap();
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, "grok-4.6", None)
        .unwrap();
    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/model", record.id),
            &token,
            &format!(
                "revision={}&provider=xai&model=grok-4.6&thinking={}",
                record.revision,
                effort.as_str()
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let updated = state.conversations.get(&record.id).unwrap();
    assert!(updated.revision > record.revision);
    assert_eq!(updated.model.unwrap().settings.model.model, "grok-4.6");
    let body = text(response).await;
    assert!(body.contains("target=\"conversation-detail\""));
    assert!(
        body.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains(&format!("name=\"revision\" value=\"{}\"", updated.revision))
    );
    assert!(!body.contains("Power Plant cannot store the model preference."));
}

#[tokio::test]
async fn rename_and_delete_use_independent_conversation_identity() {
    let state = test_state();
    let token = connected(&state);

    let record = state
        .conversations
        .create("Saved conversation".to_owned())
        .unwrap();
    let path = format!("/conversations/{}", record.id.as_hex());

    let title_projection = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("{path}?title=true"))
                .header(header::COOKIE, cookie(&token))
                .header("graft-request", "patch")
                .header(header::ACCEPT, "text/vnd.hypergraft.patches+html")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(title_projection.status(), StatusCode::OK);
    assert!(
        text(title_projection)
            .await
            .contains("target=\"conversation-heading\"")
    );

    let detail = app(&state)
        .oneshot(document(&path, &token))
        .await
        .expect("detail");
    assert_eq!(detail.status(), StatusCode::OK);
    let detail_body = text(detail).await;
    assert_eq!(detail_body.matches("id=\"conversation-detail\"").count(), 1);

    let rename = app(&state)
        .oneshot(command(
            &format!("{path}/rename"),
            &token,
            &format!("title=Renamed&revision={}", record.revision),
        ))
        .await
        .expect("rename");
    assert_eq!(rename.status(), StatusCode::OK);
    let rename_body = text(rename).await;
    assert!(rename_body.contains("target=\"conversation-detail\""));
    assert!(!rename_body.contains("id=\"conversation-detail\""));
    assert!(rename_body.contains("title=\"Renamed | Power Plant\""));
    let renamed = state.conversations.get(&record.id).expect("renamed");
    assert_eq!(renamed.title, "Renamed");
    assert_eq!(state.conversations.list(), vec![renamed.clone()]);

    let delete = app(&state)
        .oneshot(command(
            &format!("{path}/delete"),
            &token,
            &format!("revision={}", renamed.revision),
        ))
        .await
        .expect("delete");
    assert_eq!(delete.status(), StatusCode::OK);
    assert!(text(delete).await.contains("navigate=\"/conversations\""));
    assert!(state.conversations.get(&record.id).is_none());
}

#[tokio::test]
async fn rename_and_delete_reject_stale_revisions_without_state_change() {
    let state = test_state();
    let token = connected(&state);
    let record = state
        .conversations
        .create("Saved conversation".to_owned())
        .unwrap();
    let path = format!("/conversations/{}", record.id.as_hex());
    let stale = record.revision + 1;

    let rename = app(&state)
        .oneshot(command(
            &format!("{path}/rename"),
            &token,
            &format!("title=Renamed&revision={stale}"),
        ))
        .await
        .expect("stale rename");
    assert_eq!(rename.status(), StatusCode::CONFLICT);
    assert_eq!(
        state.conversations.get(&record.id).expect("record").title,
        "Saved conversation"
    );

    let delete = app(&state)
        .oneshot(command(
            &format!("{path}/delete"),
            &token,
            &format!("revision={stale}"),
        ))
        .await
        .expect("stale delete");
    assert_eq!(delete.status(), StatusCode::CONFLICT);
    assert!(state.conversations.get(&record.id).is_some());
}

#[tokio::test]
async fn conversation_actions_bind_commands_and_draft_copy_to_the_record() {
    let state = test_state();
    let token = connected(&state);
    let record = state
        .conversations
        .create("Saved conversation".to_owned())
        .unwrap();
    let path = format!("/conversations/{}", record.id.as_hex());
    let detail = app(&state)
        .oneshot(document(&path, &token))
        .await
        .expect("detail");
    assert_eq!(detail.status(), StatusCode::OK);
    let body = text(detail).await;
    let actions_start = body
        .find("id=\"conversation-actions\"")
        .expect("actions menu");
    let transcript_start = body.find("id=\"transcript\"").unwrap_or(body.len());
    let actions = &body[actions_start..transcript_start];
    // The draft copy carries source identity in Conversation actions, not Plans.
    let draft = format!("/conversations/new?source={}", record.id.as_hex());
    assert!(actions.contains(&draft));
    for action in ["rename", "delete"] {
        let form = actions
            .split("<form")
            .skip(1)
            .map(|form| form.split("</form>").next().expect("form"))
            .find(|form| form.contains(&format!("action=\"{path}/{action}\"")))
            .expect("record command");
        assert!(form.contains("method=\"post\""));
        assert!(
            normalised(form).contains(&format!("name=\"revision\" value=\"{}\"", record.revision))
        );
    }
}

fn normalised(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn awaiting_gate(state: &AppState) {
    use crate::workflows::definition::{InputKey, OutputKey, StepKey};
    let project_dir = tempfile::tempdir().expect("work dir");
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(project_dir.path())
            .status()
            .expect("git")
            .success()
    );
    std::fs::write(project_dir.path().join("notes.txt"), b"candidate\n").expect("source");
    let initial_capture = crate::workflows::artefacts::CandidateCapture::capture_host(
        project_dir.path(),
        &state.workflow_artefacts,
    )
    .expect("initial capture");
    std::fs::write(project_dir.path().join("notes.txt"), b"changed\n").expect("change");
    let produced_capture = crate::workflows::artefacts::CandidateCapture::capture_host(
        project_dir.path(),
        &state.workflow_artefacts,
    )
    .expect("changed capture");
    let conversation = state
        .conversations
        .create_saved(
            crate::conversations::ConversationId::generate().expect("id"),
            Some("Fix the timeout message".to_owned()),
            None,
            vec![],
        )
        .expect("record");
    let pinned = crate::workflows::pin_quick_task(
        crate::agents::AccessMode::ReadWrite,
        &[crate::agents::ToolId::List],
        "Fix the timeout message.",
        crate::tests::test_environment_id(),
    )
    .expect("quick task");
    let mut run = crate::workflows::WorkflowRun::create(
        crate::workflows::RunId::generate().expect("run"),
        1,
        None,
        crate::workflows::RunKind::QuickTask,
        pinned.clone(),
        crate::tests::test_environment_set(&pinned.definition),
    );
    run.conversation_id = Some(conversation.id);
    let gate_run_id = run.id;
    let publish = |captured: &crate::workflows::artefacts::candidate::CandidateRevisionArtefact,
                   producer: crate::workflows::artefacts::ArtefactProducer,
                   inputs: Vec<crate::workflows::artefacts::ArtefactReference>| {
        let bytes = captured.manifest_bytes().expect("manifest");
        let object = state.workflow_artefacts.publish(&bytes).expect("publish");
        crate::workflows::artefacts::ArtefactRecord {
            id: crate::workflows::ArtefactId::generate().expect("artefact"),
            kind: crate::workflows::definition::ArtefactKind::CandidateRevision,
            artefact_hash: crate::workflows::artefacts::artefact_hash_for(
                crate::workflows::definition::ArtefactKind::CandidateRevision,
                captured.format_version,
                &bytes,
            ),
            object_hash: object,
            payload_bytes: bytes.len() as u64,
            created_at_ms: 1,
            provenance: crate::workflows::artefacts::ArtefactProvenance {
                run_id: gate_run_id,
                producer,
                inputs,
            },
            summary: crate::workflows::artefacts::ArtefactSummary::Candidate {
                candidate: captured.candidate_hash,
                entries: captured.entries.len() as u64,
                bytes: 0,
                disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
            },
        }
    };
    let initial = publish(
        &initial_capture,
        crate::workflows::artefacts::ArtefactProducer::RunSourceCapture,
        Vec::new(),
    );
    let initial_ref = crate::workflows::artefacts::ArtefactReference {
        id: initial.id,
        kind: initial.kind,
        artefact_hash: initial.artefact_hash,
    };
    run.record_initial_candidate(initial).expect("initial");
    let work = StepKey::parse("work").expect("work");
    let attempt = crate::workflows::AttemptId::generate().expect("attempt");
    run.start_attempt(
        attempt,
        vec![crate::workflows::run::AttemptArtefactInput {
            key: InputKey::parse("candidate").expect("input"),
            artefact: initial_ref.clone(),
        }],
        crate::tests::test_agent_capabilities(),
        crate::workflows::run::AttemptSandboxRecord {
            kind: crate::workflows::run::AttemptSandboxKind::IsolatedAttempt,
            snapshot_digest: run
                .environments
                .steps
                .iter()
                .find(|binding| binding.step == work)
                .expect("work environment")
                .snapshot_digest
                .clone(),
        },
        2,
    )
    .expect("start");
    let produced = publish(
        &produced_capture,
        crate::workflows::artefacts::ArtefactProducer::StepAttempt {
            attempt_id: attempt,
            step: work.clone(),
            output: Some(OutputKey::parse("candidate").expect("output")),
            disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
        },
        vec![initial_ref.clone()],
    );
    let produced_ref = crate::workflows::artefacts::ArtefactReference {
        id: produced.id,
        kind: produced.kind,
        artefact_hash: produced.artefact_hash,
    };
    run.record_attempt_outputs(
        attempt,
        vec![produced],
        vec![crate::workflows::run::AttemptArtefactOutput {
            key: OutputKey::parse("candidate").expect("output"),
            artefact: produced_ref.clone(),
        }],
        Some(produced_ref.clone()),
        crate::workflows::run::ObservedCandidate::Exact {
            artefact: produced_ref.clone(),
        },
    )
    .expect("outputs");
    run.record_cleanup(
        attempt,
        crate::workflows::run::AttemptCleanupRecord::Complete,
    )
    .expect("cleanup");
    run.complete_attempt(attempt, 3).expect("complete work");
    run.open_gate(
        crate::workflows::GateId::generate().expect("gate"),
        produced_ref,
        initial_ref,
        4,
    )
    .expect("gate");
    state.workflow_runs.create(run).expect("store run");
}

#[tokio::test]
async fn directory_history_matches_identity_without_granting_access() {
    let state = test_state();
    let token = connected(&state);
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first/code");
    let second = root.path().join("second/code");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(&first, &[]).unwrap();
    let copied = crate::execution::DirectoryGrant::from_selected(&first, &[]).unwrap();
    assert_ne!(grant.id, copied.id);
    let other = crate::execution::DirectoryGrant::from_selected(&second, &[]).unwrap();
    let key = super::page::history_directory_key(&grant);
    for (title, directory) in [
        ("Original history", grant),
        ("Copied history", copied),
        ("Other history", other),
    ] {
        let mut model = crate::conversations::ConversationModelConfiguration::direct(
            ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None).unwrap(),
            crate::tests::test_environment_id(),
        );
        model.settings.directories = vec![directory];
        state
            .conversations
            .create_saved(
                crate::conversations::ConversationId::generate().unwrap(),
                Some(title.to_owned()),
                Some(model),
                Vec::new(),
            )
            .unwrap();
    }
    let path = format!("/conversations?directory={key}");
    let patch = Request::builder()
        .uri(&path)
        .header(header::COOKIE, cookie(&token))
        .header("graft-request", "patch")
        .header(header::ACCEPT, "text/vnd.hypergraft.patches+html")
        .body(Body::empty())
        .unwrap();
    for request in [document(&path, &token), navigation(&path, &token), patch] {
        let response = app(&state).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = text(response).await;
        let body = body.split("id=\"chat-main\"").last().unwrap();
        assert!(body.contains("Original history"));
        assert!(body.contains("Copied history"));
        assert!(!body.contains("Other history"));
        assert!(body.contains(first.to_str().unwrap()));
        assert!(body.contains(second.to_str().unwrap()));
        assert!(body.contains("href=\"/conversations/new\""));
        assert!(body.contains("id=\"conversation-directory-filter\""));
        assert!(body.contains("data-graft-submit-on=\"change\""));
    }
    std::fs::rename(&first, root.path().join("old-code")).unwrap();
    std::fs::create_dir(&first).unwrap();
    let replacement = crate::execution::DirectoryGrant::from_selected(&first, &[]).unwrap();
    assert_ne!(key, super::page::history_directory_key(&replacement));
    let body = text(app(&state).oneshot(document(&path, &token)).await.unwrap()).await;
    assert!(body.contains("Unavailable"));
    assert!(body.contains("Copied history"));
    for query in [
        "directory=invalid",
        "directory=0000000000000000-0000000000000000",
    ] {
        let response = app(&state)
            .oneshot(document(&format!("/conversations?{query}"), &token))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = text(response).await;
        let body = body.split("id=\"chat-main\"").last().unwrap();
        assert!(!body.contains("Original history"));
    }
    for query in ["directory=a&directory=b", "project=abc"] {
        assert_eq!(
            app(&state)
                .oneshot(document(&format!("/conversations?{query}"), &token))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(state.conversations.list().len(), 3);

    let mut records = state.conversations.list();
    let original = records
        .iter()
        .find(|record| record.title == "Original history")
        .unwrap()
        .clone();
    let mut moved = original.clone();
    let moved_path = root.path().join("old-code");
    moved.model.as_mut().unwrap().settings.directories =
        vec![crate::execution::DirectoryGrant::from_selected(&moved_path, &[]).unwrap()];
    for pair in [[original.clone(), moved.clone()], [moved, original]] {
        let pair: Vec<_> = pair.iter().map(|record| record.metadata()).collect();
        let view = super::page::CatalogueView::from_records(
            &state,
            &pair,
            &key,
            "",
            None,
            "",
            String::new(),
            "",
        );
        assert_eq!(view.conversations.len(), 2);
        assert_eq!(view.directories.len(), 1);
        assert_eq!(view.directories[0].name, moved_path.display().to_string());
    }

    let mut replacement_record = records[0].clone();
    replacement_record.id = crate::conversations::ConversationId::generate().unwrap();
    replacement_record.title = "Replacement history".to_owned();
    replacement_record
        .model
        .as_mut()
        .unwrap()
        .settings
        .directories = vec![replacement];
    records.push(replacement_record);
    let records: Vec<_> = records.iter().map(|record| record.metadata()).collect();
    let view = super::page::CatalogueView::from_records(
        &state,
        &records,
        &key,
        "",
        None,
        "",
        String::new(),
        "",
    );
    assert_eq!(view.conversations.len(), 2);
    assert!(
        view.conversations
            .iter()
            .all(|record| record.title != "Replacement history")
    );
}

#[tokio::test]
async fn history_lists_one_row_per_conversation_including_drafts() {
    let state = test_state();
    let draft = state
        .conversations
        .create("Tool-free draft".to_owned())
        .expect("draft");
    awaiting_gate(&state);
    // A second run in the same conversation must not duplicate its row.
    // Rows are built from the conversation list, never from the run list.
    let owner = state
        .conversations
        .list()
        .into_iter()
        .find(|record| record.id != draft.id)
        .expect("run owner");
    let run = state
        .workflow_runs
        .for_conversation(&owner.id)
        .into_iter()
        .next()
        .expect("run");
    let mut duplicate = run.clone();
    duplicate.id = crate::workflows::RunId::generate().expect("run id");
    state.workflow_runs.create(duplicate).expect("second run");
    let view = super::page::CatalogueView::from_records(
        &state,
        &state.conversations.metadata(),
        "",
        "",
        None,
        "",
        String::new(),
        "",
    );
    let mut destinations: Vec<_> = view
        .conversations
        .iter()
        .map(|row| row.href.as_str())
        .collect();
    destinations.sort_unstable();
    let mut expected = [
        format!("/conversations/{}", draft.id.as_hex()),
        format!("/conversations/{}", owner.id.as_hex()),
    ];
    expected.sort_unstable();
    assert_eq!(destinations, expected);
}

#[tokio::test]
async fn history_filters_preserve_only_valid_return_context() {
    let state = test_state();
    let token = connected(&state);
    let owner = state
        .conversations
        .create("Return context".to_owned())
        .expect("conversation");
    let id = owner.id.as_hex();
    for context in [
        id.as_str(),
        "https://example.com",
        "00000000000000000000000000000000",
    ] {
        let body = text(
            app(&state)
                .oneshot(document(
                    &format!(
                        "/conversations?q=no-match&conversation={}",
                        form_value(context)
                    ),
                    &token,
                ))
                .await
                .expect("history"),
        )
        .await;
        let body = normalised(&body);
        if context == id {
            assert!(body.contains(&format!("name=\"conversation\" value=\"{id}\"")));
            assert!(body.contains(&format!("href=\"/conversations?conversation={id}\"")));
            assert!(body.contains(&format!("href=\"/conversations/{id}\"")));
        } else {
            assert!(!body.contains("name=\"conversation\""));
            assert!(!body.contains("Back to conversation"));
        }
    }
}

#[tokio::test]
async fn send_persists_a_project_free_reply() {
    let state = test_state();
    let token = connected(&state);
    let record = state
        .conversations
        .create("Discussion".to_owned())
        .expect("conversation");
    let model = "grok-4.6".to_owned();
    let effort = state
        .models_dev
        .effective_effort(ProviderKind::Xai, &model, None);
    let selection = ModelSelection::new(ProviderKind::Xai, model, effort).expect("selection");
    let record = state
        .conversations
        .select_model(
            &record.id,
            record.revision,
            selection,
            crate::tests::test_environment_id(),
        )
        .expect("selection saved");
    let path = format!("/conversations/{}/messages", record.id.as_hex());

    let response = app(&state)
        .oneshot(command(
            &path,
            &token,
            &format!("revision={}&message=Hello", record.revision),
        ))
        .await
        .expect("send");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        text(response)
            .await
            .contains("target=\"conversation-detail\"")
    );

    for _ in 0..20 {
        let current = state.conversations.get(&record.id).expect("conversation");
        if current.active_job.is_none() {
            assert_eq!(current.messages.len(), 2);
            assert_eq!(current.messages[1].text, "Hello from Power Plant.");
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("reply did not settle");
}

#[tokio::test]
async fn observation_uses_the_page_route_and_cancel_needs_only_the_job_identity() {
    let state = test_state();
    let token = connected(&state);
    let owner = sessions::generate_session_token().expect("owner");
    state.sessions.insert(owner.id());
    let record = state
        .conversations
        .create("Discussion".to_owned())
        .expect("record");
    let job = state
        .sessions
        .begin_conversation_job(&owner.id(), record.id)
        .expect("job");
    let selection =
        ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None).expect("model");
    state
        .conversations
        .begin_message(
            &record.id,
            record.revision,
            selection,
            job.id(),
            "Question".to_owned(),
        )
        .expect("begin");
    let path = format!("/conversations/{}", record.id);
    let observe = format!("{path}?job={}&cursor=0", job.id());
    for request in [document(&observe, &token), navigation(&observe, &token)] {
        let response = app(&state).oneshot(request).await.expect("page");
        assert_eq!(response.status(), StatusCode::OK);
        let body = text(response).await;
        assert!(body.contains("Question"));
        assert!(body.contains(&format!("action=\"{path}/cancel\"")));
        assert!(body.contains("id=\"conversation-stop\""));
        assert!(body.contains(&format!("value=\"{}\"", job.id())));
    }
    let response = app(&state)
        .oneshot(command(
            &format!("{path}/cancel"),
            &token,
            &format!("job={}", job.id()),
        ))
        .await
        .expect("cancel");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(job.cancel_requested());
    state
        .conversations
        .settle_message(
            &record.id,
            job.id(),
            String::new(),
            crate::conversations::MessageStatus::Interrupted,
            None,
        )
        .expect("settle");
    state
        .sessions
        .finish_conversation_job(&owner.id(), record.id, job.id());
    let request = Request::builder()
        .uri(&observe)
        .header(header::COOKIE, cookie(&token))
        .header("Graft-Request", "patch")
        .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
        .body(Body::empty())
        .expect("request");
    let response = app(&state)
        .oneshot(request)
        .await
        .expect("final observation");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("target=\"conversation-detail\""));
    assert!(!body.contains("navigate="));
}

#[tokio::test]
async fn applied_preset_copies_model_and_instructions_without_directory_authority() {
    let mut state = test_state();
    let backend = crate::providers::tests::ScriptedBackend::accept();
    state.chat = std::sync::Arc::new(crate::providers::ChatBackend::Scripted(backend.clone()));
    let token = connected(&state);
    let conversation = state
        .conversations
        .create("Discussion".to_owned())
        .expect("conversation");
    let model = "grok-4.6".to_owned();
    let selection = ModelSelection::new(
        ProviderKind::Xai,
        model.clone(),
        state
            .models_dev
            .effective_effort(ProviderKind::Xai, &model, None),
    )
    .expect("selection");
    let settings = crate::execution::ExecutionSettings::new(
        selection.clone(),
        "Review only the supplied discussion.".to_owned(),
        Vec::new(),
        super::default_environment(&state).unwrap(),
    )
    .unwrap()
    .with_network(NetworkAccess::Public)
    .unwrap();
    let preset = state
        .presets
        .create(
            "Review preset",
            settings,
            crate::presets::PresetProvenance::Draft,
        )
        .expect("preset");
    let path = format!("/conversations/{}/settings/presets/apply", conversation.id);
    let owner = session_id(&token);
    let preview = state
        .presets
        .preview(
            owner,
            preset.id,
            crate::presets::PresetDestination::Conversation(conversation.id, conversation.revision),
        )
        .unwrap();

    let response = app(&state)
        .oneshot(command(
            &path,
            &token,
            &format!(
                "revision={}&preset_preview={}",
                conversation.revision, preview.token
            ),
        ))
        .await
        .expect("apply preset");
    assert_eq!(response.status(), StatusCode::OK);
    let updated = state.conversations.get(&conversation.id).expect("updated");
    let stale_preview = state
        .presets
        .preview(
            owner,
            preset.id,
            crate::presets::PresetDestination::Conversation(conversation.id, conversation.revision),
        )
        .unwrap();
    let response = app(&state)
        .oneshot(command(
            &path,
            &token,
            &format!(
                "revision={}&preset_preview={}",
                conversation.revision, stale_preview.token
            ),
        ))
        .await
        .expect("stale apply");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        state.conversations.get(&conversation.id).as_ref(),
        Some(&updated)
    );
    let model = updated.model.as_ref().expect("model configuration");
    assert_eq!(
        model.settings.instructions,
        "Review only the supplied discussion."
    );
    assert_eq!(model.settings.model, selection);
    let applied = model.preset.as_ref().expect("preset identity");
    assert_eq!((applied.id, applied.revision), (preset.id, preset.revision));
    assert_eq!(updated.id, conversation.id);

    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/messages", conversation.id),
            &token,
            &format!("revision={}&message=Hello", updated.revision),
        ))
        .await
        .expect("send");
    assert_eq!(response.status(), StatusCode::OK);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while state
            .conversations
            .get(&conversation.id)
            .expect("conversation")
            .active_job
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("settlement");
    assert!(
        backend
            .last_preamble()
            .unwrap()
            .starts_with("Review only the supplied discussion.")
    );
    assert_eq!(backend.last_tools(), vec![crate::tools::ASK_USER]);

    let concise_settings = crate::execution::ExecutionSettings::new(
        selection.clone(),
        "Reply briefly.".to_owned(),
        Vec::new(),
        super::default_environment(&state).unwrap(),
    )
    .unwrap();
    let instructions_only = state
        .presets
        .create(
            "Concise",
            concise_settings,
            crate::presets::PresetProvenance::Draft,
        )
        .expect("instructions-only preset");
    let current = state.conversations.get(&conversation.id).expect("current");
    let concise_preview = state
        .presets
        .preview(
            owner,
            instructions_only.id,
            crate::presets::PresetDestination::Conversation(current.id, current.revision),
        )
        .unwrap();
    let response = app(&state)
        .oneshot(command(
            &path,
            &token,
            &format!(
                "revision={}&preset_preview={}",
                current.revision, concise_preview.token
            ),
        ))
        .await
        .expect("switch preset");
    assert_eq!(response.status(), StatusCode::OK);
    let current = state.conversations.get(&conversation.id).expect("current");
    let model = current.model.as_ref().expect("model");
    assert_eq!(model.settings.model, selection);
    assert_eq!(model.settings.instructions, "Reply briefly.");
    assert_eq!(
        model.preset.as_ref().expect("preset").id,
        instructions_only.id
    );

    let response = app(&state)
        .oneshot(command(
            &format!("/conversations/{}/model", conversation.id),
            &token,
            &format!(
                "revision={}&provider=xai&model={}&thinking={}",
                current.revision,
                selection.model,
                selection
                    .thinking
                    .as_ref()
                    .map(|effort| effort.as_str())
                    .unwrap_or("")
            ),
        ))
        .await
        .expect("direct model");
    assert_eq!(response.status(), StatusCode::OK);
    let current = state.conversations.get(&conversation.id).expect("current");
    let model = current.model.expect("direct model");
    assert_eq!(model.settings.model, selection);
    assert_eq!(model.settings.instructions, "Reply briefly.");
    assert_eq!(model.preset.as_ref().unwrap().id, instructions_only.id);
}

#[tokio::test]
async fn unavailable_preset_models_apply_without_substitution() {
    let state = test_state();
    let token = connected(&state);
    let conversation = state
        .conversations
        .create("Discussion".to_owned())
        .expect("conversation");
    let selections = [
        ModelSelection::new(ProviderKind::Deepseek, "deepseek-chat".to_owned(), None).unwrap(),
        ModelSelection::new(ProviderKind::Xai, "missing-model".to_owned(), None).unwrap(),
        ModelSelection::new(
            ProviderKind::Xai,
            "grok-4.6".to_owned(),
            Some(crate::providers::ThinkingEffort::new("invalid".to_owned()).unwrap()),
        )
        .unwrap(),
    ];
    let owner = session_id(&token);
    let mut current = conversation;
    for selection in selections {
        let settings = crate::execution::ExecutionSettings::new(
            selection.clone(),
            String::new(),
            Vec::new(),
            super::default_environment(&state).unwrap(),
        )
        .unwrap();
        let preset = state
            .presets
            .create(
                "Unavailable",
                settings,
                crate::presets::PresetProvenance::Draft,
            )
            .expect("preset");
        let preview = state
            .presets
            .preview(
                owner,
                preset.id,
                crate::presets::PresetDestination::Conversation(current.id, current.revision),
            )
            .unwrap();
        let response = app(&state)
            .oneshot(command(
                &format!("/conversations/{}/settings/presets/apply", current.id),
                &token,
                &format!(
                    "revision={}&preset_preview={}",
                    current.revision, preview.token
                ),
            ))
            .await
            .expect("apply");
        assert_eq!(response.status(), StatusCode::OK);
        current = state.conversations.get(&current.id).unwrap();
        assert_eq!(current.model.as_ref().unwrap().settings.model, selection);
    }
}

#[tokio::test]
async fn stale_rename_returns_a_conflict_without_replacing_the_current_title() {
    let state = test_state();
    let token = connected(&state);
    let record = state
        .conversations
        .create("First discussion".to_owned())
        .expect("record");
    state
        .conversations
        .rename(&record.id, record.revision, "Current title".to_owned())
        .expect("current");
    let path = format!("/conversations/{}/rename", record.id.as_hex());

    let response = app(&state)
        .oneshot(command(
            &path,
            &token,
            &format!("title=Stale&revision={}", record.revision),
        ))
        .await
        .expect("rename");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = text(response).await;
    assert!(body.contains("target=\"conversation-detail\""));
    assert!(body.contains("Current title"));
    assert_eq!(
        state.conversations.get(&record.id).expect("current").title,
        "Current title"
    );
}

#[tokio::test]
async fn conversation_network_controls_are_bounded_revisioned_and_reserved() {
    let state = test_state();
    let token = connected(&state);
    let conversation = state
        .conversations
        .create("Network settings".to_owned())
        .expect("conversation");
    let path = format!("/conversations/{}/network", conversation.id);

    let invalid = app(&state)
        .oneshot(command(
            &path,
            &token,
            &format!(
                "revision={}&network=restricted&network_domains=",
                conversation.revision
            ),
        ))
        .await
        .expect("invalid network");
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        state.conversations.get(&conversation.id),
        Some(conversation.clone())
    );

    let public = app(&state)
        .oneshot(command(
            &path,
            &token,
            &format!(
                "revision={}&network=public&network_domains=",
                conversation.revision
            ),
        ))
        .await
        .expect("public network");
    assert_eq!(public.status(), StatusCode::OK);
    let body = text(public).await;
    assert!(body.contains("Public internet"));
    let updated = state.conversations.get(&conversation.id).expect("updated");
    assert_eq!(updated.network, NetworkAccess::Public);

    let stale = app(&state)
        .oneshot(command(
            &path,
            &token,
            &format!(
                "revision={}&network=none&network_domains=",
                conversation.revision
            ),
        ))
        .await
        .expect("stale network");
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(
        state.conversations.get(&conversation.id),
        Some(updated.clone())
    );

    let other = sessions::generate_session_token().expect("other session");
    state.sessions.insert(other.id());
    let _job = state
        .sessions
        .begin_conversation_job(&other.id(), conversation.id)
        .expect("reserved conversation");
    let reserved = app(&state)
        .oneshot(command(
            &path,
            &token,
            &format!(
                "revision={}&network=none&network_domains=",
                updated.revision
            ),
        ))
        .await
        .expect("reserved network");
    assert_eq!(reserved.status(), StatusCode::CONFLICT);
    assert_eq!(state.conversations.get(&conversation.id), Some(updated));
}

#[tokio::test]
async fn candidate_review_rejects_unknown_catalogue_choices() {
    let mut state = test_state();
    state.chat = std::sync::Arc::new(crate::providers::ChatBackend::Scripted(
        crate::providers::tests::ScriptedBackend::accept(),
    ));
    let token = connected(&state);
    let (run, candidate) = candidate_run(&state);
    let preview = app(&state)
        .oneshot(document(
            &format!(
                "/conversations/candidate-review?run={}&candidate={}&diff_base={}",
                run.id, candidate.id, candidate.id
            ),
            &token,
        ))
        .await
        .expect("preview");
    assert_eq!(preview.status(), StatusCode::OK);
    let _ = text(preview).await;

    for (model, effort, message) in [
        ("no-such-model", "medium", "Choose an available model."),
        (
            "grok-4.6",
            "invalid",
            "Choose an available thinking effort.",
        ),
    ] {
        let response = app(&state)
            .oneshot(command(
                "/conversations/candidate-review",
                &token,
                &format!(
                    "run={}&candidate={}&diff_base={}&brief={}&provider=xai&model={}&thinking={}",
                    run.id,
                    candidate.id,
                    candidate.id,
                    form_value("Review only this candidate."),
                    form_value(model),
                    form_value(effort),
                ),
            ))
            .await
            .expect("invalid reviewer");
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let rejected = text(response).await;
        assert!(rejected.contains(message), "{rejected}");
        assert!(rejected.contains(&run.id.as_hex()), "{rejected}");
        assert!(state.conversations.list().is_empty());
    }
}

#[tokio::test]
async fn catalogue_title_search_trims_case_and_combines_with_directory() {
    let state = test_state();
    let token = connected(&state);
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first");
    std::fs::create_dir_all(&first).unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(&first, &[]).unwrap();
    let key = super::page::history_directory_key(&grant);
    for (title, directory) in [
        ("Alpha Springfield", Some(grant.clone())),
        ("alpha beta", None),
        ("Gamma", Some(grant.clone())),
    ] {
        let mut model = crate::conversations::ConversationModelConfiguration::direct(
            ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None).unwrap(),
            crate::tests::test_environment_id(),
        );
        if let Some(directory) = directory {
            model.settings.directories = vec![directory];
        }
        state
            .conversations
            .create_saved(
                crate::conversations::ConversationId::generate().unwrap(),
                Some(title.to_owned()),
                Some(model),
                Vec::new(),
            )
            .unwrap();
    }
    let filtered = text(
        app(&state)
            .oneshot(document("/conversations?q=alpha", &token))
            .await
            .unwrap(),
    )
    .await;
    let filtered = filtered.split("id=\"chat-main\"").last().unwrap();
    assert!(filtered.contains("Alpha Springfield"));
    assert!(filtered.contains("alpha beta"));
    assert!(!filtered.contains("Gamma"));
    assert!(filtered.contains("value=\"alpha\""));
    assert!(filtered.contains("id=\"conversation-title-filter\""));

    let padded = text(
        app(&state)
            .oneshot(document("/conversations?q=++ALPHA++", &token))
            .await
            .unwrap(),
    )
    .await;
    let padded = padded.split("id=\"chat-main\"").last().unwrap();
    assert!(padded.contains("Alpha Springfield"));
    assert!(padded.contains("alpha beta"));
    assert!(!padded.contains("Gamma"));

    let combined = text(
        app(&state)
            .oneshot(document(
                &format!("/conversations?directory={key}&q=alpha"),
                &token,
            ))
            .await
            .unwrap(),
    )
    .await;
    let combined = combined.split("id=\"chat-main\"").last().unwrap();
    assert!(combined.contains("Alpha Springfield"));
    assert!(!combined.contains("alpha beta"));
    assert!(!combined.contains("Gamma"));

    let navigation = app(&state)
        .oneshot(navigation("/conversations?q=alpha", &token))
        .await
        .unwrap();
    assert_eq!(navigation.status(), StatusCode::OK);
    assert!(
        text(navigation)
            .await
            .contains("operation=\"children\" target=\"chat-main\"")
    );
    let patch = Request::builder()
        .uri("/conversations?q=alpha")
        .header(header::COOKIE, cookie(&token))
        .header("graft-request", "patch")
        .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
        .body(Body::empty())
        .unwrap();
    let patch = app(&state).oneshot(patch).await.unwrap();
    assert_eq!(patch.status(), StatusCode::OK);
    assert!(text(patch).await.contains("target=\"chat-main\""));

    let too_long = app(&state)
        .oneshot(document(
            &format!("/conversations?q={}", "a".repeat(257)),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(too_long.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(text(too_long).await.contains("Search is too long"));

    assert_eq!(
        app(&state)
            .oneshot(document("/conversations?title=alpha", &token))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(state.conversations.list().len(), 3);
}

fn seeded_history(
    state: &AppState,
    title: &str,
    count: usize,
) -> crate::conversations::ConversationRecord {
    let selection =
        ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None).expect("selection");
    let mut record = state
        .conversations
        .create(title.to_owned())
        .expect("conversation");
    for index in 0..count {
        let job = sessions::JobId::generate().expect("job");
        record = state
            .conversations
            .begin_message(
                &record.id,
                record.revision,
                selection.clone(),
                job,
                format!("Question {index}"),
            )
            .expect("begin");
        state
            .conversations
            .settle_message(
                &record.id,
                job,
                format!("Reply {index}"),
                crate::conversations::MessageStatus::Complete,
                None,
            )
            .expect("settle");
        record = state.conversations.get(&record.id).expect("record");
    }
    record
}

#[tokio::test]
async fn transcript_cursors_expose_bounded_history_across_representations() {
    let state = test_state();
    let token = connected(&state);
    let record = seeded_history(&state, "History", 70);
    let base = format!("/conversations/{}", record.id.as_hex());

    let document_body = text(app(&state).oneshot(document(&base, &token)).await.unwrap()).await;
    assert!(!document_body.contains(&super::page::message_id(&record.id, &record.messages[0])));
    assert!(document_body.contains(&super::page::message_id(
        &record.id,
        record.messages.last().unwrap()
    )));
    assert!(normalised(&document_body).contains("remain in local history and model context"));

    let (_, window) = state
        .conversations
        .transcript_window(&record.id, None)
        .unwrap()
        .unwrap();
    let anchor = window.before_anchor.expect("earlier anchor");
    let earlier_path = format!("{base}?before={}", anchor.as_hex());
    let earlier = normalised(
        &text(
            app(&state)
                .oneshot(document(&earlier_path, &token))
                .await
                .unwrap(),
        )
        .await,
    );
    assert!(earlier.contains(&format!("{base}?before=")));
    assert!(earlier.contains(&format!("{base}?after=")));
    assert!(!earlier.contains(&super::page::message_id(
        &record.id,
        record.messages.last().unwrap()
    )));

    let navigation = app(&state)
        .oneshot(navigation(&earlier_path, &token))
        .await
        .unwrap();
    assert_eq!(navigation.status(), StatusCode::OK);
    let navigation = normalised(&text(navigation).await);
    assert!(navigation.contains("target=\"chat-main\""));
    assert!(navigation.contains("Earlier"));
}

#[tokio::test]
async fn transcript_cursors_reject_invalid_foreign_and_incompatible_requests() {
    let state = test_state();
    let token = connected(&state);
    let record = seeded_history(&state, "History", 3);
    let other = seeded_history(&state, "Other", 2);
    let base = format!("/conversations/{}", record.id.as_hex());
    let first = record.messages[0].id.as_hex();
    let foreign = other.messages[0].id.as_hex();
    for (path, message) in [
        (
            format!("{base}?before={first}&after={first}"),
            "one transcript position",
        ),
        (format!("{base}?before=nothex"), "not valid"),
        (
            format!("{base}?around={first}&title=true"),
            "Observation cannot use a transcript position",
        ),
        (
            format!("{base}?around={first}&cursor=1"),
            "Observation cannot use a transcript position",
        ),
        (
            format!("{base}?around={first}&historical=true"),
            "Observation cannot use a transcript position",
        ),
        (
            format!("{base}?before={foreign}"),
            "not part of this conversation",
        ),
        (
            format!("{base}?job=00000000000000000000000000000000&before={first}"),
            "Observation cannot use a transcript position",
        ),
    ] {
        let response = app(&state).oneshot(document(&path, &token)).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{path}"
        );
        assert!(
            normalised(&text(response).await).contains(message),
            "{path}"
        );
    }
}

#[tokio::test]
async fn expired_historical_observation_keeps_the_transcript_window() {
    let state = test_state();
    let token = connected(&state);
    let record = seeded_history(&state, "History", 1);
    let mut request = document(
        &format!(
            "/conversations/{}?job={}&historical=true",
            record.id.as_hex(),
            sessions::JobId::generate().unwrap().as_hex()
        ),
        &token,
    );
    request
        .headers_mut()
        .insert(hypergraft::GRAFT_REQUEST, "patch".parse().unwrap());
    request
        .headers_mut()
        .insert(header::ACCEPT, hypergraft::MEDIA_TYPE.parse().unwrap());
    let response = app(&state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("target=\"conversation-history-status\""));
    assert!(body.contains("target=\"conversation-observe\""));
    assert!(!body.contains("target=\"conversation-detail\""));
    assert!(!body.contains("target=\"transcript\""));
}

#[tokio::test]
async fn catalogue_pages_every_conversation_with_stable_keys() {
    let state = test_state();
    let token = connected(&state);
    for index in 0..55 {
        state
            .conversations
            .create(format!("History {index:02}"))
            .expect("conversation");
    }
    state
        .conversations
        .create("Special title".to_owned())
        .expect("special");

    let first = text(
        app(&state)
            .oneshot(document("/conversations", &token))
            .await
            .unwrap(),
    )
    .await;
    let first = normalised(&first);
    let visible_ids = |body: &str| -> std::collections::BTreeSet<_> {
        let body = body
            .split("aria-label=\"Saved conversations\"")
            .nth(1)
            .unwrap_or("")
            .split("</ul>")
            .next()
            .unwrap();
        state
            .conversations
            .metadata()
            .into_iter()
            .filter(|record| {
                body.contains(&format!("href=\"/conversations/{}\"", record.id.as_hex()))
            })
            .map(|record| record.id)
            .collect()
    };
    let first_ids = visible_ids(&first);
    assert_eq!(first_ids.len(), 50);
    assert!(first.contains("Older conversations"));
    let cursor = first
        .split("cursor=")
        .nth(1)
        .expect("cursor")
        .split('"')
        .next()
        .expect("cursor value")
        .to_owned();
    let second = text(
        app(&state)
            .oneshot(document(&format!("/conversations?cursor={cursor}"), &token))
            .await
            .unwrap(),
    )
    .await;
    let second = normalised(&second);
    let second_ids = visible_ids(&second);
    assert_eq!(second_ids.len(), 6);
    assert!(first_ids.is_disjoint(&second_ids));
    assert!(!second.contains("Older conversations"));
    assert!(second.contains("Newest first"));

    // Filters apply before the page bound, so a match never hides behind it.
    let filtered = text(
        app(&state)
            .oneshot(document("/conversations?q=Special", &token))
            .await
            .unwrap(),
    )
    .await;
    let filtered = normalised(&filtered);
    assert_eq!(visible_ids(&filtered).len(), 1);
    assert!(filtered.contains("Special title"));
    assert!(!filtered.contains("Older conversations"));
}
