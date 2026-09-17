//! ADR-0063 §3 over the workbench's interactive launch (issue #408): a NEW
//! console opens INSIDE the checkout the picker selected. `/ws/session?repo=…
//! &agent=…&checkout=<name>` resolves the name against the primary's
//! `.ralphy/worktrees/` with the same resolver and confinement every git-backed
//! verb uses, spawns the child with the worktree as `cwd`, and announces the
//! name on `session-open` — on launch AND on a reattach, from the record — and
//! on `/api/sessions`. An unknown name is `400 unknown checkout` BEFORE any
//! write (the Cursor gate) or spawn; no key is the primary tree, byte-identical
//! to today.
//!
//! Every leg runs against ONE live loopback daemon, each on its own socket, so
//! the "no session row" and "no file written" claims are about the SERVING
//! router's own state. The cwd is read off the CHILD (`CWD:` line), not the
//! spec — a regression that resolved the name but spawned at the root would
//! still fail. This file is the sole setter of `RALPHY_DAEMON_AGENT_OVERRIDE`
//! in its process (one `#[tokio::test]`, legs sequential; nextest isolates).

use std::path::Path;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn terminal(data: &[u8]) -> Message {
    Message::Binary(protocol::encode(&Frame::Terminal {
        session: 1,
        data: data.to_vec(),
    }))
}

/// A raw HTTP/1.1 GET on the live listener, returning the body.
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

async fn sessions(port: u16) -> Vec<serde_json::Value> {
    let body = http_get(port, "/api/sessions?local=1").await;
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("sessions body {body:?}: {e}"))
}

/// The PTY wraps and reflows, so every path comparison runs on a separator-
/// normalized, whitespace-stripped view.
fn flatten(s: &str) -> String {
    s.replace('\\', "/")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// Read terminal frames until `needle` shows up, answering ConPTY's startup
/// `ESC[6n` along the way, and capture the `session-open` payload seen on the
/// way (the bridge sends it FIRST, before any terminal byte).
async fn read_until(ws: &mut Ws, needle: &str) -> (String, Option<serde_json::Value>) {
    let mut terminal_text = String::new();
    let mut open = None;
    let completed = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(message) = ws.next().await {
            match message.unwrap() {
                Message::Binary(bytes) => match protocol::decode(&bytes) {
                    Ok(Frame::Terminal { data, .. }) => {
                        if data
                            .windows(CURSOR_POSITION_REQUEST.len())
                            .any(|window| window == CURSOR_POSITION_REQUEST)
                        {
                            ws.send(terminal(CURSOR_POSITION_REPLY)).await.unwrap();
                        }
                        terminal_text.push_str(&String::from_utf8_lossy(&data));
                        if terminal_text.replace("\r\n", "").contains(needle) {
                            return true;
                        }
                    }
                    Ok(Frame::Command(command)) if command.verb == "session-open" => {
                        open = Some(command.payload);
                    }
                    _ => {}
                },
                Message::Close(_) => return true,
                _ => {}
            }
        }
        true
    })
    .await;
    assert!(
        completed.is_ok(),
        "timed out waiting for {needle:?}; open={open:?}; terminal={terminal_text:?}"
    );
    (terminal_text, open)
}

/// The path the helper child prints between `CWD:` and `READY` on start,
/// flattened — the PTY may prefix the line with escapes and wrap a long path.
fn cwd_line(text: &str) -> String {
    let flat = flatten(text);
    let (_, rest) = flat
        .split_once("CWD:")
        .unwrap_or_else(|| panic!("a CWD: line in the child's output; got {text:?}"));
    let (path, _) = rest
        .split_once("READY")
        .unwrap_or_else(|| panic!("READY after the CWD: line; got {text:?}"));
    path.to_string()
}

/// Connect and expect an HTTP refusal: `(status, body)`.
async fn refused(url: &str) -> (u16, String) {
    let err = tokio_tungstenite::connect_async(url)
        .await
        .expect_err("the upgrade must be refused");
    match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => {
            let status = resp.status().as_u16();
            let body = String::from_utf8_lossy(resp.body().as_deref().unwrap_or(&[])).into_owned();
            (status, body)
        }
        other => panic!("expected an HTTP refusal, got {other:?}"),
    }
}

fn no_opt_out_anywhere(root: &Path, wt: &Path) -> bool {
    !root.join(".cursorindexingignore").exists() && !wt.join(".cursorindexingignore").exists()
}

