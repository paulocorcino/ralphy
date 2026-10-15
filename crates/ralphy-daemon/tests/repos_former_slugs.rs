//! `GET /api/repos` serves the keys a project had before it was re-keyed, so a
//! host can follow the rename (ADR-0036 amendment 2026-10-15). A project that
//! was never re-keyed has no such field.

use std::time::Instant;

use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use ralphy_daemon::{registry, router};
use tower::ServiceExt;

#[tokio::test]
async fn api_repos_serves_former_slugs_only_for_a_rekeyed_project() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert("path-abc", "/no/such/renamed");
    store.rekey("path-abc", "owner/renamed");
    store.upsert("owner/plain", "/no/such/plain");
    registry::save_to(&store, &registry_path).unwrap();

    let app = router(
        None,
        registry_path,
        std::path::PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        tokio::sync::watch::channel(false).1,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/repos")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    let row = |slug: &str| {
        rows.iter()
            .find(|r| r["slug"] == slug)
            .unwrap_or_else(|| panic!("no row for {slug}: {rows:?}"))
    };
    assert_eq!(
        row("owner/renamed")["former_slugs"],
        serde_json::json!(["path-abc"])
    );
    assert!(
        row("owner/plain").get("former_slugs").is_none(),
        "the field is omitted when empty: {rows:?}"
    );
}
