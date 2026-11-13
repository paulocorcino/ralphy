//! An Antigravity console from the workbench runs `agy` in Ralphy's owned root,
//! with Ralphy's hooks file in it. When the file is missing, the daemon asks the
//! CLI to prepare the root and opens the console only when the file is there.
//! `command_test_child` stands in for the CLI: it writes the hooks file under
//! `RALPHY_DAEMON_DIR`, or writes nothing when the repo has `prepare-root.noop`.
//!
//! Each test sets `RALPHY_DAEMON_DIR` to its own folder: nextest runs each test
//! in its own process.

use std::path::Path;
use std::time::{Duration, Instant};

use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio_tungstenite::tungstenite::Message;

use crate::page;

fn terminal(data: &[u8]) -> Message {
    Message::Binary(
        protocol::encode(&Frame::Terminal {
            session: 1,
            data: data.to_vec(),
        })
        .into(),
    )
}

fn launch() -> serde_json::Value {
    serde_json::json!({ "repo": "owner/agylab", "agent": "antigravity" })
}

/// Serve a daemon whose store is `store` and whose one repo is `repo`; its port.
async fn serve(store: &Path, repo: &Path) -> u16 {
    std::env::set_var("RALPHY_DAEMON_DIR", store);
    let registry_path = store.join("repos.toml");
    let mut registered = registry::RegistryStore::default();
    registered.upsert("owner/agylab", &repo.to_string_lossy());
    registry::save_to(&registered, &registry_path).expect("write the registry");
    super::point_launcher_at_test_child();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback port");
    let port = listener.local_addr().expect("the bound address").port();
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
        let _keep_open = tx;
        axum::serve(listener, app).await.expect("serve the daemon");
    });
    port
}

/// The PTY wraps lines, so compare without separators' style or whitespace.
fn flatten(s: &str) -> String {
    s.replace('\\', "/")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// Everything the console prints until it holds `want` (flattened), or 10 s.
async fn read_until(ws: &mut page::Channel, want: &str) -> String {
    let read = async {
        let mut acc = String::new();
        while let Some(msg) = ws.next().await {
            let Message::Binary(bytes) = msg.expect("read the next frame") else {
                continue;
            };
            let Ok(Frame::Terminal { data, .. }) = protocol::decode(&bytes) else {
                continue;
            };
            // Answer ConPTY's startup `ESC[6n`, or the child waits on Windows.
            if data
                .windows(CURSOR_POSITION_REQUEST.len())
                .any(|w| w == CURSOR_POSITION_REQUEST)
            {
                ws.send(terminal(CURSOR_POSITION_REPLY))
                    .await
                    .expect("reply");
            }
            acc.push_str(&String::from_utf8_lossy(&data));
            if flatten(&acc).contains(want) {
                break;
            }
        }
        acc
    };
    tokio::time::timeout(Duration::from_secs(10), read)
        .await
        .expect("the console answers within 10s")
}

#[tokio::test]
async fn an_antigravity_console_without_its_hooks_file_prepares_the_root_then_opens() {
    let (store, repo) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let port = serve(store.path(), repo.path()).await;
    let root = store.path().join("antigravity-root");
    let hooks = root.join("config").join("hooks.json");
    assert!(!hooks.exists());

    let mut ws = page::console(port, launch()).await;
    ws.send(terminal(b"argv\r")).await.expect("send the frame");
    let want = flatten(&format!("ARGV:--gemini_dir={}", root.to_string_lossy()));
    let got = read_until(&mut ws, &want).await;

    assert!(flatten(&got).contains(&want), "wanted {want}, got:\n{got}");
    assert!(
        repo.path().join("prepare-root.ran").exists(),
        "prepare-root ran"
    );
    assert!(
        hooks.is_file(),
        "the console opened on the hooks file it found"
    );
    ws.send(terminal(b"quit\r")).await.expect("send the frame");
}

#[tokio::test]
async fn an_antigravity_console_is_refused_when_prepare_root_writes_nothing() {
    let (store, repo) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(repo.path().join("prepare-root.noop"), "").unwrap();
    let port = serve(store.path(), repo.path()).await;

    let refusal = page::refused(port, launch()).await;

    assert!(
        repo.path().join("prepare-root.ran").exists(),
        "prepare-root ran"
    );
    assert_eq!(
        refusal["message"], "antigravity: the hooks file is still missing after preparing the root",
        "{refusal}"
    );
}
