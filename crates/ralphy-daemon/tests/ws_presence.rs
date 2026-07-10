//! End-to-end presence over a real loopback WebSocket (docs/adr/0032 §4/§5):
//! a client connects to `/ws` and the daemon's first binary message decodes to
//! a live `Frame::Presence` carrying the identity's name and process uptime.
//! Uses a real socket (not the in-process `oneshot` router) because the upgrade
//! + heartbeat loop is what this slice adds — cross-platform loopback only.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::{identity, router};
use tokio_tungstenite::tungstenite::Message;

fn anvil() -> identity::Identity {
    identity::Identity {
        id: ulid::Ulid::nil(),
        name: "anvil".into(),
        avatar: "🐙".into(),
    }
}

/// Read one binary frame and decode it to a `Presence`.
async fn next_presence<S>(ws: &mut S) -> protocol::Presence
where
    S: futures_util::Stream<Item = tokio_tungstenite::tungstenite::Result<Message>> + Unpin,
{
    let msg = ws.next().await.expect("a message").expect("message is Ok");
    let bytes = match msg {
        Message::Binary(b) => b,
        other => panic!("expected a binary presence frame, got {other:?}"),
    };
    match protocol::decode(&bytes).expect("decodes to a frame") {
        Frame::Presence(p) => p,
        other => panic!("expected a presence frame, got {other:?}"),
    }
}

#[tokio::test]
async fn ws_pushes_live_presence_heartbeat() {
    // Start the daemon "5s ago" so the first heartbeat reports uptime >= 5.
    let start = Instant::now()
        .checked_sub(Duration::from_secs(5))
        .expect("clock is at least 5s past its epoch");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        Some(anvil()),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        start,
        rx,
        ralphy_daemon::auth::AuthPolicy::Localhost,
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let (mut ws, _resp) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .expect("connecting to /ws");

    let first = next_presence(&mut ws).await;
    assert_eq!(first.name, Some("anvil".into()));
    assert!(
        first.uptime_secs >= 5,
        "uptime must reflect the 5s-past start, got {}",
        first.uptime_secs
    );

    // A second heartbeat proves the loop recurs (the 2s cadence), not a
    // fire-once-then-silent target that the first assertion alone would pass.
    let second = next_presence(&mut ws).await;
    assert!(
        second.uptime_secs >= first.uptime_secs,
        "the second beat's uptime must not regress: {} then {}",
        first.uptime_secs,
        second.uptime_secs
    );
}

#[tokio::test]
async fn ws_loop_stops_on_shutdown() {
    let start = Instant::now();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        Some(anvil()),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        start,
        rx,
        ralphy_daemon::auth::AuthPolicy::Localhost,
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let (mut ws, _resp) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .expect("connecting to /ws");
    let _ = next_presence(&mut ws).await;

    // Signal shutdown; the server's presence loop must break and drop the
    // socket, so the client's stream ends (no task keeps sending). Bounded so a
    // regression (loop ignoring shutdown) fails instead of hanging the suite.
    tx.send(true).unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        // Drain until the stream closes (a Close frame, then None).
        loop {
            match ws.next().await {
                None | Some(Ok(Message::Close(_))) => break,
                Some(Ok(_)) => continue,
                Some(Err(_)) => break,
            }
        }
    })
    .await;
    assert!(
        ended.is_ok(),
        "the presence loop must tear down on shutdown, not keep the socket open"
    );
}
