//! `GET /api/audit/logins/notices` and `POST /api/audit/logins/dismiss`
//! (issue #717): a login used from a second device is a notice until its
//! login is revoked, the pair is dismissed, or 30 days pass. The audit log is
//! written by the test as the daemon writes it, in a store of its own.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use ralphy_daemon::auth::AuthState;
use ralphy_daemon::cookie::LoginId;
use ralphy_daemon::router;
use serde_json::{json, Value};
use tower::ServiceExt;

#[path = "support/golden.rs"]
mod golden;

/// A login ID in its one canonical spelling (16 zero bytes).
const LOGIN: &str = "AAAAAAAAAAAAAAAAAAAAAA";
const OWN: &str = "0123456789abcdef0123456789abcdef";
const OTHER: &str = "fedcba9876543210fedcba9876543210";

/// A router over `auth` whose store is `dir`.
fn app(auth: &Arc<AuthState>, dir: &Path) -> axum::Router {
    let (tx, rx) = tokio::sync::watch::channel(false);
    std::mem::forget(tx);
    router(
        None,
        dir.join("repos.toml"),
        dir.join("usage"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        Arc::clone(auth),
    )
}

fn log_path(dir: &Path) -> PathBuf {
    dir.join("daemon-audit.jsonl")
}

/// The time `days_ago` days before now, as the log writes it.
fn at(days_ago: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::days(days_ago))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// A log where `login` signed in from [`OWN`] and was then used from
/// [`OTHER`], whose page reported its facts, `days_ago` days ago.
fn write_log(dir: &Path, login: &str, days_ago: i64) {
    let t = at(days_ago);
    let lines = [
        json!({"at": t, "event": "login_ok", "device": OWN, "login": login, "actor": "device"}),
        json!({
            "at": t,
            "event": "device_facts",
            "device": OTHER,
            "actor": "device",
            "ip": "203.0.113.5",
            "normalizer": 1,
            "os": {"value": "android", "source": "ua"},
            "browser": {"value": "chrome", "source": "ua"},
            "form": {"value": "phone", "source": "ua"},
        }),
        json!({
            "at": t,
            "event": "login_other_device",
            "device": OTHER,
            "login": login,
            "actor": "device",
            "did": "console_launch",
            "ip": "203.0.113.5",
        }),
    ]
    .map(|line| line.to_string());
    std::fs::write(log_path(dir), lines.join("\n") + "\n").expect("write the audit log");
}

fn get_notices() -> Request<Body> {
    Request::builder()
        .uri("/api/audit/logins/notices")
        .body(Body::empty())
        .expect("build the request")
}

fn dismiss(body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/audit/logins/dismiss")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
        .expect("build the request")
}

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("read the response body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("a JSON reply")
}

async fn notices(auth: &Arc<AuthState>, dir: &Path) -> Vec<Value> {
    let resp = app(auth, dir).oneshot(get_notices()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let reply = body_json(resp).await;
    reply["notices"]
        .as_array()
        .expect("a notices array")
        .clone()
}

fn dismissed_lines(dir: &Path) -> usize {
    std::fs::read_to_string(log_path(dir))
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains(r#""event":"login_notice_dismissed""#))
        .count()
}

#[tokio::test]
async fn a_reported_pair_is_listed() {
    let dir = tempfile::tempdir().unwrap();
    write_log(dir.path(), LOGIN, 1);
    // A second login, used from a device whose page never reported facts.
    let (second, bare) = (LoginId::mint(), "00112233445566778899aabbccddeeff");
    let t = at(0);
    let second_id = second.to_string();
    let more = [
        json!({"at": t, "event": "login_ok", "device": OWN, "login": second_id, "actor": "device"}),
        json!({
            "at": t,
            "event": "login_other_device",
            "device": bare,
            "login": second_id,
            "actor": "device",
            "did": "branch.switch",
        }),
    ]
    .map(|line| line.to_string());
    let mut text = std::fs::read_to_string(log_path(dir.path())).expect("read the audit log");
    text.push_str(&(more.join("\n") + "\n"));
    std::fs::write(log_path(dir.path()), text).expect("write the audit log");

    let list = notices(&AuthState::localhost(), dir.path()).await;
    assert_eq!(list.len(), 2, "{list:?}");
    assert_eq!(list[0]["login"], second.to_string(), "the newest first");
    assert_eq!(list[0]["device"], bare);
    assert!(
        list[0]["facts"].is_null(),
        "a device with no facts: {list:?}"
    );
    let notice = &list[1];
    assert_eq!(notice["login"], LOGIN);
    assert_eq!(notice["device"], OTHER);
    assert_eq!(notice["did"], "console_launch");
    assert_eq!(notice["at"], at(1));
    assert_eq!(notice["facts"]["os"], "android");
    assert_eq!(notice["facts"]["browser"], "chrome");
    assert_eq!(notice["facts"]["form"], "phone");
}

#[tokio::test]
async fn a_revoked_login_is_not_listed() {
    let dir = tempfile::tempdir().unwrap();
    let revoked = LoginId::mint();
    write_log(dir.path(), &revoked.to_string(), 1);
    let auth = AuthState::localhost();
    assert_eq!(notices(&auth, dir.path()).await.len(), 1);
    auth.revoke_login(revoked, chrono::Utc::now().timestamp() as u64)
        .expect("revoke the login");
    let list = notices(&auth, dir.path()).await;
    assert!(list.is_empty(), "{list:?}");
}

#[tokio::test]
async fn a_malformed_dismiss_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    write_log(dir.path(), LOGIN, 1);
    let auth = AuthState::localhost();
    let bad = [
        format!("login=AAAAAAAAAAAAAAAAAAAAA&device={OTHER}"),
        format!("login={LOGIN}&device=not-a-device"),
        format!("device={OTHER}"),
        format!("login={LOGIN}"),
    ];
    for body in bad {
        let resp = app(&auth, dir.path())
            .oneshot(dismiss(&body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{body}");
    }
    assert_eq!(
        dismissed_lines(dir.path()),
        0,
        "a refused dismiss writes nothing"
    );
    assert_eq!(notices(&auth, dir.path()).await.len(), 1);
}

#[tokio::test]
async fn a_dismissed_pair_is_not_listed() {
    let dir = tempfile::tempdir().unwrap();
    write_log(dir.path(), LOGIN, 1);
    let auth = AuthState::localhost();
    for _ in 0..2 {
        let resp = app(&auth, dir.path())
            .oneshot(dismiss(&format!("login={LOGIN}&device={OTHER}")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
    let list = notices(&auth, dir.path()).await;
    assert!(list.is_empty(), "{list:?}");
    assert_eq!(
        dismissed_lines(dir.path()),
        1,
        "a second dismiss writes no line"
    );
}

#[tokio::test]
async fn the_notice_replies_match_their_fixtures() {
    let dir = tempfile::tempdir().unwrap();
    write_log(dir.path(), LOGIN, 1);
    let auth = AuthState::localhost();
    let resp = app(&auth, dir.path()).oneshot(get_notices()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    golden::check(
        "api-audit-logins-notices",
        body_json(resp).await,
        &[
            "/notices/0/at",
            "/notices/0/facts/first_seen",
            "/notices/0/facts/last_seen",
        ],
    );
    let resp = app(&auth, dir.path())
        .oneshot(dismiss(&format!("login={LOGIN}&device={OTHER}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    golden::check("api-audit-logins-dismiss", body_json(resp).await, &[]);
}

/// The payload of the next `verb` push on the page socket; panics after 10 s.
async fn next_push<S>(ws: &mut S, verb: &str) -> Value
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

/// A dismiss on the served router puts `logins.dirty` on the page socket.
#[tokio::test]
async fn a_notice_change_pushes_logins_dirty() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let dir = tempfile::tempdir().unwrap();
    write_log(dir.path(), LOGIN, 1);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a port");
    let port = listener.local_addr().expect("a bound address").port();
    let served = app(&AuthState::localhost(), dir.path());
    tokio::spawn(async move {
        axum::serve(listener, served)
            .await
            .expect("the daemon serves");
    });
    let (mut page, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .expect("connect the page socket");

    let body = format!("login={LOGIN}&device={OTHER}");
    let request = format!(
        "POST /api/audit/logins/dismiss HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
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

    assert_eq!(
        next_push(&mut page, "logins.dirty").await,
        serde_json::json!({})
    );
}
