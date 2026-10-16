//! A peer poll that carries a key the registry has since re-keyed is served, not
//! refused (ADR-0036 amendment 2026-10-15). The host's poller reads its key once,
//! so after a rename it asks with the old one until its tab closes.

use std::time::Instant;

use axum::body::Body;
use axum::http::{header, Request};
use http_body_util::BodyExt;
use ralphy_daemon::{registry, router};
use tower::ServiceExt;

async fn poll(registry_path: std::path::PathBuf, repo: &str) -> serde_json::Value {
    let (tx, rx) = tokio::sync::watch::channel(false);
    std::mem::forget(tx);
    let app = router(
        None,
        registry_path,
        std::path::PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    let body = serde_json::json!({
        "sub": "s1", "repo": repo, "paths": [""], "runs": false, "timeout_ms": 50
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/peer/tree/poll")
                .header(header::HOST, "127.0.0.1:1")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("the router must answer");
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&bytes)))
}

#[tokio::test]
async fn a_peer_poll_with_a_former_slug_is_served_like_the_current_key() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    std::fs::create_dir(&root).unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert("path-abc", &root.to_string_lossy());
    store.rekey("path-abc", "owner/repo");
    registry::save_to(&store, &registry_path).unwrap();

    let current = poll(registry_path.clone(), "owner/repo").await;
    let former = poll(registry_path.clone(), "path-abc").await;
    assert_ne!(current["message"], "unknown repo", "{current}");
    assert_eq!(former, current, "the old key answers as the new one does");

    let stranger = poll(registry_path, "path-zzz").await;
    assert_eq!(stranger["message"], "unknown repo", "{stranger}");
}
