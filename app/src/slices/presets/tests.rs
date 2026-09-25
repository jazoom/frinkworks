use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
    middleware::from_fn_with_state,
};
use tower::ServiceExt;

#[test]
fn preset_destinations_validate_identity_without_application_or_preview_tokens() {
    use askama::Template;
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let conversation = state
        .conversations
        .create("Destination".to_owned())
        .unwrap();
    let preset = state
        .presets
        .create(
            "Reusable",
            form().settings(None).unwrap(),
            PresetProvenance::Draft,
        )
        .unwrap();
    for (context, valid) in [
        (conversation.id.as_hex(), true),
        (String::new(), false),
        ("0123456789abcdef0123456789abcdef".to_owned(), false),
        ("https://example.com/evil".to_owned(), false),
    ] {
        let page = page::PresetsPage::new(&state, PresetForm::default(), "", false)
            .with_destination(&state, &context, &preset.id.as_hex())
            .render()
            .unwrap();
        assert_eq!(page.contains(">Back to conversation<"), valid);
        assert!(page.contains("Preview Reusable"));
        assert!(page.contains(&conversation.id.as_hex()));
        assert!(!page.contains("example.com"));
        assert!(!page.contains("name=\"preset_preview\""));
        assert!(!page.contains("settings/presets/preview"));
        assert!(!page.contains("settings/presets/apply"));
    }
    let stale = page::PresetsPage::new(&state, PresetForm::default(), "", false)
        .with_destination(&state, "", "0123456789abcdef0123456789abcdef")
        .render()
        .unwrap();
    assert!(stale.contains("That preset is no longer available."));
    assert!(!stale.contains("Choose this conversation"));
    assert_eq!(state.conversations.list().len(), 1);
    assert_eq!(
        state.conversations.get(&conversation.id).unwrap().revision,
        conversation.revision
    );
}

