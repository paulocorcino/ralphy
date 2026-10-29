//! A main daemon and its peer (docs/adr/0085 D14): the main claims the peer,
//! reads the peer's own list on its tick, and sends the peer's console
//! requests with its own bot. The main is its [`Notifier`] and its
//! [`Claimer`] over a temp store, with a fake transport; the peer is a daemon
//! router on a port of its own, or a fake peer that answers a fixed list.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{bail, Result};
use axum::routing::{get, post};
use serde_json::{json, Value};

use super::*;
use crate::auth::{AuthPolicy, AuthState};
use crate::peer::telegram_claim::{claim_once, Claimed, Claimer};
use crate::peer::{self, PeerDescriptor, PEER_PROTOCOL_VERSION};
use crate::protocol;

const MAIN: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const PEER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const TOKEN: &str = "peer-token";
const BOT: &str = "token = \"123:abc\"\nchat_id = 42\n";
const ORIGIN: &str = "https://main.example:8443";

/// A transport that keeps the text of each `sendMessage`.
#[derive(Clone, Default)]
struct Texts(Arc<Mutex<Vec<String>>>);

impl Transport for Texts {
    fn get(&self, method: &str) -> Result<Value> {
        bail!("the sender calls no GET, got {method}")
    }

    fn post(&self, method: &str, body: Value) -> Result<Value> {
        assert_eq!(method, "sendMessage");
        let text = body["text"].as_str().unwrap_or_default().to_string();
        self.0.lock().expect("the texts lock").push(text);
        Ok(json!({ "ok": true, "result": { "message_id": 1 } }))
    }
}

impl Texts {
    fn all(&self) -> Vec<String> {
        self.0.lock().expect("the texts lock").clone()
    }
}

/// The main daemon: its store, its sender and its claimer.
struct Main {
    store: tempfile::TempDir,
    notifier: Notifier,
    claimer: Claimer,
    texts: Texts,
}

/// A main daemon over a temp store, with a bot when `with_bot`. The origin
/// of its last page is [`ORIGIN`], and that page is hidden.
fn main_daemon(with_bot: bool) -> Main {
    let store = tempfile::tempdir().expect("a temp dir");
    let config = store.path().join("telegram.toml");
    if with_bot {
        std::fs::write(&config, BOT).expect("write the config");
    }
    let board = presence_board::Shared::default();
    {
        let mut board = board.lock().expect("the board lock");
        let conn = board.open();
        let report = json!({ "visible": false, "lastInput": null, "now": 1.0, "origin": ORIGIN });
        assert!(board.report(conn, &report, SystemTime::now()));
    }
    let claim = SharedClaim::default();
    let peers_dir = store.path().join("peers");
    let texts = Texts::default();
    let mut notifier = Notifier::new(
        config.clone(),
        (board, claim.clone()),
        peers_dir.clone(),
        Some(MAIN.to_string()),
    );
    let fake = texts.clone();
    notifier.transport = Arc::new(move |_token| AnyTransport(Box::new(fake.clone())));
    let claimer = Claimer {
        config,
        claim,
        peers_dir,
        daemon_id: MAIN.to_string(),
    };
    Main {
        store,
        notifier,
        claimer,
        texts,
    }
}

impl Main {
    /// Put the peer on `port` in the main's peers folder.
    fn announce(&self, port: u16) {
        let descriptor = PeerDescriptor {
            daemon_id: PEER.to_string(),
            name: "peer".to_string(),
            avatar: "🐙".to_string(),
            address: "127.0.0.1".to_string(),
            port,
            environment: "WSL: Ubuntu".to_string(),
            os: String::new(),
            token: TOKEN.to_string(),
            protocol_version: PEER_PROTOCOL_VERSION,
            tunnel: None,
            nudge: None,
        };
        peer::write_descriptor(self.store.path(), &descriptor).expect("announce the peer");
    }

    /// One tick of the main with its own `requests`, and the end of the
    /// send it started.
    async fn tick(&mut self, requests: &[AttentionRequest]) {
        self.notifier
            .tick_at(requests, &[], SystemTime::now())
            .await;
        if let Some(flight) = self.notifier.flight.take() {
            let delivery = flight.await.expect("the send ends");
            self.notifier.sender.finish(delivery);
        }
    }
}

