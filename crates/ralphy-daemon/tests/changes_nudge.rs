//! The run-completion nudge (issue #310; ADR-0036 amendment 2026-07-25): when a
//! dispatched child exits, the daemon pushes `changes.dirty {repo}` on EVERY
//! `/ws/tree` connection — with no `watch` frame ever sent, because this third
//! push kind is not watcher-fed and has no subscription verb.
//!
//! The second test is the load-bearing one: it drops the `/ws/command` socket
//! mid-run, so the nudge can only arrive if the send lives inside the blocking
//! wait task rather than in the handler's `select!` wait arm (which that
//! disconnect abandons). A presence-only assertion would not catch that.
//!
//! SOLE env-setter in its file: `RALPHY_EXE_OVERRIDE`/`RALPHY_TEST_*` are
//! process-global. Both tests set the SAME values, so they may run in parallel.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

/// The child sleeps before exiting so a test can disconnect mid-run. Identical
/// in both tests — a differing value would be an intra-process env race.
const CHILD_SLEEP_MS: &str = "1500";

fn set_child_env() {
    std::env::set_var(
        "RALPHY_EXE_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
    std::env::set_var("RALPHY_TEST_EXIT_CODE", "0");
    std::env::set_var("RALPHY_TEST_SLEEP_MS", CHILD_SLEEP_MS);
}

fn run_command(slug: &str) -> Message {
    Message::Binary(protocol::encode(&Frame::Command(Command {
        id: 1,
        verb: "run".to_string(),
        payload: serde_json::json!({ "repo": slug, "agent": "claude", "branchMode": "new" }),
    })))
}

/// Serve `router` over loopback and answer with its port AND the shutdown sender
/// — the caller must HOLD that sender: dropping it makes `shutdown.changed()`
/// resolve `Err` at once, which every handler reads as shutdown and closes on.
async fn serve(registry_path: std::path::PathBuf) -> (u16, tokio::sync::watch::Sender<bool>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::watch::channel(false);
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
    (port, tx)
}

/// Read `/ws/tree` frames until one decodes as `changes.dirty`, and answer its
/// `payload.repo`. Panics on the 15s deadline.
async fn next_changes_dirty<S>(tree: &mut S) -> String
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(msg) = tree.next().await {
            let Ok(Message::Binary(bytes)) = msg else {
                continue;
            };
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                if cmd.verb == "changes.dirty" {
                    return cmd.payload["repo"].as_str().unwrap_or_default().to_string();
                }
            }
        }
        panic!("the /ws/tree socket closed before any changes.dirty frame");
    })
    .await
    .expect("a changes.dirty frame must arrive within 15s")
}

#[tokio::test]
async fn run_exit_pushes_changes_dirty() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    // TWO repos, and the run goes to the SECOND: with a single registry entry a
    // daemon echoing a constant (or just the sole entry) would pass.
    store.upsert("owner/other", &dir.path().to_string_lossy());
    let slug = "owner/nudge";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();

    set_child_env();
    let (port, _shutdown) = serve(registry_path).await;

    // Connect the listener FIRST and with NO watch frame: the nudge must reach a
    // connection that holds no watch at all.
    let (mut tree, _resp) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws/tree"))
            .await
            .expect("connecting to /ws/tree");
    // The upgraded handler subscribes after the handshake returns; settle so the
    // subscription is live before the run that must reach it.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let (mut cmd_ws, _resp) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws/command"))
            .await
            .expect("connecting to /ws/command");
    cmd_ws.send(run_command(slug)).await.unwrap();

    assert_eq!(
        next_changes_dirty(&mut tree).await,
        slug,
        "the nudge must name the repo whose run exited"
    );
}

#[tokio::test]
async fn nudge_survives_the_spawning_client_disconnect() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/nudge";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();

    set_child_env();
    let (port, _shutdown) = serve(registry_path).await;

    let (mut tree, _resp) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws/tree"))
            .await
            .expect("connecting to /ws/tree");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let (mut cmd_ws, _resp) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws/command"))
            .await
            .expect("connecting to /ws/command");
    cmd_ws.send(run_command(slug)).await.unwrap();

    // Read only up to the ack, then drop the socket while the child still sleeps.
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(msg) = cmd_ws.next().await {
            let Ok(Message::Binary(bytes)) = msg else {
                continue;
            };
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                if cmd.payload.get("status").and_then(|s| s.as_str()) == Some("spawned") {
                    return;
                }
            }
        }
        panic!("never received the spawn ack");
    })
    .await
    .expect("the spawn ack must arrive within 10s");
    drop(cmd_ws);

    assert_eq!(
        next_changes_dirty(&mut tree).await,
        slug,
        "the nudge must fire from the blocking wait task, which the disconnect \
         arm does not abandon"
    );
}
