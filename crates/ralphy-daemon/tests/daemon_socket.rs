//! Each account's daemon also listens on `daemon.sock` in its own store, so
//! several accounts on one host can each be reached while only one of them
//! holds the TCP port (#518, M1).
//!
//! Unix only: tokio has no Unix socket on Windows, and a Windows daemon serves
//! TCP alone (#518, amendment M6).
#![cfg(unix)]

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ralphy_daemon::auth::{AuthPolicy, AuthState};
use ralphy_daemon::epoch::SessionEpoch;
use ralphy_daemon::identity::Identity;
use ralphy_daemon::{router, socket};

const A_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const B_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const TOKEN: &str = "sock-tok";

fn app(id: &str, store: &Path, auth: Arc<AuthState>) -> axum::Router {
    let (tx, rx) = tokio::sync::watch::channel(false);
    std::mem::forget(tx);
    router(
        Some(Identity {
            id: id.parse().expect("a valid ULID"),
            name: id.to_string(),
            avatar: "🐙".to_string(),
        }),
        store.join("repos.toml"),
        PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        auth,
    )
}

/// Bind the store's socket and serve `app` on it, the way the daemon does.
fn serve_socket(store: &Path, app: axum::Router) -> tokio::task::JoinHandle<()> {
    let bound = socket::bind(store)
        .expect("the socket binds")
        .expect("a temp store path fits in sun_path");
    let (listener, _remove) = bound.into_parts();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serving the socket");
    })
}

/// One HTTP/1.1 GET over the store's socket: the status and the body.
async fn get(store: &Path, uri: &str, bearer: Option<&str>) -> (u16, String) {
    let path = socket::socket_path(store);
    let mut request = format!("GET {uri} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    if let Some(token) = bearer {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    request.push_str("\r\n");
    let response = tokio::task::spawn_blocking(move || {
        let mut stream = std::os::unix::net::UnixStream::connect(&path).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("a read timeout");
        stream.write_all(request.as_bytes()).expect("write");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read");
        response
    })
    .await
    .expect("the request thread");
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .expect("a status line");
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    (status, body)
}

fn daemon_id(body: &str) -> String {
    let json: serde_json::Value = serde_json::from_str(body).expect("a JSON body");
    json["daemon_id"].as_str().expect("a daemon_id").to_string()
}

#[tokio::test]
async fn two_accounts_each_answer_on_their_own_socket_while_another_holds_the_port() {
    // The TCP port is held by a third listener, as by another account's daemon.
    let _port = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a TCP port");
    let a = tempfile::tempdir().expect("a temp store");
    let b = tempfile::tempdir().expect("a temp store");
    let _a = serve_socket(a.path(), app(A_ID, a.path(), AuthState::localhost()));
    let _b = serve_socket(b.path(), app(B_ID, b.path(), AuthState::localhost()));

    let (status, body) = get(a.path(), "/api/peer/hello", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(daemon_id(&body), A_ID);
    let (status, body) = get(b.path(), "/api/peer/hello", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(daemon_id(&body), B_ID);
}

#[tokio::test]
async fn a_stale_socket_file_is_replaced() {
    let store = tempfile::tempdir().expect("a temp store");
    // A socket nobody answers: its listener is gone, its file is not.
    let stale = std::os::unix::net::UnixListener::bind(socket::socket_path(store.path()))
        .expect("a stale socket");
    drop(stale);
    assert!(socket::socket_path(store.path()).exists());

    let _served = serve_socket(
        store.path(),
        app(A_ID, store.path(), AuthState::localhost()),
    );
    let (status, body) = get(store.path(), "/api/peer/hello", None).await;
    assert_eq!(status, 200, "{body}");
}

#[test]
fn a_file_that_is_not_a_socket_is_never_deleted() {
    let store = tempfile::tempdir().expect("a temp store");
    let path = socket::socket_path(store.path());
    std::fs::write(&path, "not a socket").expect("a plain file");
    assert!(socket::bind(store.path()).is_err(), "the bind must refuse");
    assert_eq!(
        std::fs::read_to_string(&path).expect("the file is kept"),
        "not a socket"
    );
}

#[tokio::test]
async fn a_live_socket_makes_the_second_bind_fail() {
    let store = tempfile::tempdir().expect("a temp store");
    let _served = serve_socket(
        store.path(),
        app(A_ID, store.path(), AuthState::localhost()),
    );

    let second = socket::bind(store.path());
    let err = second
        .err()
        .expect("a second daemon of the account is refused");
    assert!(
        format!("{err:#}").contains("already listens"),
        "unexpected error: {err:#}"
    );
    // The first daemon still answers: its file was not replaced.
    let (status, _) = get(store.path(), "/api/peer/hello", None).await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn the_socket_is_owner_only() {
    let store = tempfile::tempdir().expect("a temp store");
    let _served = serve_socket(
        store.path(),
        app(A_ID, store.path(), AuthState::localhost()),
    );
    let mode = std::fs::metadata(socket::socket_path(store.path()))
        .expect("the socket file")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[tokio::test]
async fn the_bearer_policy_holds_on_the_socket() {
    let store = tempfile::tempdir().expect("a temp store");
    let auth = AuthState::fixed(
        AuthPolicy::Bearer(TOKEN.to_string()),
        SessionEpoch::in_memory_detached(),
    );
    let _served = serve_socket(store.path(), app(A_ID, store.path(), auth));

    let (status, _) = get(store.path(), "/api/peer/hello", None).await;
    assert_eq!(status, 401);
    let (status, body) = get(store.path(), "/api/peer/hello", Some(TOKEN)).await;
    assert_eq!(status, 200, "{body}");
}

#[tokio::test]
async fn the_daemon_removes_only_its_own_socket_file() {
    let store = tempfile::tempdir().expect("a temp store");
    let path = socket::socket_path(store.path());
    let (listener, remove) = socket::bind(store.path())
        .expect("the socket binds")
        .expect("a temp store path fits in sun_path")
        .into_parts();
    drop(listener);
    remove();
    assert!(!path.exists(), "the daemon removes its own socket at exit");

    let bound = socket::bind(store.path())
        .expect("the socket binds")
        .expect("a temp store path fits in sun_path");
    let (listener, remove) = bound.into_parts();
    drop(listener);
    // Another daemon bound the name while this one was stopping. The old
    // file is renamed, not removed: ext4 reuses a freed inode number at
    // once, and the new socket would then look like this daemon's own.
    std::fs::rename(&path, store.path().join("old.sock")).expect("rename");
    let _other = std::os::unix::net::UnixListener::bind(&path).expect("another socket");
    remove();
    assert!(path.exists(), "the other daemon's socket is kept");
}

#[test]
fn a_socket_that_still_answers_is_never_removed() {
    // The same device and inode, as when a newer daemon's file reuses a freed
    // inode number: the file still answers, so it is not this daemon's.
    let store = tempfile::tempdir().expect("a temp store");
    let path = socket::socket_path(store.path());
    let rt = tokio::runtime::Runtime::new().expect("a runtime");
    let _guard = rt.enter();
    let (listener, remove) = socket::bind(store.path())
        .expect("the socket binds")
        .expect("a temp store path fits in sun_path")
        .into_parts();
    remove();
    assert!(path.exists(), "a socket that answers is kept");
    drop(listener);
}
