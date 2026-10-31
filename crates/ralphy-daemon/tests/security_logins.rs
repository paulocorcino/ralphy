//! `POST /api/security/logins/revoke` (issue #717): the form `login=<id>` puts
//! one login on the revoked-login list and answers `{"revoked":true}`. Drives
//! the `Router` in-process via `ServiceExt::oneshot` (no socket).
//!
//! Only `a_failed_write_changes_nothing` sets `RALPHY_DAEMON_DIR`; the other
//! tests use a detached store and read no environment.

#![expect(
    clippy::disallowed_methods,
    reason = "nextest runs each test in its own process, so a test may change its environment"
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use ralphy_daemon::auth::{AuthPolicy, AuthState, RevokedLogins, SessionAuth};
use ralphy_daemon::cookie::LoginId;
use ralphy_daemon::{epoch, router, totp};
use tower::ServiceExt;

#[path = "support/golden.rs"]
mod golden;

/// A router over `auth`, with no store of its own.
fn app(auth: &Arc<AuthState>) -> axum::Router {
    let (tx, rx) = tokio::sync::watch::channel(false);
    std::mem::forget(tx);
    router(
        None,
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        Arc::clone(auth),
    )
}

fn revoke(body: String) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/security/logins/revoke")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .expect("build the request")
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("read the response body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("a JSON reply")
}

#[tokio::test]
async fn a_malformed_login_id_is_refused() {
    let auth = AuthState::localhost();
    // 21 characters, one too few; 22 characters with one outside base64url;
    // and no ID at all.
    for bad in ["AAAAAAAAAAAAAAAAAAAAA", "AAAAAAAAAAAAAAAAAAAAA+", ""] {
        let resp = app(&auth)
            .oneshot(revoke(format!("login={bad}")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "login={bad}");
    }
}

#[tokio::test]
async fn the_revoke_reply_matches_its_fixture() {
    let auth = AuthState::localhost();
    let login = LoginId::mint();
    let resp = app(&auth)
        .oneshot(revoke(format!("login={login}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(auth.login_revoked(&login), "the login is on the list");
    golden::check("api-security-logins-revoke", body_json(resp).await, &[]);
}

/// The payload of the next `verb` push on the page socket; panics after 10 s.
async fn next_push<S>(ws: &mut S, verb: &str) -> serde_json::Value
where
    S: futures_util::Stream<
            Item = Result<
                tokio_tungstenite::tungstenite::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + Unpin,
{
    use futures_util::StreamExt;
    use ralphy_daemon::protocol::{self, Frame};
    let read = async {
        while let Some(Ok(msg)) = ws.next().await {
            let tokio_tungstenite::tungstenite::Message::Binary(bytes) = msg else {
                continue;
            };
            match protocol::decode(&bytes) {
                Ok(Frame::Command(c)) if c.verb == verb => return c.payload,
                _ => continue,
            }
        }
        panic!("the page socket closed before {verb}");
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), read)
        .await
        .unwrap_or_else(|_| panic!("no {verb} within 10 s"))
}

#[tokio::test]
async fn a_revoke_pushes_logins_dirty() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let auth = AuthState::localhost();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a port");
    let port = listener.local_addr().expect("a bound address").port();
    let served = app(&auth);
    tokio::spawn(async move {
        axum::serve(listener, served)
            .await
            .expect("the daemon serves");
    });
    let (mut page, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .expect("connect the page socket");

    let body = format!("login={}", LoginId::mint());
    let request = format!(
        "POST /api/security/logins/revoke HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
         Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to the daemon");
    sock.write_all(request.as_bytes())
        .await
        .expect("send the request");
    let mut reply = String::new();
    sock.read_to_string(&mut reply)
        .await
        .expect("read the reply");
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");

    let pushed = next_push(&mut page, "logins.dirty").await;
    golden::check("logins.dirty", pushed, &[]);
}

#[tokio::test]
async fn revoke_needs_a_login() {
    let session_epoch = epoch::SessionEpoch::in_memory_detached();
    let policy = AuthPolicy::Session(Arc::new(SessionAuth {
        token: "tok".to_string(),
        totp: totp::Seed::from_bytes(b"12345678901234567890".to_vec()),
        password: None,
        epoch: session_epoch.clone(),
        revoked: RevokedLogins::default(),
    }));
    let auth = AuthState::fixed(policy, session_epoch);
    let login = LoginId::mint();
    let resp = app(&auth)
        .oneshot(revoke(format!("login={login}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(
        !auth.login_revoked(&login),
        "the refused request revoked nothing"
    );
}

/// Make every later write of the list at `path` fail, and keep it readable.
/// The owner-only write creates `<file>.tmp-<pid>-<n>` beside the file, `n`
/// counting the writes of this process, and renames it over the file: a
/// directory under each of those names stops the write before the rename.
fn block_writes(path: &std::path::Path) {
    let name = path
        .file_name()
        .expect("the list path names a file")
        .to_string_lossy();
    for n in 0..256 {
        let tmp = path.with_file_name(format!("{name}.tmp-{}-{n}", std::process::id()));
        // A file under the name would be replaced; a directory is not.
        std::fs::create_dir_all(tmp.join("held")).expect("hold a temporary name");
    }
}

#[tokio::test]
async fn a_failed_write_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("RALPHY_DAEMON_DIR", dir.path());
    let auth = AuthState::boot(
        "127.0.0.1:0".parse().unwrap(),
        None,
        epoch::SessionEpoch::in_memory_detached(),
        &[],
    )
    .expect("boot on an empty store");
    let (first, second) = (LoginId::mint(), LoginId::mint());
    let resp = app(&auth)
        .oneshot(revoke(format!("login={first}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let path = ralphy_daemon::auth::revoked_logins_path_in(dir.path());
    let before = std::fs::read(&path).expect("the list is written");

    block_writes(&path);
    let resp = app(&auth)
        .oneshot(revoke(format!("login={second}")))
        .await
        .unwrap();
    let after = std::fs::read(&path).expect("the list is still readable");

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(after, before, "the file is unchanged");
    assert!(auth.login_revoked(&first), "the first revoke stands");
    assert!(
        !auth.login_revoked(&second),
        "the failed revoke is not live"
    );
}