/// A console request `console:7`, opened `open_secs` ago.
fn waiting(open_secs: u64) -> AttentionRequest {
    let since = SystemTime::now() - Duration::from_secs(open_secs);
    AttentionRequest {
        id: "console:7".to_string(),
        kind: Kind::ConsoleWaiting,
        vendor: "claude".to_string(),
        console: Some("7".to_string()),
        repo: Some("owner/app".to_string()),
        run: None,
        issue: None,
        since: chrono::DateTime::<chrono::Utc>::from(since)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        detail: Some("permission: Bash".to_string()),
        dismissable: false,
    }
}

/// Serve `app` on a port of its own.
async fn serve(app: axum::Router) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a port");
    let port = listener.local_addr().expect("a bound address").port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("the peer serves");
    });
    port
}

/// A fake peer: its own list is `list` (no answer while it is `None`), and
/// it has a claim route unless `old`.
async fn fake_peer(list: Arc<Mutex<Option<Value>>>, old: bool) -> u16 {
    let read = move || {
        let list = list.lock().expect("the list lock").clone();
        async move {
            match list {
                Some(list) => axum::Json(list).into_response(),
                None => axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response(),
            }
        }
    };
    let mut app = axum::Router::new().route("/api/attention", get(read));
    if !old {
        app = app.route(
            "/api/telegram/main",
            post(|| async { axum::Json(json!({ "state": "off" })) }),
        );
    }
    serve(app).await
}

use axum::response::IntoResponse as _;

/// A daemon router as a peer: its own store in `home`, a bot when
/// `with_bot`, the peer token.
async fn real_peer(home: &std::path::Path, with_bot: bool) -> u16 {
    let config = home.join("telegram.toml");
    if with_bot {
        std::fs::write(&config, BOT).expect("write the config");
    }
    let (tx, shutdown) = tokio::sync::watch::channel(false);
    let app = crate::router(
        Some(crate::identity::Identity {
            id: PEER.parse().expect("a daemon id"),
            name: "peer".to_string(),
            avatar: "🐙".to_string(),
        }),
        home.join("repos.toml"),
        PathBuf::from("does-not-exist"),
        crate::StorePaths {
            telegram_config: config,
            ..Default::default()
        },
        Instant::now(),
        shutdown,
        AuthState::fixed(
            AuthPolicy::Bearer(TOKEN.to_string()),
            crate::epoch::SessionEpoch::in_memory_detached(),
        ),
    );
    // The shutdown sender lives as long as the test's runtime.
    std::mem::forget(tx);
    serve(app).await
}

/// The descriptor the main holds of its peer.
fn peer_of(main: &Main) -> PeerDescriptor {
    let (peers, _) = peer::read_store(&main.claimer.peers_dir);
    peers.into_iter().next().expect("the main has its peer")
}

/// The JSON a peer answers on `path`.
async fn peer_reply(main: &Main, path: &str) -> Value {
    let (status, body) = peer::client::get(&peer_of(main), path)
        .await
        .expect("the peer answers");
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).expect("JSON")
}

#[tokio::test]
async fn a_peer_through_its_main_sends_nothing() {
    let mut peer = main_daemon(true);
    peer.claimer
        .claim
        .lock()
        .expect("the claim lock")
        .take("m".to_string(), SystemTime::now());
    for _ in 0..3 {
        peer.tick(&[waiting(600)]).await;
    }
    assert_eq!(peer.texts.all(), Vec::<String>::new(), "its main sends");

    let mut alone = main_daemon(true);
    alone.tick(&[waiting(600)]).await;
    assert_eq!(alone.texts.all().len(), 1, "with no claim it sends");
}

