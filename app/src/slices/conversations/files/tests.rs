use std::path::Path;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use tower::ServiceExt;

use super::super::tests::{app, connected, document, form_value, test_state, text};
use super::page::search;
use crate::{
    conversations::{ConversationId, ConversationModelConfiguration},
    execution::{DirectoryGrant, ExecutionSettings, ToolLocation},
    providers::{ModelSelection, ProviderKind},
    state::AppState,
};

fn conversation_with_grant(
    state: &AppState,
    directory: &Path,
    location: ToolLocation,
) -> (crate::conversations::ConversationRecord, DirectoryGrant) {
    let grant = DirectoryGrant::from_selected(directory, &[]).expect("grant");
    let record = conversation_with_grants(state, vec![grant.clone()], location);
    (record, grant)
}

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
            Some("Files test".to_owned()),
            Some(ConversationModelConfiguration {
                settings,
                preset: None,
            }),
            Vec::new(),
        )
        .expect("create")
}

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("root");
    std::fs::create_dir_all(root.path().join("src")).expect("src");
    std::fs::write(root.path().join("src/main.rs"), b"fn main() {}").expect("main");
    std::fs::write(root.path().join("src/lib.rs"), b"pub fn lib() {}").expect("lib");
    std::fs::write(root.path().join("README.md"), b"# Read me").expect("readme");
    root
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

fn suggestions(body: &str) -> Vec<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_str(body).expect("json");
    value["suggestions"]
        .as_array()
        .expect("suggestions")
        .clone()
}

#[tokio::test]
async fn sandbox_lookup_returns_scoped_alias_paths_without_contents() {
    let state = test_state();
    let token = connected(&state);
    let root = fixture();
    let (record, grant) = conversation_with_grant(&state, root.path(), ToolLocation::Sandbox);
    let response = app(&state)
        .oneshot(json_request(
            &format!("/conversations/{}/files?q=main", record.id),
            &token,
        ))
        .await
        .expect("lookup");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    let suggestions = suggestions(&body);
    assert_eq!(suggestions.len(), 1);
    assert_eq!(
        suggestions[0]["path"],
        format!("/access/{}/src/main.rs", grant.alias)
    );
    assert_eq!(suggestions[0]["scope"], grant.alias);
    assert!(!body.contains("fn main"));
}

#[tokio::test]
async fn host_lookup_returns_host_paths_for_selected_work_locations() {
    let state = test_state();
    let token = connected(&state);
    let root = fixture();
    let (record, _) = conversation_with_grant(&state, root.path(), ToolLocation::Host);
    let response = app(&state)
        .oneshot(json_request(
            &format!("/conversations/{}/files?q=main", record.id),
            &token,
        ))
        .await
        .expect("lookup");
    let body = text(response).await;
    let suggestions = suggestions(&body);
    assert_eq!(suggestions.len(), 1);
    assert_eq!(
        suggestions[0]["path"],
        format!(
            "{}/src/main.rs",
            root.path().canonicalize().expect("canonical").display()
        )
    );
}

#[tokio::test]
async fn an_unavailable_root_is_explained_without_hiding_a_valid_root() {
    let state = test_state();
    let token = connected(&state);
    let valid = fixture();
    let missing = tempfile::tempdir().expect("missing");
    let missing_grant = DirectoryGrant::from_selected(missing.path(), &[]).expect("missing grant");
    drop(missing);
    let valid_grant = DirectoryGrant::from_selected(valid.path(), &[]).expect("valid grant");
    let record = conversation_with_grants(
        &state,
        vec![missing_grant.clone(), valid_grant.clone()],
        ToolLocation::Sandbox,
    );
    let response = app(&state)
        .oneshot(json_request(
            &format!("/conversations/{}/files?q=main", record.id),
            &token,
        ))
        .await
        .expect("lookup");
    let body = text(response).await;
    let suggestions = suggestions(&body);
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0]["scope"], valid_grant.alias);
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert!(
        value["message"]
            .as_str()
            .expect("message")
            .contains("approval"),
        "{body}"
    );
}

