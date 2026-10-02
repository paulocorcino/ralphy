//! ADR-0043 D4/D6 over the workbench's interactive launch (issue #261): a Gemini
//! console opened from the UI must land in the SAME owned configuration root, and
//! under the same policy document, that `ralphy run --agent gemini` uses. When
//! the root is missing, the daemon asks the CLI to prepare it (ADR-0040
//! Amendment 3); `command_test_child` stands in for the CLI and decides by marker
//! files in the repo.
//!
//! Four legs against one live loopback daemon: the CLI fails → refused with its
//! reason, nothing spawned; the CLI writes nothing → still refused; the CLI
//! writes the root → the launch is contained; the root exists → the CLI is not
//! run again. Containment is read back off the CHILD, not the spec: the
//! `GEMINI_CLI_HOME` in its environment and the `--policy` in its own argv.

use std::path::Path;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

fn terminal(data: &[u8]) -> Message {
    Message::Binary(
        protocol::encode(&Frame::Terminal {
            session: 1,
            data: data.to_vec(),
        })
        .into(),
    )
}

/// A raw HTTP/1.1 GET on the live listener, returning the body. Raw sockets rather
/// than `oneshot` because the assertion is about the SERVING router's own session
/// state — a second `router()` would have its own empty session manager and the
/// "nothing was spawned" claim would be vacuous.
async fn http_get(port: u16, path: &str) -> String {
    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    sock.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut raw = String::new();
    sock.read_to_string(&mut raw).await.unwrap();
    raw.split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or(raw)
}

/// Open `url`, read back the env var and argv the LIVE child was spawned with,
/// and assert both halves of the containment: the repo's own `GEMINI_CLI_HOME`
/// and `--policy` pointing at the owned root's document. Read off the CHILD,
/// not the spec: a regression that dropped `spec.args` would leave the root
/// right and the policy gone.
async fn assert_launches_contained(url: &str, home: &Path, policy: &Path) {
    let (mut ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("a repo with an owned root must upgrade");
    ws.send(terminal(b"env GEMINI_CLI_HOME\r")).await.unwrap();
    ws.send(terminal(b"argv\r")).await.unwrap();

    // The PTY wraps and reflows, so every comparison runs on a separator-
    // normalized, whitespace-stripped view.
    fn flatten(s: &str) -> String {
        s.replace('\\', "/")
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect()
    }
    let want_env = flatten(&format!("ENV:GEMINI_CLI_HOME={}", home.to_string_lossy()));
    let want_argv = flatten(&format!("ARGV:--policy {}", policy.to_string_lossy()));

    let got = tokio::time::timeout(Duration::from_secs(10), async {
        let mut acc = String::new();
        while let Some(msg) = ws.next().await {
            let bytes = match msg.unwrap() {
                Message::Binary(b) => b,
                _ => continue,
            };
            if let Ok(Frame::Terminal { data, .. }) = protocol::decode(&bytes) {
                // Play the terminal emulator: answer ConPTY's startup `ESC[6n` so
                // the child unblocks on Windows.
                if data
                    .windows(CURSOR_POSITION_REQUEST.len())
                    .any(|w| w == CURSOR_POSITION_REQUEST)
                {
                    ws.send(terminal(CURSOR_POSITION_REPLY)).await.unwrap();
                }
                acc.push_str(&String::from_utf8_lossy(&data));
                // Wait for the COMPLETE value of each, not merely the marker: the
                // echo of the typed command already puts a newline in `acc`, so a
                // "marker plus any newline" condition returns on a partial read.
                let flat = flatten(&acc);
                if flat.contains(&want_env) && flat.contains(&want_argv) {
                    return acc;
                }
            }
        }
        acc
    })
    .await
    .expect("the gemini session's env + argv round-trip must complete within 10s");

    let flat = flatten(&got);
    assert!(
        flat.contains(&want_env),
        "the workbench child must run under the repo's OWN gemini root; wanted {want_env}, got:\n{got}"
    );
    assert!(
        flat.contains(&want_argv),
        "the workbench child must carry --policy pointing at the owned root's document; wanted {want_argv}, got:\n{got}"
    );
    ws.send(terminal(b"quit\r")).await.unwrap();
}

#[tokio::test]
async fn gemini_session_prepares_a_missing_root_and_refuses_when_it_cannot() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/geminilab";
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

    let url = format!("ws://127.0.0.1:{port}/ws/session?repo=owner%2Fgeminilab&agent=gemini");
    let home = dir.path().join(".ralphy").join("gemini-home");
    let policy = home.join(".gemini").join("ralphy-policy.toml");
    let (fail, noop, ran) = (
        dir.path().join("prepare-root.fail"),
        dir.path().join("prepare-root.noop"),
        dir.path().join("prepare-root.ran"),
    );

    // --- Leg 1: the CLI cannot prepare the root → the launch is refused with
    // the CLI's own reason, and nothing is spawned.
    std::fs::write(&fail, "").unwrap();
    let refusal = super::refused(&url).await;
    assert!(
        ran.exists(),
        "the daemon must ask the CLI to prepare the root"
    );
    assert_eq!(
        refusal["message"], "gemini: autonomy is disabled by your administrator",
        "the refusal carries the CLI's last line, without its `Error: ` head"
    );
    // The browser's fold reads this very file (`endNotice`).
    super::golden::check(
        "session-end--refused",
        refusal,
        &["/daemon_id", "/environment"],
    );
    assert_eq!(
        http_get(port, "/api/sessions").await,
        "[]",
        "the refusal must return BEFORE spawn_attached — no child, no session record"
    );

    // --- Leg 2: the CLI says it succeeded but wrote nothing → still refused:
    // the gate keys on the file, never on the exit code.
    std::fs::remove_file(&fail).unwrap();
    std::fs::write(&noop, "").unwrap();
    let refusal = super::refused(&url).await;
    assert!(
        refusal["message"]
            .as_str()
            .unwrap_or_default()
            .contains("still missing"),
        "{refusal}"
    );
    assert!(!policy.exists());
    assert_eq!(http_get(port, "/api/sessions").await, "[]");

    // --- Leg 3: the CLI writes the root → the same URL launches, contained.
    std::fs::remove_file(&noop).unwrap();
    assert_launches_contained(&url, &home, &policy).await;
    assert!(
        policy.is_file(),
        "the CLI wrote the policy the child points at"
    );

    // --- Leg 4: the root exists → the launch does not ask the CLI again.
    std::fs::remove_file(&ran).unwrap();
    assert_launches_contained(&url, &home, &policy).await;
    assert!(!ran.exists(), "an existing root needs no preparation");
}