#[tokio::test]
async fn routes_support_navigation_and_patch_commands_only() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let token = crate::sessions::generate_session_token().unwrap();
    state.sessions.insert(token.id());
    let app = crate::slices::router()
        .layer(from_fn_with_state(
            state.clone(),
            crate::sessions::resolve_session,
        ))
        .layer(axum::middleware::from_fn(hypergraft::middleware::classify))
        .with_state(state.clone());
    for (kind, status) in [
        (None, StatusCode::OK),
        (Some("navigation"), StatusCode::OK),
        (Some("patch"), StatusCode::BAD_REQUEST),
    ] {
        let mut request = Request::builder().uri("/presets").header(
            header::COOKIE,
            format!("frinkworks_session={}", token.raw().as_str()),
        );
        if let Some(kind) = kind {
            request = request
                .header(hypergraft::GRAFT_REQUEST, kind)
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE);
        }
        assert_eq!(
            app.clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status(),
            status
        );
    }
    for path in ["/presets/create", "/presets/edit", "/presets/delete"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header(
                        header::COOKIE,
                        format!("frinkworks_session={}", token.raw().as_str()),
                    )
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("name=Forged"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    assert!(state.presets.list().is_empty());
    let record = state
        .presets
        .create(
            "Unavailable",
            form().settings(None).unwrap(),
            PresetProvenance::Draft,
        )
        .unwrap();
    let changed = state
        .presets
        .update(
            record.id,
            record.revision,
            "Updated",
            record.settings.clone(),
        )
        .unwrap();
    for (revision, expected) in [
        (record.revision, StatusCode::CONFLICT),
        (changed.revision, StatusCode::OK),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/presets/delete")
                    .header(hypergraft::GRAFT_REQUEST, "patch")
                    .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!(
                        "preset_id={}&revision={revision}",
                        record.id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    assert!(state.presets.get(&record.id).is_none());
}

#[tokio::test]
async fn invalid_and_stale_edits_leave_the_saved_snapshot_unchanged() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let record = state
        .presets
        .create(
            "Original",
            form().settings(None).unwrap(),
            PresetProvenance::Draft,
        )
        .unwrap();
    let app = crate::slices::router()
        .layer(from_fn_with_state(
            state.clone(),
            crate::sessions::resolve_session,
        ))
        .layer(axum::middleware::from_fn(hypergraft::middleware::classify))
        .with_state(state.clone());
    for (fields, expected) in [
        ("name=Changed&revision=0".to_owned(), StatusCode::CONFLICT),
        (
            format!("name={}&revision=1", "x".repeat(96 * 1024)),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "name=Changed&revision=1&unknown=value".to_owned(),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/presets/edit")
                    .header(hypergraft::GRAFT_REQUEST, "patch")
                    .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!("preset_id={}&{fields}", record.id)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(state.presets.get(&record.id), Some(record.clone()));
    }
}

async fn post_form(app: &axum::Router, uri: &str, body: String) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(hypergraft::GRAFT_REQUEST, "patch")
                .header(header::ACCEPT, hypergraft::MEDIA_TYPE)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn capable_models_need_a_listed_effort_while_unlisted_models_stay_retained() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let app = crate::slices::router()
        .layer(from_fn_with_state(
            state.clone(),
            crate::sessions::resolve_session,
        ))
        .layer(axum::middleware::from_fn(hypergraft::middleware::classify))
        .with_state(state.clone());
    let environment = EnvironmentId::generate().unwrap().as_hex();
    let create = |model: &str, thinking: &str| {
        format!(
            "name=Effort&provider=xai&model={model}&thinking={thinking}&environment={environment}&network=none"
        )
    };
    // grok-4.6 is catalogue-listed with adjustable effort, so an empty value
    // never selects a default silently.
    assert_eq!(
        post_form(&app, "/presets/create", create("grok-4.6", "")).await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        post_form(&app, "/presets/create", create("grok-4.6", "nonsense")).await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        post_form(&app, "/presets/create", create("grok-4.6", "medium")).await,
        StatusCode::OK
    );
    // Unlisted requested values stay retained without substitution.
    assert_eq!(
        post_form(&app, "/presets/create", create("retired-model-1", ""),).await,
        StatusCode::OK
    );
    let retained = state
        .presets
        .list()
        .into_iter()
        .find(|record| record.settings.model.model == "retired-model-1")
        .expect("retained preset");
    assert_eq!(
        post_form(
            &app,
            "/presets/edit",
            format!(
                "preset_id={}&revision={}&name=Renamed&provider=xai&model=retired-model-1&environment={environment}&network=none",
                retained.id,
                retained.revision,
            ),
        )
        .await,
        StatusCode::OK
    );
    let edited = state.presets.get(&retained.id).expect("edited preset");
    assert_eq!(edited.settings.model.model, "retired-model-1");
    assert_eq!(edited.settings.model.thinking, None);
}

fn form() -> PresetForm {
    PresetForm {
        name: "Review".to_owned(),
        provider: "xai".to_owned(),
        model: "grok-4.6".to_owned(),
        thinking: "medium".to_owned(),
        environment: EnvironmentId::generate().unwrap().as_hex(),
        network: "none".to_owned(),
        ..PresetForm::default()
    }
}

#[test]
fn form_rejects_unknown_duplicate_tools_and_unbounded_instructions() {
    let mut f = form();
    f.tool_read = "unknown".to_owned();
    assert!(f.settings(None).is_err());
    f.tool_read = "read".to_owned();
    f.tool_run = "read".to_owned();
    assert!(f.settings(None).is_err());
    f.tool_run.clear();
    f.instructions = "x".repeat(32769);
    assert!(f.settings(None).is_err());
}

#[test]
fn overlapping_directory_roots_fail() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("source");
    let child = parent.join("nested");
    std::fs::create_dir_all(&child).unwrap();
    let mut f = form();
    f.read_only = parent.display().to_string();
    f.direct_write = child.display().to_string();
    assert!(f.settings(None).is_err());
    f.direct_write.clear();
    f.direct_write = f.read_only.clone();
    assert!(f.settings(None).is_err());
}

#[test]
fn edits_keep_directory_order_and_avoid_alias_collisions() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("one/src");
    let second = temp.path().join("two/src");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    let mut f = form();
    f.direct_write = first.display().to_string();
    let store = crate::presets::PresetStore::in_memory();
    let record = store
        .create(
            "Sources",
            f.settings(None).unwrap(),
            PresetProvenance::Draft,
        )
        .unwrap();
    f.read_only = second.display().to_string();
    let edited = f.settings(Some(&record)).unwrap();
    assert_eq!(edited.directories[0], record.settings.directories[0]);
    assert_ne!(edited.directories[0].alias, edited.directories[1].alias);
}

#[test]
fn directory_paths_retain_trailing_spaces_without_retargeting() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("source ");
    std::fs::create_dir(&path).unwrap();
    std::fs::create_dir(temp.path().join("source")).unwrap();
    let mut f = form();
    f.read_only = path.display().to_string();
    let store = crate::presets::PresetStore::in_memory();
    let original = store
        .create(&f.name, f.settings(None).unwrap(), PresetProvenance::Draft)
        .unwrap();
    assert_eq!(original.settings.directories[0].host_path, path);
    std::fs::remove_dir(&path).unwrap();
    let edited = PresetForm::from(&original)
        .settings(Some(&original))
        .unwrap();
    assert_eq!(edited.directories, original.settings.directories);
    assert!(!edited.directories[0].is_available());
}

#[test]
fn unavailable_directory_keeps_identity_and_duplicate_roots_fail() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("source");
    std::fs::create_dir(&path).unwrap();
    let mut f = form();
    f.read_only = path.display().to_string();
    let settings = f.settings(None).unwrap();
    let store = crate::presets::PresetStore::in_memory();
    let original = store
        .create(&f.name, settings, PresetProvenance::Draft)
        .unwrap();
    std::fs::remove_dir(&path).unwrap();
    assert_eq!(f.settings(Some(&original)).unwrap(), original.settings);
    std::fs::create_dir(&path).unwrap();
    assert_eq!(
        f.settings(Some(&original)).unwrap().directories[0].identity,
        original.settings.directories[0].identity
    );
    f.direct_write = f.read_only.clone();
    assert!(f.settings(Some(&original)).is_err());
}
