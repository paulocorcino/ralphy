//! A page socket outlives the request the guard checked. Each `channel.open`
//! reads the login again: after a logout, or a revoke of the login the socket
//! was let in under, an open is refused and the socket ends, so the page
//! connects again through the guard.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ralphy_daemon::auth::AuthState;

use crate::support::{ends_within, next_closed, open, Closed, Ws};

const WITHIN: Duration = Duration::from_secs(5);

/// A daemon served on `port` with its store in a temporary directory.
struct Served {
    port: u16,
    _store: tempfile::TempDir,
}

/// Serve a router whose auth state the test keeps, so that it can log every
/// session out as `/api/logout` does.
async fn serve_with_auth(auth: Arc<AuthState>) -> Served {
    let dir = tempfile::tempdir().expect("create a temporary store");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let port = listener
        .local_addr()
        .expect("read the bound address")
        .port();
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let app = ralphy_daemon::router(
        None,
        dir.path().join("repos.toml"),
        dir.path().join("usage"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        shutdown,
        auth,
    );
    tokio::spawn(async move {
        // The shutdown stays open while the server runs.
        let _stop = stop;
        axum::serve(listener, app)
            .await
            .expect("serve the test app");
    });
    Served { port, _store: dir }
}

/// Connect a page socket to the daemon's `/ws`.
async fn connect(daemon: &Served) -> Ws {
    let url = format!("ws://127.0.0.1:{}/ws", daemon.port);
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect to /ws");
    ws
}

/// Open a `spawn` channel whose command no verb serves: a served open gets
/// the command's error and ends `done`.
async fn open_spawn(ws: &mut Ws, channel: u32) {
    let command = serde_json::json!({ "id": 5, "verb": "no.such.verb", "payload": {} });
    open(
        ws,
        channel,
        "spawn",
        serde_json::json!({ "command": command }),
    )
    .await;
}

#[tokio::test]
async fn an_open_after_a_logout_is_refused_and_ends_the_socket() {
    let auth = AuthState::localhost();
    let daemon = serve_with_auth(auth.clone()).await;
    let mut ws = connect(&daemon).await;
    auth.invalidate_sessions().expect("bump the session epoch");

    open_spawn(&mut ws, 1).await;
    let closed = next_closed(&mut ws, WITHIN).await;
    assert_eq!(
        closed,
        Some(Closed {
            channel: 1,
            reason: "refused".to_string()
        })
    );
    assert!(ends_within(&mut ws, WITHIN).await, "the socket ends");
}

/// The control: with no logout, the same open reaches its kind.
#[tokio::test]
async fn an_open_with_the_same_login_is_served() {
    let auth = AuthState::localhost();
    let daemon = serve_with_auth(auth).await;
    let mut ws = connect(&daemon).await;

    open_spawn(&mut ws, 1).await;
    let closed = next_closed(&mut ws, WITHIN).await;
    assert_eq!(
        closed,
        Some(Closed {
            channel: 1,
            reason: "done".to_string()
        })
    );
    assert!(!ends_within(&mut ws, Duration::from_secs(1)).await);
}

/// The seed of the `Session` policy the revoke test signs in under.
const SEED: &[u8] = b"12345678901234567890";

/// The current RFC 6238 code for [`SEED`], computed here: `Seed::code_at` is
/// `pub(crate)`, and a test does not widen the public API.
fn code_now() -> String {
    use hmac::{Hmac, Mac};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("read the system clock")
        .as_secs();
    let mut mac = Hmac::<sha1::Sha1>::new_from_slice(SEED).expect("key the HMAC");
    mac.update(&(now / 30).to_be_bytes());
    let hs = mac.finalize().into_bytes();
    let off = (hs[hs.len() - 1] & 0x0f) as usize;
    let bin = ((hs[off] as u32 & 0x7f) << 24)
        | ((hs[off + 1] as u32) << 16)
        | ((hs[off + 2] as u32) << 8)
        | (hs[off + 3] as u32);
    format!("{:06}", bin % 1_000_000)
}

/// Send one HTTP/1.1 request to the daemon on `port` and read the whole reply.
async fn http(port: u16, request: String) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
    reply
}

/// A form `POST` of `body` to `path`, with the `cookie` pair when given.
async fn post_form(port: u16, path: &str, body: &str, cookie: Option<&str>) -> String {
    let cookie = cookie.map_or(String::new(), |c| format!("Cookie: {c}\r\n"));
    http(
        port,
        format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{cookie}\
             Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            body.len()
        ),
    )
    .await
}

/// Sign in with the current code: the `ralphy_session=…` pair of the reply.
async fn sign_in(port: u16) -> String {
    let reply = post_form(port, "/api/login", &format!("code={}", code_now()), None).await;
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    reply
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, value)| value.trim())
        .find(|value| value.starts_with("ralphy_session="))
        .unwrap_or_else(|| panic!("a session cookie in {reply}"))
        .split(';')
        .next()
        .expect("a cookie pair")
        .to_string()
}

/// A revoke of the login a page socket was let in under reaches its next
/// open: the open is refused and the socket ends.
#[tokio::test]
async fn an_open_after_a_revoke_of_its_login_is_refused_and_ends_the_socket() {
    use ralphy_daemon::auth::{AuthPolicy, RevokedLogins, SessionAuth};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let epoch = ralphy_daemon::epoch::SessionEpoch::in_memory_detached();
    let session = Arc::new(SessionAuth {
        token: "tok".to_string(),
        totp: ralphy_daemon::totp::Seed::from_bytes(SEED.to_vec()),
        password: None,
        epoch: epoch.clone(),
        revoked: RevokedLogins::default(),
    });
    let policy = AuthPolicy::Session(session.clone());
    let auth = AuthState::fixed(policy, epoch);
    let daemon = serve_with_auth(auth.clone()).await;
    let cookie = sign_in(daemon.port).await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("read the system clock")
        .as_secs();
    let login = session
        .cookie_login(Some(&cookie), now)
        .expect("the cookie names a live login");

    let mut request = format!("ws://127.0.0.1:{}/ws", daemon.port)
        .into_client_request()
        .expect("build the upgrade request");
    request
        .headers_mut()
        .insert("cookie", cookie.parse().expect("a cookie header value"));
    let (mut ws, _resp) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connect to /ws with the session cookie");

    let reply = post_form(
        daemon.port,
        "/api/security/logins/revoke",
        &format!("login={login}"),
        Some(&cookie),
    )
    .await;
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    assert!(auth.login_revoked(&login), "the login is on the list");

    open_spawn(&mut ws, 1).await;
    let closed = next_closed(&mut ws, WITHIN).await;
    assert_eq!(
        closed,
        Some(Closed {
            channel: 1,
            reason: "refused".to_string()
        })
    );
    assert!(ends_within(&mut ws, WITHIN).await, "the socket ends");
}
