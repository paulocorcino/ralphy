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

#[tokio::test]
async fn ws_pushes_live_presence_heartbeat() {
    let id = identity::Identity {
        id: ulid::Ulid::nil(),
        name: "anvil".into(),
        avatar: "🐙".into(),
    };
    // Start the daemon "5s ago" so the first heartbeat reports uptime >= 5.
    let start = Instant::now()
        .checked_sub(Duration::from_secs(5))
        .expect("clock is at least 5s past its epoch");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = router(Some(id), PathBuf::from("does-not-exist"), start);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let (mut ws, _resp) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .expect("connecting to /ws");

    let msg = ws
        .next()
        .await
        .expect("a first message")
        .expect("message is Ok");
    let bytes = match msg {
        Message::Binary(b) => b,
        other => panic!("expected a binary presence frame, got {other:?}"),
    };
    match protocol::decode(&bytes).expect("decodes to a frame") {
        Frame::Presence(p) => {
            assert_eq!(p.name, Some("anvil".into()));
            assert!(
                p.uptime_secs >= 5,
                "uptime must reflect the 5s-past start, got {}",
                p.uptime_secs
            );
        }
        other => panic!("expected a presence frame, got {other:?}"),
    }
}
