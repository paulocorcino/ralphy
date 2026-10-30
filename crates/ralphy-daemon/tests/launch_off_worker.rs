//! A Claude console launch reads `claude --version` before it spawns the
//! console. That read waits on a child process, so it must not run on a tokio
//! worker: on a runtime with one worker, every other request would wait for it.
//!
//! SOLE env-setter in its file: the variables are process-global, so this
//! test must be alone in its file (no intra-process race).

#![expect(
    clippy::disallowed_methods,
    reason = "nextest runs each test in its own process, so a test may change its environment"
)]

use std::time::{Duration, Instant};

use ralphy_daemon::{registry, router};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// How long the fake `claude --version` waits before it answers.
const VERSION_SLEEP: Duration = Duration::from_secs(3);
/// How long after the launch the second request is sent.
const SECOND_REQUEST_DELAY: Duration = Duration::from_millis(200);
/// How long the second request may take to answer.
const ANSWER_LIMIT: Duration = Duration::from_secs(1);

#[tokio::test(flavor = "current_thread")]
async fn a_claude_launch_does_not_block_the_worker() {
    let store = tempfile::tempdir().expect("a temporary store");
    let repo = tempfile::tempdir().expect("a temporary repo");
    let bin = tempfile::tempdir().expect("a folder for the fake claude");

    // A fresh copy: the version read is cached by path and modification time,
    // so a copy no other test ran is read for real.
    let claude = bin
        .path()
        .join(format!("claude{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(env!("CARGO_BIN_EXE_command_test_child"), &claude)
        .expect("copying the test child as claude");
    std::env::set_var("RALPHY_DAEMON_DIR", store.path());
    std::env::set_var("RALPHY_DAEMON_AGENT_OVERRIDE", &claude);
    std::env::set_var(
        "RALPHY_TEST_VERSION_SLEEP_MS",
        VERSION_SLEEP.as_millis().to_string(),
    );

    let registry_path = store.path().join("repos.toml");
    let mut repos = registry::RegistryStore::default();
    repos.upsert("owner/launch", &repo.path().to_string_lossy());
    registry::save_to(&repos, &registry_path).expect("writing the registry");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding a loopback port");
    let port = listener.local_addr().expect("the bound address").port();
    let (_shutdown, rx) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        registry_path,
        std::path::PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    tokio::spawn(async move { axum::serve(listener, app).await });

    let launched = Instant::now();
    let url = format!("ws://127.0.0.1:{port}/ws/session?repo=owner%2Flaunch&agent=claude");
    tokio::spawn(async move {
        // Held open until the test ends, so the launch is not abandoned.
        let held = tokio_tungstenite::connect_async(&url).await;
        tokio::time::sleep(VERSION_SLEEP * 2).await;
        drop(held);
    });

    tokio::time::sleep(SECOND_REQUEST_DELAY).await;
    let answer = second_request(port).await;
    // Measured from the launch, not from the end of the sleep: a blocked
    // worker also holds the sleep, and the request would then look fast.
    let waited = launched.elapsed();
    assert!(
        answer.starts_with("HTTP/1.1 "),
        "the second request gets an HTTP answer: {answer:?}"
    );
    assert!(
        waited < SECOND_REQUEST_DELAY + ANSWER_LIMIT,
        "a request sent while a Claude launch reads the version must answer within \
         {ANSWER_LIMIT:?}; it answered {waited:?} after the launch"
    );
}

/// Send `GET /api/identity` and return the status line and headers of the
/// answer.
async fn second_request(port: u16) -> String {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connecting for the second request");
    let request = format!(
        "GET /api/identity HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("sending the second request");
    let mut buf = vec![0u8; 1024];
    let n = stream
        .read(&mut buf)
        .await
        .expect("reading the second answer");
    String::from_utf8_lossy(&buf[..n]).into_owned()
}