#[tokio::test]
async fn traversal_rejects_symlink_escape_and_git_internals() {
    let state = test_state();
    let token = connected(&state);
    let root = fixture();
    std::fs::create_dir_all(root.path().join(".git")).expect("git");
    std::fs::write(root.path().join(".git/config"), b"secret").expect("config");
    let outside = tempfile::tempdir().expect("outside");
    std::fs::write(outside.path().join("escape.txt"), b"outside").expect("escape file");
    std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).expect("symlink");
    let (record, _) = conversation_with_grant(&state, root.path(), ToolLocation::Sandbox);
    for query in ["config", "escape", "escape.txt"] {
        let response = app(&state)
            .oneshot(json_request(
                &format!("/conversations/{}/files?q={query}", record.id),
                &token,
            ))
            .await
            .expect("lookup");
        let body = text(response).await;
        assert!(
            suggestions(&body).is_empty(),
            "unexpected match for {query}"
        );
    }
}

#[tokio::test]
async fn draft_lookup_labels_each_submitted_grant_with_its_scope() {
    let state = test_state();
    let token = connected(&state);
    let base = tempfile::tempdir().expect("base");
    let selected = base.path().join("selected");
    let ignored = base.path().join("ignored");
    for directory in [&selected, &ignored] {
        std::fs::create_dir_all(directory.join("src")).expect("src");
        std::fs::write(directory.join("src/main.rs"), b"fn main() {}").expect("main");
    }
    let selected_grant = DirectoryGrant::from_selected(&selected, &[]).expect("grant");
    let ignored_grant = DirectoryGrant::from_selected(&ignored, &[]).expect("ignored");
    let uri = format!(
        "/conversations/new/files?q=main&draft_nonce={}&directory_0={}&directory_1={}",
        "a".repeat(64),
        form_value(&selected_grant.form_value()),
        form_value(&ignored_grant.form_value()),
    );
    let response = app(&state)
        .oneshot(json_request(&uri, &token))
        .await
        .expect("lookup");
    let body = text(response).await;
    let suggestions = suggestions(&body);
    assert_eq!(suggestions.len(), 2);
    let mut scopes = suggestions
        .iter()
        .map(|suggestion| suggestion["scope"].as_str().expect("scope"))
        .collect::<Vec<_>>();
    scopes.sort_unstable();
    let mut expected = vec![selected_grant.alias.as_str(), ignored_grant.alias.as_str()];
    expected.sort_unstable();
    assert_eq!(scopes, expected);
    assert!(suggestions.iter().all(|suggestion| {
        let scope = suggestion["scope"].as_str().expect("scope");
        let path = suggestion["path"].as_str().expect("path");
        path.starts_with(&format!("/access/{scope}/"))
    }));
}

#[tokio::test]
async fn lookup_supports_patch_and_document_representations() {
    let state = test_state();
    let token = connected(&state);
    let root = fixture();
    let (record, _) = conversation_with_grant(&state, root.path(), ToolLocation::Sandbox);

    let response = app(&state)
        .oneshot(patch_request(
            &format!("/conversations/{}/files?q=main", record.id),
            &token,
        ))
        .await
        .expect("patch");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("conversation-file-suggestions"));
    assert!(body.contains("data-file-path"));

    let response = app(&state)
        .oneshot(document(
            &format!("/conversations/{}/files?q=main", record.id),
            &token,
        ))
        .await
        .expect("document");
    assert!(response.status().is_redirection());
}