#[tokio::test]
async fn a_main_without_a_config_leaves_the_peer_alone() {
    let home = tempfile::tempdir().expect("a temp dir");
    let port = real_peer(home.path(), true).await;
    let main = main_daemon(false);
    main.announce(port);

    assert_eq!(claim_once(&main.claimer).await, Vec::new(), "no claim");
    assert_eq!(
        peer_reply(&main, "/api/telegram?local=1").await,
        json!({ "state": "sends" }),
        "the peer sends by itself"
    );
}

#[tokio::test]
async fn the_main_sends_a_peer_console_request_once() {
    let list = Arc::new(Mutex::new(Some(json!({
        "requests": [waiting(121)],
        "present": false,
    }))));
    let port = fake_peer(list.clone(), false).await;
    let mut main = main_daemon(true);
    main.announce(port);
    assert_eq!(
        claim_once(&main.claimer).await,
        vec![(PEER.to_string(), Claimed::Taken)]
    );

    main.tick(&[]).await;
    // The peer misses a tick, then answers again: still one message.
    *list.lock().expect("the list lock") = None;
    main.tick(&[]).await;
    *list.lock().expect("the list lock") = Some(json!({
        "requests": [waiting(130)],
        "present": false,
    }));
    main.tick(&[]).await;

    let texts = main.texts.all();
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(
        texts[0].ends_with(&format!("\n{ORIGIN}/?console={PEER}/7")),
        "{}",
        texts[0]
    );
    assert!(texts[0].contains(" in owner/app "), "{}", texts[0]);
}

#[tokio::test]
async fn a_page_on_the_peer_counts_as_present() {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message as Wire};
    let home = tempfile::tempdir().expect("a temp dir");
    let port = real_peer(home.path(), false).await;
    let mut main = main_daemon(true);
    main.announce(port);

    let mut request = format!("ws://127.0.0.1:{port}/ws")
        .into_client_request()
        .expect("a websocket request");
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {TOKEN}").parse().expect("a header value"),
    );
    let (mut page, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("a page on the peer");
    let now = 1_000_000.0;
    let report = protocol::encode(&protocol::Frame::Command(protocol::Command {
        id: 0,
        verb: "presence.report".to_string(),
        payload: json!({ "visible": true, "lastInput": now, "now": now, "origin": "http://127.0.0.1:1" }),
    }));
    page.send(Wire::Binary(report.into()))
        .await
        .expect("send the report");
    let deadline = Instant::now() + Duration::from_secs(10);
    while peer_reply(&main, "/api/attention?local=1").await["present"] != json!(true) {
        assert!(Instant::now() < deadline, "the peer never saw its page");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    main.tick(&[waiting(121)]).await;
    assert_eq!(main.texts.all(), Vec::<String>::new(), "a page on the peer");

    page.close(None).await.expect("close the page");
    drop(page);
    let deadline = Instant::now() + Duration::from_secs(10);
    while peer_reply(&main, "/api/attention?local=1").await["present"] != json!(false) {
        assert!(Instant::now() < deadline, "the peer kept its closed page");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    main.tick(&[waiting(121)]).await;
    assert_eq!(main.texts.all().len(), 1, "no page anywhere");
}

/// A log the test reads back.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("the log lock").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Log {
    type Writer = Log;
    fn make_writer(&'a self) -> Log {
        self.clone()
    }
}

#[tokio::test]
async fn an_old_peer_does_not_break_the_main() {
    let list = Arc::new(Mutex::new(Some(json!({ "requests": [waiting(121)] }))));
    let port = fake_peer(list, true).await;
    let mut main = main_daemon(true);
    main.announce(port);

    let log = Log::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(log.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .finish();
    let claims = {
        let _logging = tracing::subscriber::set_default(subscriber);
        claim_once(&main.claimer).await
    };
    assert_eq!(claims, vec![(PEER.to_string(), Claimed::OldPeer)]);
    let logged = String::from_utf8_lossy(&log.0.lock().expect("the log lock")).into_owned();
    assert!(logged.contains("older version"), "{logged}");
    assert!(!logged.contains("WARN"), "{logged}");

    main.tick(&[]).await;
    let texts = main.texts.all();
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(
        texts[0].contains(&format!("?console={PEER}/7")),
        "{}",
        texts[0]
    );
}
