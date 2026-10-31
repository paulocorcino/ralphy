//! End-to-end workbench session over a real loopback WebSocket (docs/adr/0032
//! §2; issues #162, #166): a page socket opens a `console` channel with
//! `repo=<slug>` and `agent=claude` (the agent program overridden to the helper
//! bin), types a line, and reads it echoed back through the codec + PTY —
//! proving the channel → codec → PTY → child path. Then it types `quit` so the
//! CHILD exits: under the #166 tmux model a WS drop no longer kills the session
//! (persistence — see `session_persistence.rs`), so the session ends only when
//! its child exits, and that end is observed as the daemon closing the channel
//! with `done` within a bounded wait.
//! Tree-kill DEPTH is proven by `session_roundtrip`; this proves the transport +
//! the child-exit teardown path.

use std::time::{Duration, Instant};

use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio_tungstenite::tungstenite::Message;

/// Encode a terminal keystroke frame the way the browser would.
fn terminal(data: &[u8]) -> Message {
    Message::Binary(
        protocol::encode(&Frame::Terminal {
            session: 1,
            data: data.to_vec(),
        })
        .into(),
    )
}

#[tokio::test]
async fn session_ws_round_trips_keystrokes_and_tears_down_on_close() {
    // A registry with one reachable slug pointing at a temp dir.
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/workbench";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();

    super::point_launcher_at_test_child();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        registry_path,
        std::path::PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let mut ws = super::page::console(
        port,
        serde_json::json!({ "repo": "owner/workbench", "agent": "claude" }),
    )
    .await;

    // Type a line and read until the child echoes it. Along the way, play the
    // terminal emulator (xterm.js's role): answer the ConPTY startup `ESC[6n` so
    // the child unblocks on Windows. Bounded so a regression fails, not hangs.
    ws.send(terminal(b"hello-ws\r")).await.unwrap();
    let mut size = None;
    let got = tokio::time::timeout(Duration::from_secs(10), async {
        let mut acc = String::new();
        while let Some(msg) = ws.next().await {
            let bytes = match msg.unwrap() {
                Message::Binary(b) => b,
                Message::Text(_)
                | Message::Ping(_)
                | Message::Pong(_)
                | Message::Close(_)
                | Message::Frame(_) => continue,
            };
            let frame = protocol::decode(&bytes);
            if let Ok(Frame::Command(cmd)) = &frame {
                if cmd.verb == "session-size" && size.is_none() {
                    size = Some(cmd.payload.clone());
                }
            }
            if let Ok(Frame::Terminal { data, .. }) = frame {
                if data
                    .windows(CURSOR_POSITION_REQUEST.len())
                    .any(|w| w == CURSOR_POSITION_REQUEST)
                {
                    ws.send(terminal(CURSOR_POSITION_REPLY)).await.unwrap();
                }
                acc.push_str(&String::from_utf8_lossy(&data));
                if acc.contains("GOT:hello-ws") {
                    return acc;
                }
            }
        }
        acc
    })
    .await
    .expect("keystroke round-trip must complete within 10s");
    assert!(
        got.contains("GOT:hello-ws"),
        "server must echo the typed line back through codec + PTY; got:\n{got}"
    );
    // The browser's fold reads this very file (`readSocketEvent`).
    super::golden::check(
        "session-size",
        size.expect("a launch is told the session size"),
        &[],
    );

    // End the CHILD (not the socket): under the #166 tmux model a WS drop only
    // detaches, so a session ends when its child exits. `quit` exits the helper;
    // its PTY reaches EOF, the pump removes the session and evicts this bridge,
    // and the daemon closes the channel. Bounded so a hang fails.
    ws.send(terminal(b"quit\r")).await.unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(msg) = ws.next().await {
            match msg {
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => continue,
            }
        }
    })
    .await;
    assert!(
        ended.is_ok(),
        "the child exiting must end the session's stream (pump EOF → evict), not hang"
    );
    assert_eq!(
        ws.closed_reason(),
        Some("done"),
        "the session ended, so its channel closes as done"
    );
}
