//! End-to-end bind auth over a real loopback WebSocket (docs/adr/0032 §4; issue
//! #164): under a `Bearer` policy the daemon rejects an unauthenticated `/ws`
//! handshake with HTTP `401` and accepts one carrying `Authorization: Bearer
//! <tok>`, then pushes a live `Frame::Presence`. Proves the auth middleware
//! wraps the WS UPGRADE, not just the `/api` handlers — a slip here fails open.
//! Own file (single test) so no intra-process env race.

use std::path::PathBuf;
use std::time::Instant;

use futures_util::StreamExt;
use ralphy_daemon::auth::AuthPolicy;
use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::{identity, router};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

fn anvil() -> identity::Identity {
    identity::Identity {
        id: ulid::Ulid::nil(),
        name: "anvil".into(),
        avatar: "🐙".into(),
    }
}

#[tokio::test]
async fn bearer_policy_gates_the_ws_upgrade() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        Some(anvil()),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        Instant::now(),
        rx,
        AuthPolicy::Bearer("tok".into()),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // No `Authorization` header → the upgrade is rejected as HTTP 401, surfaced
    // by tungstenite as an `Http` error carrying the response.
    let url = format!("ws://127.0.0.1:{port}/ws");
    let err = tokio_tungstenite::connect_async(&url)
        .await
        .expect_err("an unauthenticated /ws handshake must be rejected");
    match err {
        WsError::Http(resp) => assert_eq!(
            resp.status(),
            401,
            "the unauthenticated upgrade must fail with 401"
        ),
        other => panic!("expected an HTTP 401 handshake error, got {other:?}"),
    }

    // With the correct bearer the upgrade succeeds and the first frame is a
    // live presence heartbeat.
    let mut req = url.into_client_request().unwrap();
    req.headers_mut()
        .insert("Authorization", "Bearer tok".parse().unwrap());
    let (mut ws, _resp) = tokio_tungstenite::connect_async(req)
        .await
        .expect("an authenticated /ws handshake must succeed");

    let msg = ws.next().await.expect("a message").expect("message is Ok");
    let bytes = match msg {
        Message::Binary(b) => b,
        other => panic!("expected a binary presence frame, got {other:?}"),
    };
    match protocol::decode(&bytes).expect("decodes to a frame") {
        Frame::Presence(p) => assert_eq!(p.name, Some("anvil".into())),
        other => panic!("expected a presence frame, got {other:?}"),
    }
}
