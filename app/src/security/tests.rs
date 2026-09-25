use super::*;
use crate::{config::RuntimeConfig, state::AppState};
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use tower::ServiceExt;

fn test_state(config: RuntimeConfig) -> AppState {
    crate::tests::test_state(config)
}

#[test]
fn normalise_accepts_canonical_origin() {
    assert_eq!(
        normalise_origin("https://frinkworks.example.com").as_deref(),
        Some("https://frinkworks.example.com")
    );
    assert_eq!(
        normalise_origin("http://localhost:4000").as_deref(),
        Some("http://localhost:4000")
    );
}

#[test]
fn normalise_lowercases_domain_host() {
    assert_eq!(
        normalise_origin("https://FRINKWORKS.Example.COM").as_deref(),
        Some("https://frinkworks.example.com")
    );
}

#[test]
fn normalise_rejects_null_origin() {
    assert_eq!(normalise_origin("null"), None);
}

#[test]
fn normalise_rejects_non_http_schemes() {
    assert_eq!(normalise_origin("file:///etc/hosts"), None);
    assert_eq!(normalise_origin("ws://localhost:4000"), None);
}

#[test]
fn normalise_rejects_origin_with_path_query_or_fragment() {
    assert_eq!(
        normalise_origin("https://frinkworks.example.com/connect"),
        None
    );
    assert_eq!(normalise_origin("https://frinkworks.example.com?x=1"), None);
    assert_eq!(
        normalise_origin("https://frinkworks.example.com#frag"),
        None
    );
}

#[test]
fn normalise_rejects_origin_with_credentials() {
    assert_eq!(
        normalise_origin("https://user:pass@frinkworks.example.com"),
        None
    );
}

#[test]
fn csp_has_no_unsafe_directives() {
    let policy = content_security_policy(None, "ws://localhost:4000/_hypergraft/live");
    let policy = policy.to_str().unwrap();
    assert!(!policy.contains("unsafe-inline"), "{policy}");
    assert!(!policy.contains("unsafe-eval"), "{policy}");
    assert!(policy.contains("default-src 'self'"));
    assert!(policy.contains("frame-ancestors 'none'"));
    assert!(policy.contains("object-src 'none'"));
    assert!(policy.contains("img-src 'self' data:"));
    assert!(policy.contains("script-src 'self'"));
    assert!(policy.contains("style-src 'self'"));
    assert!(policy.contains("connect-src 'self' ws://localhost:4000/_hypergraft/live"));
    let connect = policy
        .split(';')
        .find(|directive| directive.trim_start().starts_with("connect-src"))
        .unwrap();
    assert!(
        connect
            .split_whitespace()
            .all(|source| !matches!(source, "ws:" | "wss:"))
    );
    assert!(policy.contains("form-action 'self'"));
    assert!(policy.contains("base-uri 'self'"));
    assert!(policy.contains("require-trusted-types-for 'script'"));
    assert!(policy.contains("trusted-types hypergraft"));
}

#[test]
fn nonce_policy_adds_only_the_nonce_to_script_src() {
    let nonce = CspNonce("test-nonce".to_owned());
    let policy = content_security_policy(Some(&nonce), "ws://localhost:4000/_hypergraft/live");
    let policy = policy.to_str().unwrap();
    assert!(
        policy.contains("script-src 'nonce-test-nonce' 'self'"),
        "{policy}"
    );
    assert!(!policy.contains("unsafe-inline"), "{policy}");
    assert!(!policy.contains("unsafe-eval"), "{policy}");
}

#[test]
fn generated_nonces_are_unique_and_base64url() {
    let a = CspNonce::generate();
    let b = CspNonce::generate();
    assert_ne!(a.as_str(), b.as_str());
    assert_eq!(a.as_str().len(), 22);
    assert!(
        a.as_str()
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    );
}

#[tokio::test]
async fn unsafe_method_without_origin_is_forbidden() {
    let app = axum::Router::new()
        .route("/", axum::routing::post(|| async { "ok" }))
        .layer(axum::middleware::from_fn_with_state(
            test_state(RuntimeConfig::development()),
            enforce_origin,
        ))
        .with_state(test_state(RuntimeConfig::development()));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn unsafe_method_with_foreign_origin_is_forbidden() {
    let app = axum::Router::new()
        .route("/", axum::routing::post(|| async { "ok" }))
        .layer(axum::middleware::from_fn_with_state(
            test_state(RuntimeConfig::development()),
            enforce_origin,
        ))
        .with_state(test_state(RuntimeConfig::development()));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(header::ORIGIN, "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn unsafe_method_with_public_origin_passes() {
    let app = axum::Router::new()
        .route("/", axum::routing::post(|| async { "ok" }))
        .layer(axum::middleware::from_fn_with_state(
            test_state(RuntimeConfig::development()),
            enforce_origin,
        ))
        .with_state(test_state(RuntimeConfig::development()));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(header::ORIGIN, "http://localhost:4000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn get_skips_origin_check() {
    let app = axum::Router::new()
        .route("/", axum::routing::get(|| async { "ok" }))
        .layer(axum::middleware::from_fn_with_state(
            test_state(RuntimeConfig::development()),
            enforce_origin,
        ))
        .with_state(test_state(RuntimeConfig::development()));

    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
