use std::path::PathBuf;

use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

use crate::{auth, router, StorePaths};

#[path = "../../../tests/support/golden.rs"]
mod golden;

async fn state(config: &std::path::Path) -> serde_json::Value {
    let (_tx, shutdown) = tokio::sync::watch::channel(false);
    let response = router(
        None,
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        StorePaths {
            telegram_config: config.to_path_buf(),
            ..Default::default()
        },
        std::time::Instant::now(),
        shutdown,
        auth::AuthState::localhost(),
    )
    .oneshot(
        Request::builder()
            .uri("/api/telegram")
            .body(Body::empty())
            .expect("a request"),
    )
    .await
    .expect("the router answers");
    assert_eq!(response.status(), 200);
    let body = axum::body::to_bytes(response.into_body(), 1 << 16)
        .await
        .expect("the body");
    serde_json::from_slice(&body).expect("JSON")
}

#[tokio::test]
async fn the_telegram_state_follows_the_config() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let config = dir.path().join("telegram.toml");
    assert_eq!(state(&config).await, serde_json::json!({ "state": "off" }));

    std::fs::write(&config, "token = \"123:abc\"\n").expect("write the config");
    assert_eq!(
        state(&config).await,
        serde_json::json!({ "state": "off" }),
        "a bot with no chat yet sends nothing"
    );

    std::fs::write(&config, "token = \"123:abc\"\nchat_id = 42\n").expect("write the config");
    let sends = state(&config).await;
    assert_eq!(sends, serde_json::json!({ "state": "sends" }));
    golden::check("api-telegram", sends, &[]);
}

/// The reply of `request` on a router over the config at `config`.
async fn reply_of(
    app: axum::Router,
    request: Request<Body>,
) -> (axum::http::StatusCode, serde_json::Value) {
    let response = app.oneshot(request).await.expect("the router answers");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 16)
        .await
        .expect("the body");
    (status, serde_json::from_slice(&body).unwrap_or_default())
}

#[tokio::test]
async fn a_claimed_peer_sends_through_its_main() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let config = dir.path().join("telegram.toml");
    std::fs::write(&config, "token = \"123:abc\"\nchat_id = 42\n").expect("write the config");
    let (_tx, shutdown) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        dir.path().join("repos.toml"),
        PathBuf::from("does-not-exist"),
        StorePaths {
            telegram_config: config,
            ..Default::default()
        },
        std::time::Instant::now(),
        shutdown,
        auth::AuthState::localhost(),
    );
    let get = |uri: &str| {
        Request::builder()
            .uri(uri)
            .body(Body::empty())
            .expect("a request")
    };
    let (_, before) = reply_of(app.clone(), get("/api/telegram")).await;
    assert_eq!(
        before,
        serde_json::json!({ "state": "sends" }),
        "its own bot"
    );

    let claim = Request::builder()
        .method("POST")
        .uri("/api/telegram/main")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"daemon_id":"m"}"#))
        .expect("a request");
    let (status, claimed) = reply_of(app.clone(), claim).await;
    assert_eq!(status, 200);
    golden::check("api-telegram-main", claimed, &[]);

    let through = serde_json::json!({ "state": "through-main", "main": "m" });
    let (_, joined) = reply_of(app.clone(), get("/api/telegram")).await;
    assert_eq!(joined, through, "the claim wins over its own bot");
    let (_, local) = reply_of(app, get("/api/telegram?local=1")).await;
    assert_eq!(local, through);
}