#[tokio::test]
async fn invalid_query_text_is_rejected_without_a_search() {
    let state = test_state();
    let token = connected(&state);
    let root = fixture();
    let (record, _) = conversation_with_grant(&state, root.path(), ToolLocation::Sandbox);
    let response = app(&state)
        .oneshot(json_request(
            &format!("/conversations/{}/files?q={}", record.id, "x".repeat(201)),
            &token,
        ))
        .await
        .expect("lookup");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[test]
fn live_search_bounds_work_even_without_matches() {
    use crate::execution::resources::{EffectiveRoot, MAXIMUM_FILE_TRAVERSAL_ENTRIES};
    let directory = tempfile::tempdir().unwrap();
    for index in 0..=MAXIMUM_FILE_TRAVERSAL_ENTRIES {
        std::fs::write(directory.path().join(format!("file-{index}")), "").unwrap();
    }
    let grant = crate::execution::DirectoryGrant::from_selected(directory.path(), &[]).unwrap();
    let root = EffectiveRoot {
        scope: grant.alias.clone(),
        model_path: grant.guest_path(),
        host_path: Some(grant.host_path.clone()),
    };
    let result = search(&[root], "absent", Path::new("/private-data"), &[grant]);
    assert!(result.partial);
    assert!(result.suggestions.is_empty());
}

#[tokio::test]
async fn replaced_root_does_not_reuse_its_grant() {
    let state = test_state();
    let token = connected(&state);
    let base = tempfile::tempdir().expect("base");
    let root = base.path().join("root");
    std::fs::create_dir(&root).expect("root");
    let (record, _) = conversation_with_grant(&state, &root, ToolLocation::Host);
    std::fs::rename(&root, base.path().join("old")).expect("move root");
    std::fs::create_dir(&root).expect("replacement");
    std::fs::write(root.join("secret.txt"), b"secret").expect("file");
    let response = app(&state)
        .oneshot(json_request(
            &format!("/conversations/{}/files?q=secret", record.id),
            &token,
        ))
        .await
        .expect("lookup");
    assert!(suggestions(&text(response).await).is_empty());
}

#[tokio::test]
async fn prefix_completion_returns_files_and_directories_with_separators() {
    let state = test_state();
    let token = connected(&state);
    let root = fixture();
    std::fs::create_dir_all(root.path().join("src/nested")).expect("nested");
    std::fs::write(root.path().join("src/nested/deep.rs"), b"").expect("deep");
    std::fs::write(root.path().join("src/nested/my file.rs"), b"").expect("spaced file");
    std::fs::write(root.path().join("src/nested/myother.rs"), b"").expect("other file");
    let (record, grant) = conversation_with_grant(&state, root.path(), ToolLocation::Sandbox);
    let base = format!("/access/{}", grant.alias);
    for (query, expected) in [
        ("src/ma", vec![format!("{base}/src/main.rs")]),
        ("src/MA", vec![format!("{base}/src/main.rs")]),
        (
            "src/",
            vec![
                format!("{base}/src/lib.rs"),
                format!("{base}/src/main.rs"),
                format!("{base}/src/nested/"),
            ],
        ),
        ("src/ne", vec![format!("{base}/src/nested/")]),
        (
            "src/nested/my ",
            vec![format!("{base}/src/nested/my file.rs")],
        ),
    ] {
        let response = app(&state)
            .oneshot(json_request(
                &format!(
                    "/conversations/{}/files?q={}&mode=complete",
                    record.id,
                    form_value(query)
                ),
                &token,
            ))
            .await
            .expect("lookup");
        assert_eq!(response.status(), StatusCode::OK);
        let body = text(response).await;
        let mut paths = suggestions(&body)
            .iter()
            .map(|value| value["path"].as_str().expect("path").to_owned())
            .collect::<Vec<_>>();
        paths.sort();
        assert_eq!(paths, expected, "query {query}");
    }
}

#[tokio::test]
async fn completion_rejects_parent_traversal_and_foreign_roots() {
    let state = test_state();
    let token = connected(&state);
    let root = fixture();
    let (record, grant) = conversation_with_grant(&state, root.path(), ToolLocation::Sandbox);

    for query in [
        "../",
        "src/../../etc",
        "/access/parent/../other",
        "src//",
        &format!("/access/{}//", grant.alias),
    ] {
        let response = app(&state)
            .oneshot(json_request(
                &format!(
                    "/conversations/{}/files?q={}&mode=complete",
                    record.id,
                    form_value(query)
                ),
                &token,
            ))
            .await
            .expect("lookup");
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{query}"
        );
    }

    // A string prefix that changes the root identity must not match the grant.
    let foreign = format!("/access/{}-other/src/", grant.alias);
    let response = app(&state)
        .oneshot(json_request(
            &format!(
                "/conversations/{}/files?q={}&mode=complete",
                record.id,
                form_value(&foreign)
            ),
            &token,
        ))
        .await
        .expect("lookup");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(suggestions(&body).is_empty(), "{body}");
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert!(
        value["message"]
            .as_str()
            .expect("message")
            .contains("outside"),
        "{body}"
    );
}

