//! A page socket outlives the request the guard checked. Each `channel.open`
//! reads the login again: after a logout, an open is refused and the socket
//! ends, so the page connects again through the guard.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ralphy_daemon::auth::AuthState;

use crate::support::{ends_within, next_closed, open, Closed, Ws};

const WITHIN: Duration = Duration::from_secs(5);

/// A daemon served on `port` with its store in a temporary directory.
struct Served {
    port: u16,
    _store: tempfile::TempDir,
}

/// Serve a router whose auth state the test keeps, so that it can log every
/// session out as `/api/logout` does.
async fn serve_with_auth(auth: Arc<AuthState>) -> Served {
    let dir = tempfile::tempdir().expect("create a temporary store");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let port = listener
        .local_addr()
        .expect("read the bound address")
        .port();
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let app = ralphy_daemon::router(
        None,
        dir.path().join("repos.toml"),
        dir.path().join("usage"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        shutdown,
        auth,
    );
    tokio::spawn(async move {
        // The shutdown stays open while the server runs.
        let _stop = stop;
        axum::serve(listener, app)
            .await
            .expect("serve the test app");
    });
    Served { port, _store: dir }
}

/// Connect a page socket to the daemon's `/ws`.
async fn connect(daemon: &Served) -> Ws {
    let url = format!("ws://127.0.0.1:{}/ws", daemon.port);
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect to /ws");
    ws
}

/// Open a `spawn` channel whose command no verb serves: a served open gets
/// the command's error and ends `done`.
async fn open_spawn(ws: &mut Ws, channel: u32) {
    let command = serde_json::json!({ "id": 5, "verb": "no.such.verb", "payload": {} });
    open(
        ws,
        channel,
        "spawn",
        serde_json::json!({ "command": command }),
    )
    .await;
}

#[tokio::test]
async fn an_open_after_a_logout_is_refused_and_ends_the_socket() {
    let auth = AuthState::localhost();
    let daemon = serve_with_auth(auth.clone()).await;
    let mut ws = connect(&daemon).await;
    auth.invalidate_sessions().expect("bump the session epoch");

    open_spawn(&mut ws, 1).await;
    let closed = next_closed(&mut ws, WITHIN).await;
    assert_eq!(
        closed,
        Some(Closed {
            channel: 1,
            reason: "refused".to_string()
        })
    );
    assert!(ends_within(&mut ws, WITHIN).await, "the socket ends");
}

/// The control: with no logout, the same open reaches its kind.
#[tokio::test]
async fn an_open_with_the_same_login_is_served() {
    let auth = AuthState::localhost();
    let daemon = serve_with_auth(auth).await;
    let mut ws = connect(&daemon).await;

    open_spawn(&mut ws, 1).await;
    let closed = next_closed(&mut ws, WITHIN).await;
    assert_eq!(
        closed,
        Some(Closed {
            channel: 1,
            reason: "done".to_string()
        })
    );
    assert!(!ends_within(&mut ws, Duration::from_secs(1)).await);
}
