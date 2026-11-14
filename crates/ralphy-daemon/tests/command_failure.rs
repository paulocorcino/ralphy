//! A moved verb replies with the failure the CLI wrote on stderr, and its
//! error chain goes to the daemon log only (ADR-0078 D11, D12). The child is
//! `command_test_child`, pointed at via `RALPHY_EXE_OVERRIDE`; it writes
//! `RALPHY_TEST_FAILURE_LINE` to stderr when its argv asks for JSON.
//!
//! Each test sets process-global variables: a lock serializes them.
#![expect(
    clippy::tests_outside_test_module,
    reason = "an integration test crate is the test module"
)]
#![expect(
    clippy::disallowed_methods,
    reason = "the tests hold ENV_LOCK while they change the environment"
)]

#[path = "support/golden.rs"]
mod golden;
#[path = "support/page.rs"]
pub mod page;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

const SLUG: &str = "owner/failure";
const CHAIN: &str = "committing the index: CHAIN-ONLY-TEXT";

/// The stderr line the CLI writes for a commit with nothing staged.
fn nothing_staged_line() -> String {
    serde_json::json!({
        "failure": {
            "domain": "working-tree",
            "code": "nothing-staged",
            "details": {},
            "text": { "cause": "nothing is staged", "remedy": "Stage a file first." },
        },
        "chain": CHAIN,
    })
    .to_string()
}

/// Start a daemon whose commands spawn the test child; `line` is the failure
/// line the child writes, or none. The child exits 1. The daemon stops when
/// the returned guard drops.
async fn daemon(line: Option<&str>) -> (u16, Running) {
    let guard = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("a temporary folder");
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert(SLUG, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).expect("saving the registry");
    std::env::set_var(
        "RALPHY_EXE_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
    std::env::set_var("RALPHY_TEST_EXIT_CODE", "1");
    match line {
        Some(line) => std::env::set_var("RALPHY_TEST_FAILURE_LINE", line),
        None => std::env::remove_var("RALPHY_TEST_FAILURE_LINE"),
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free port");
    let port = listener.local_addr().expect("the bound port").port();
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
        axum::serve(listener, app).await.expect("the daemon serves");
    });
    (port, (dir, tx, guard))
}

/// What keeps a test daemon alive: its folder, its shutdown sender, and the
/// lock on the process environment until the test ends.
type Running = (
    tempfile::TempDir,
    tokio::sync::watch::Sender<bool>,
    tokio::sync::MutexGuard<'static, ()>,
);

/// The tests set one process environment; plain `cargo test` runs them as threads.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// One command round-trip on its own page socket, returning the reply payload.
async fn ask(port: u16, verb: &str, payload: serde_json::Value) -> serde_json::Value {
    let mut ws = page::command(port).await;
    let frame = Frame::Command(Command {
        id: 1,
        verb: verb.to_owned(),
        payload,
    });
    ws.send(Message::Binary(protocol::encode(&frame).into()))
        .await
        .expect("the frame is sent");
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(msg) = ws.next().await {
            let Message::Binary(bytes) = msg.expect("the websocket frame arrives") else {
                continue;
            };
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                if cmd.id == 1 {
                    return Some(cmd.payload);
                }
            }
        }
        None
    })
    .await
    .expect("a reply must arrive within 10s")
    .expect("a reply on the requesting id")
}

async fn commit(port: u16) -> serde_json::Value {
    let payload = serde_json::json!({ "repo": SLUG, "message": "a message" });
    ask(port, "changes.commit", payload).await
}

#[tokio::test]
async fn the_commit_reply_carries_the_failure() {
    let line = nothing_staged_line();
    let (port, _dir) = daemon(Some(&line)).await;
    let reply = commit(port).await;
    assert_eq!(reply["status"], "error", "{reply}");
    assert_eq!(reply["failure"]["code"], "nothing-staged", "{reply}");
    golden::check("changes.commit--nothing-staged", reply, &[]);
}

#[tokio::test]
async fn the_message_is_the_filled_text() {
    let line = nothing_staged_line();
    let (port, _dir) = daemon(Some(&line)).await;
    let reply = commit(port).await;
    assert_eq!(
        reply["message"], "nothing is staged. Stage a file first.",
        "{reply}"
    );
}

#[tokio::test]
async fn a_missing_failure_line_is_ralphy_failed() {
    let (port, _dir) = daemon(None).await;
    let reply = commit(port).await;
    assert_eq!(reply["status"], "error", "{reply}");
    assert_eq!(reply["failure"]["domain"], "daemon", "{reply}");
    assert_eq!(reply["failure"]["code"], "ralphy-failed", "{reply}");
    assert!(
        reply["failure"]["details"].get("cause").is_none(),
        "no cause detail reaches the browser: {reply}"
    );
}

#[tokio::test]
async fn a_ralphy_failed_line_loses_its_cause() {
    const MARKER: &str = "CAUSE-MARKER-7f3a";
    let line = serde_json::json!({
        "failure": {
            "domain": "daemon",
            "code": "ralphy-failed",
            "details": { "cause": MARKER },
            "text": { "cause": "the command failed", "remedy": "Try again." },
        },
        "chain": MARKER,
    })
    .to_string();
    let (port, _dir) = daemon(Some(&line)).await;
    let reply = commit(port).await;
    assert_eq!(reply["failure"]["domain"], "daemon", "{reply}");
    assert_eq!(reply["failure"]["code"], "ralphy-failed", "{reply}");
    assert!(
        reply["failure"]["details"].get("cause").is_none(),
        "{reply}"
    );
    assert!(!reply.to_string().contains(MARKER), "{reply}");
}

#[tokio::test]
async fn an_unmoved_verb_keeps_its_argv() {
    let line = nothing_staged_line();
    let (port, _dir) = daemon(Some(&line)).await;
    let payload = serde_json::json!({ "repo": SLUG, "paths": ["a.txt"] });
    let reply = ask(port, "changes.stage", payload).await;
    let message = reply["message"].as_str().expect("an error message");
    assert!(
        message.contains("dispatch-argv: changes stage --path=a.txt"),
        "{reply}"
    );
    assert!(!message.contains("--failure-format"), "{reply}");
    assert!(reply.get("failure").is_none(), "{reply}");
}

/// The text the daemon log received.
#[derive(Clone, Default)]
struct Written(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Written {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("a test lock").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Written {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn the_chain_goes_to_the_log_not_the_reply() {
    let line = nothing_staged_line();
    let (port, _dir) = daemon(Some(&line)).await;
    // Under the lock, so no other test registers a log callsite while this one listens.
    let written = Written::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(written.clone())
        .with_ansi(false)
        .finish();
    let _log = tracing::subscriber::set_default(subscriber);
    tracing::callsite::rebuild_interest_cache();
    let reply = commit(port).await;
    assert!(!reply.to_string().contains("CHAIN-ONLY-TEXT"), "{reply}");
    let log = String::from_utf8_lossy(&written.0.lock().expect("a test lock")).into_owned();
    assert!(log.contains(CHAIN), "the log holds the chain: {log}");
}