#[tokio::test]
async fn a_console_opens_in_the_selected_checkout_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    // The primary LOOKS like a git checkout — the Cursor gate walks for `.git`.
    std::fs::create_dir(root.join(".git")).unwrap();
    let registry_path = root.join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/wtlab";
    store.upsert(slug, &root.to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();

    // The linked worktree as git records it: a pointer FILE, no git spawn.
    let wt = root.join(".ralphy").join("worktrees").join("wt-a");
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(wt.join(".git"), "gitdir: /r/.git/worktrees/wt-a\n").unwrap();

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
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base = format!("ws://127.0.0.1:{port}/ws/session?repo=owner%2Fwtlab");

    // --- (a) a claude console with `checkout=wt-a` runs IN the worktree and
    // announces the name.
    let (mut ws_a, _) =
        tokio_tungstenite::connect_async(format!("{base}&agent=claude&checkout=wt-a"))
            .await
            .expect("a known checkout must upgrade");
    let (text, open) = read_until(&mut ws_a, "READY").await;
    let open = open.expect("session-open precedes the first terminal byte");
    assert_eq!(
        open["checkout"], "wt-a",
        "(a) session-open must carry the worktree name; got {open}"
    );
    let cwd = cwd_line(&text);
    assert!(
        cwd.ends_with(".ralphy/worktrees/wt-a"),
        "(a) the child must run in the worktree; CWD was {cwd:?}"
    );
    let id_a = open["session"].as_u64().expect("a numeric session id");

    // --- (b) an unknown name is refused with 400 and leaves no session row.
    let (status, body) = refused(&format!("{base}&agent=claude&checkout=nope")).await;
    assert_eq!(
        status, 400,
        "(b) an unknown checkout is a 400; body {body:?}"
    );
    assert_eq!(body, "unknown checkout", "(b) the one refusal text");
    let rows = sessions(port).await;
    assert_eq!(
        rows.len(),
        1,
        "(b) the refusal must precede spawn_attached — only leg (a)'s row; got {rows:?}"
    );

    // --- (b') the refusal precedes the Cursor gate: nothing is written anywhere.
    let (status, body) = refused(&format!("{base}&agent=cursor&checkout=nope")).await;
    assert_eq!(
        status, 400,
        "(b') an unknown checkout is a 400; body {body:?}"
    );
    assert_eq!(body, "unknown checkout", "(b') the one refusal text");
    assert!(
        no_opt_out_anywhere(&root, &wt),
        "(b') a bad name must not write .cursorindexingignore at the root or the worktree"
    );
    assert_eq!(sessions(port).await.len(), 1, "(b') no session row either");

    // --- (c) no key: the primary tree, no `checkout` announced.
    let (mut ws_c, _) = tokio_tungstenite::connect_async(format!("{base}&agent=claude"))
        .await
        .expect("no checkout must upgrade as before");
    let (text, open) = read_until(&mut ws_c, "READY").await;
    let open = open.expect("session-open precedes the first terminal byte");
    assert!(
        open["checkout"].is_null(),
        "(c) no checkout → null on the frame; got {open}"
    );
    let cwd = cwd_line(&text);
    // The child prints the spelling it was spawned with, and on a GitHub
    // runner the temp dir is `RUNNER~1` — an 8.3 alias of `runneradmin` — so
    // BOTH sides go through `canonicalize` before they meet.
    let canonical = |path: &Path| {
        flatten(&path.canonicalize().unwrap().to_string_lossy())
            .trim_start_matches("//?/")
            .to_string()
    };
    assert_eq!(
        canonical(Path::new(&cwd)),
        canonical(&root),
        "(c) the child must run at the registry path"
    );
    let id_c = open["session"].as_u64().expect("a numeric session id");
    assert_ne!(id_a, id_c);

    // --- (d) a reattach re-announces the checkout from the record (`takeover`:
    // leg (a)'s socket still holds the writer slot; the snapshot replays `READY`).
    let (mut ws_d, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws/session?id={id_a}&repo=owner%2Fwtlab&takeover=1"
    ))
    .await
    .expect("a reattach on a live session must upgrade");
    let (_, open) = read_until(&mut ws_d, "READY").await;
    let open = open.expect("session-open on reattach");
    assert_eq!(
        open["checkout"], "wt-a",
        "(d) the reattach must re-announce the worktree; got {open}"
    );

    // --- (e) the sessions listing carries the key only where it applies.
    let rows = sessions(port).await;
    let row_a = rows
        .iter()
        .find(|r| r["id"].as_u64() == Some(id_a))
        .unwrap_or_else(|| panic!("row for {id_a} in {rows:?}"));
    let row_c = rows
        .iter()
        .find(|r| r["id"].as_u64() == Some(id_c))
        .unwrap_or_else(|| panic!("row for {id_c} in {rows:?}"));
    assert_eq!(
        row_a["checkout"], "wt-a",
        "(e) row a carries the name; {row_a}"
    );
    assert!(
        row_c.get("checkout").is_none(),
        "(e) row c has NO checkout key (serialised only when present); {row_c}"
    );

    // --- (f) a Cursor console in the worktree writes the opt-out IN the worktree.
    let (mut ws_f, _) =
        tokio_tungstenite::connect_async(format!("{base}&agent=cursor&checkout=wt-a"))
            .await
            .expect("cursor in a known checkout must upgrade");
    let (_, open) = read_until(&mut ws_f, "READY").await;
    assert_eq!(open.expect("session-open")["checkout"], "wt-a");
    assert_eq!(
        std::fs::read_to_string(wt.join(".cursorindexingignore")).unwrap(),
        "*\n",
        "(f) the indexing gate protects the worktree the console lives in"
    );

    // `ws_a` was evicted by the takeover; `ws_d` now writes for that child.
    drop(ws_a);
    for ws in [&mut ws_c, &mut ws_d, &mut ws_f] {
        ws.send(terminal(b"quit\r")).await.unwrap();
    }
}