#[tokio::test]
async fn completion_of_a_missing_directory_is_empty_without_an_error() {
    let state = test_state();
    let token = connected(&state);
    let root = fixture();
    let (record, _) = conversation_with_grant(&state, root.path(), ToolLocation::Sandbox);
    let response = app(&state)
        .oneshot(json_request(
            &format!(
                "/conversations/{}/files?q={}&mode=complete",
                record.id,
                form_value("src/missing/ma")
            ),
            &token,
        ))
        .await
        .expect("lookup");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(suggestions(&body).is_empty(), "{body}");
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(value["message"], "");
}

#[tokio::test]
async fn completion_maps_absolute_host_paths_to_the_selected_root() {
    let state = test_state();
    let token = connected(&state);
    let root = fixture();
    let (record, _) = conversation_with_grant(&state, root.path(), ToolLocation::Host);
    let canonical = root.path().canonicalize().expect("canonical");
    let query = format!("{}/src/ma", canonical.display());
    let response = app(&state)
        .oneshot(json_request(
            &format!(
                "/conversations/{}/files?q={}&mode=complete",
                record.id,
                form_value(&query)
            ),
            &token,
        ))
        .await
        .expect("lookup");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    let suggestions = suggestions(&body);
    assert_eq!(suggestions.len(), 1);
    assert_eq!(
        suggestions[0]["path"],
        format!("{}/src/main.rs", canonical.display())
    );
}

#[test]
fn live_completion_returns_directories_and_rejects_parent() {
    use crate::execution::resources::EffectiveRoot;
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("src/nested")).unwrap();
    std::fs::write(directory.path().join("src/main.rs"), "").unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(directory.path(), &[]).unwrap();
    let root = EffectiveRoot {
        scope: grant.alias.clone(),
        model_path: "/access/project".into(),
        host_path: Some(grant.host_path.clone()),
    };
    let grants = [grant];
    let data = Path::new("/private-data");
    let result = super::page::complete(std::slice::from_ref(&root), "src/ne", data, &grants)
        .expect("complete");
    assert_eq!(result.suggestions.len(), 1);
    assert_eq!(result.suggestions[0].path, "/access/project/src/nested/");
    assert!(result.suggestions[0].directory);

    let result = super::page::complete(
        std::slice::from_ref(&root),
        "/access/project/src/ma",
        data,
        &grants,
    )
    .expect("complete");
    assert_eq!(result.suggestions.len(), 1);
    assert_eq!(result.suggestions[0].path, "/access/project/src/main.rs");
    assert!(!result.suggestions[0].directory);

    let result = super::page::complete(
        std::slice::from_ref(&root),
        "/access/project-other/src/ma",
        data,
        &grants,
    )
    .expect("complete");
    assert!(result.foreign);
    assert!(result.suggestions.is_empty());

    assert!(super::page::complete(&[root], "src/../other", data, &[]).is_err());
}
