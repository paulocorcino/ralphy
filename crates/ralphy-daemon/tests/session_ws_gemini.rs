//! ADR-0043 D4/D6 over the workbench's interactive launch (issue #261): a Gemini
//! console opened from the UI must land in the SAME owned configuration root, and
//! under the same policy document, that `ralphy run --agent gemini` uses — and
//! must be refused BEFORE anything is spawned when that root does not exist.
//!
//! Two legs against one live loopback daemon: the URL is refused with `400` and no
//! session while the repo has no owned root, then — once the policy document
//! exists — it launches and the child reports back the `GEMINI_CLI_HOME` it was
//! actually given, which is the only observation that proves the containment
//! reached the process rather than only the spec.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

fn terminal(data: &[u8]) -> Message {
    Message::Binary(protocol::encode(&Frame::Terminal {
        session: 1,
        data: data.to_vec(),
    }))
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

#[tokio::test]
async fn gemini_session_refuses_a_rootless_repo_and_launches_under_the_owned_one() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/geminilab";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();

    std::env::set_var(
        "RALPHY_DAEMON_AGENT_OVERRIDE",
        env!("CARGO_BIN_EXE_session_test_child"),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        registry_path,
        std::path::PathBuf::from("does-not-exist"),
        std::path::PathBuf::from("does-not-exist"),
        std::path::PathBuf::from("does-not-exist"),
        std::path::PathBuf::from("does-not-exist"),
        std::path::PathBuf::from("does-not-exist"),
        std::path::PathBuf::from("does-not-exist"),
        std::path::PathBuf::from("does-not-exist"),
        std::path::PathBuf::from("does-not-exist"),
        std::path::PathBuf::from("does-not-exist"),
        Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let url = format!("ws://127.0.0.1:{port}/ws/session?repo=owner%2Fgeminilab&agent=gemini");

    // --- Leg 1: no owned root → the upgrade is refused and nothing is spawned.
    let err = tokio_tungstenite::connect_async(&url)
        .await
        .expect_err("a repo with no owned root must NOT upgrade");
    let (status, body) = match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => {
            let status = resp.status();
            let body = String::from_utf8_lossy(resp.body().as_deref().unwrap_or(&[])).into_owned();
            (status, body)
        }
        other => panic!("expected an HTTP refusal, got {other:?}"),
    };
    assert_eq!(status.as_u16(), 400, "the refusal must be a 400");
    assert!(
        body.contains("ralphy run --agent gemini"),
        "the refusal must name the remedy verbatim; got:\n{body}"
    );
    assert_eq!(
        http_get(port, "/api/sessions").await,
        "[]",
        "the refusal must return BEFORE spawn_attached — no child, no session record"
    );

    // --- Leg 2: the owned root exists → the same URL launches, and the child
    // reports the containment env var it was actually spawned with.
    let home = dir.path().join(".ralphy").join("gemini-home");
    let cli_dir = home.join(".gemini");
    std::fs::create_dir_all(&cli_dir).unwrap();
    std::fs::write(cli_dir.join("ralphy-policy.toml"), "# policy\n").unwrap();

    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("a repo with an owned root must upgrade");

    ws.send(terminal(b"env GEMINI_CLI_HOME\r")).await.unwrap();
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
                if acc.contains("ENV:GEMINI_CLI_HOME=") && acc.contains('\n') {
                    return acc;
                }
            }
        }
        acc
    })
    .await
    .expect("the gemini session's env round-trip must complete within 10s");

    // The PTY wraps and reflows, so compare on a separator-normalized, whitespace-
    // stripped view rather than on the raw line.
    let flat: String = got
        .replace('\\', "/")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let want = format!(
        "ENV:GEMINI_CLI_HOME={}",
        home.to_string_lossy().replace('\\', "/")
    );
    let want: String = want.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        flat.contains(&want),
        "the workbench child must run under the repo's OWN gemini root; wanted {want}, got:\n{got}"
    );

    ws.send(terminal(b"quit\r")).await.unwrap();
}
