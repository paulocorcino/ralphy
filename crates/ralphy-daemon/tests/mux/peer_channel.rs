//! The client of a peer's page socket, `peer::client::open_channel`, against
//! a real peer daemon on a loopback port, and against a stand-in peer whose
//! handshake answers an older protocol.

#![expect(
    clippy::disallowed_methods,
    reason = "nextest runs each test in its own process, so a test may change its environment"
)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ralphy_daemon::auth::{AuthPolicy, AuthState};
use ralphy_daemon::epoch::SessionEpoch;
use ralphy_daemon::identity::Identity;
use ralphy_daemon::peer::client::{
    open_channel, PeerChannel, PeerEvent, PeerStatus, SelfRef, SocketError,
};
use ralphy_daemon::peer::{PeerDescriptor, PEER_PROTOCOL_VERSION};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};

const LOCAL_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
const PEER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB1";
const SLUG: &str = "owner/shared";
const ENVIRONMENT: &str = "Linux: peer";
const TOKEN: &str = "peer-token";
const WITHIN: Duration = Duration::from_secs(15);

/// The largest piece one channel frame carries, as the check states it.
const PIECE_CAP: usize = 16 * 1024;

/// The dialling daemon: a port no peer in these tests listens on.
const ME: SelfRef<'static> = SelfRef {
    port: 1,
    daemon_id: LOCAL_ID,
};

/// Point the session launcher at `session_test_child`, once.
fn point_launcher_at_test_child() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::env::set_var(
            "RALPHY_DAEMON_AGENT_OVERRIDE",
            env!("CARGO_BIN_EXE_session_test_child"),
        );
    });
}

/// The peer pings every 300 ms and ends a socket silent for 675 ms. Set
/// before the peer starts: the period is read once per process.
fn fast_pings() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| std::env::set_var("RALPHY_DAEMON_WS_PING_MS", "300"));
}

/// A peer daemon that takes the bearer `TOKEN` and hosts `SLUG`.
struct Peer {
    port: u16,
    _shutdown: tokio::sync::watch::Sender<bool>,
    _store: tempfile::TempDir,
    _repo: tempfile::TempDir,
}

fn descriptor(port: u16) -> PeerDescriptor {
    PeerDescriptor {
        daemon_id: PEER_ID.to_string(),
        name: "peer".to_string(),
        avatar: "🐙".to_string(),
        address: "127.0.0.1".to_string(),
        port,
        environment: ENVIRONMENT.to_string(),
        os: String::new(),
        token: TOKEN.to_string(),
        protocol_version: PEER_PROTOCOL_VERSION,
        tunnel: None,
        nudge: None,
    }
}

async fn bind() -> (tokio::net::TcpListener, u16) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let port = listener
        .local_addr()
        .expect("read the bound address")
        .port();
    (listener, port)
}

async fn peer() -> Peer {
    point_launcher_at_test_child();
    let store = tempfile::tempdir().expect("create a temporary store");
    let repo = tempfile::tempdir().expect("create a temporary checkout");
    let registry_path = store.path().join("repos.toml");
    let mut repos = registry::RegistryStore::default();
    repos.upsert(SLUG, &repo.path().to_string_lossy());
    registry::save_to(&repos, &registry_path).expect("save the registry");
    let identity = Identity {
        id: PEER_ID.parse().expect("the test id parses"),
        name: "peer".to_string(),
        avatar: "🐙".to_string(),
    };
    let auth = AuthState::fixed(
        AuthPolicy::Bearer(TOKEN.to_string()),
        SessionEpoch::in_memory_detached(),
    );
    let (listener, port) = bind().await;
    let (shutdown, rx) = tokio::sync::watch::channel(false);
    let app = router(
        Some(identity),
        registry_path,
        PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        auth,
    );
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve the test app");
    });
    Peer {
        port,
        _shutdown: shutdown,
        _store: store,
        _repo: repo,
    }
}

/// Open a channel of `kind` on the peer, or fail the test with why not.
async fn open(peer: &Peer, kind: &str, fields: serde_json::Value) -> PeerChannel {
    match open_channel(&descriptor(peer.port), ME, kind, fields).await {
        Ok(channel) => channel,
        Err(SocketError::Peer(status)) => panic!("the peer refused the dial: {status:?}"),
        Err(SocketError::Http { status, body }) => panic!("the peer answered {status}: {body}"),
    }
}

/// Open a free console of `SLUG` on the peer.
async fn console(peer: &Peer) -> PeerChannel {
    let fields = serde_json::json!({ "console": 1, "repo": SLUG });
    open(peer, "console", fields).await
}

/// The next event of `channel` within the wait.
async fn next(channel: &mut PeerChannel) -> PeerEvent {
    tokio::time::timeout(WITHIN, channel.next())
        .await
        .expect("an event within the wait")
}

/// The terminal text a console channel showed, read until it holds `needle`.
/// A cursor position request is answered, as the page's terminal answers it.
struct Screen {
    text: Vec<u8>,
}

impl Screen {
    fn shown(&self) -> String {
        String::from_utf8_lossy(&self.text)
            .replace("\r\n", "")
            .replace('\n', "")
    }

    async fn until(&mut self, channel: &mut PeerChannel, needle: &str) {
        let read = async {
            while !self.shown().contains(needle) {
                let event = next(channel).await;
                self.take(channel, event).await;
            }
        };
        if tokio::time::timeout(WITHIN, read).await.is_err() {
            panic!("{needle:?} on the console: {}", self.shown());
        }
    }

    async fn take(&mut self, channel: &mut PeerChannel, event: PeerEvent) {
        let bytes = match event {
            PeerEvent::Frame(bytes) => bytes,
            other @ (PeerEvent::Closed { .. } | PeerEvent::Gone) => {
                panic!("the channel ended: {other:?}; it showed {}", self.shown())
            }
        };
        let Ok(Frame::Terminal { session, data }) = protocol::decode(&bytes) else {
            return;
        };
        self.text.extend_from_slice(&data);
        if data
            .windows(CURSOR_POSITION_REQUEST.len())
            .any(|w| w == CURSOR_POSITION_REQUEST)
        {
            let reply = Frame::Terminal {
                session,
                data: CURSOR_POSITION_REPLY.to_vec(),
            };
            channel
                .send(&protocol::encode(&reply))
                .await
                .expect("answer the cursor position request");
        }
    }
}

/// The `session-open` of a new console, as the first frame of its channel,
/// then its terminal read until the child is ready.
async fn ready(channel: &mut PeerChannel) -> (Command, Screen) {
    let opened = match next(channel).await {
        PeerEvent::Frame(bytes) => protocol::decode(&bytes).expect("a well-formed inner frame"),
        other @ (PeerEvent::Closed { .. } | PeerEvent::Gone) => {
            panic!("expected session-open, got {other:?}")
        }
    };
    let Frame::Command(opened) = opened else {
        panic!("expected session-open, got {opened:?}");
    };
    assert_eq!(opened.verb, "session-open", "{opened:?}");
    let mut screen = Screen { text: Vec::new() };
    screen.until(channel, "READY").await;
    (opened, screen)
}

#[tokio::test]
async fn a_peer_channel_reads_the_session_open_of_a_peer_console() {
    let peer = peer().await;
    let mut channel = console(&peer).await;
    let event = next(&mut channel).await;
    let PeerEvent::Frame(bytes) = event else {
        panic!("expected a frame, got {event:?}");
    };
    let frame = protocol::decode(&bytes).expect("one whole inner frame");
    let Frame::Command(opened) = frame else {
        panic!("expected session-open, got {frame:?}");
    };
    assert_eq!(opened.verb, "session-open", "{opened:?}");
    assert_eq!(opened.payload["daemon_id"], PEER_ID, "{opened:?}");
    channel.close().await;
}

#[tokio::test]
async fn a_peer_channel_open_the_peer_refuses_reads_as_refused() {
    let peer = peer().await;
    let mut channel = open(&peer, "no-such-kind", serde_json::json!({})).await;
    assert_eq!(
        next(&mut channel).await,
        PeerEvent::Closed {
            reason: "refused".to_string()
        }
    );
}

#[tokio::test]
async fn a_peer_channel_surfaces_nothing_of_channel_zero() {
    let peer = peer().await;
    let mut channel = open(&peer, "changes", serde_json::json!({})).await;
    // The peer sends its heartbeat on channel 0 at once, and every few
    // seconds after.
    let read = tokio::time::timeout(Duration::from_secs(3), channel.next()).await;
    assert!(read.is_err(), "no event in 3 s, got {read:?}");
}

#[tokio::test]
async fn a_peer_channel_answers_the_peer_pings() {
    fast_pings();
    let peer = peer().await;
    let mut channel = console(&peer).await;
    let (opened, mut screen) = ready(&mut channel).await;
    let quiet_until = tokio::time::Instant::now() + Duration::from_secs(3);
    while let Ok(event) = tokio::time::timeout_at(quiet_until, channel.next()).await {
        screen.take(&mut channel, event).await;
    }
    let session = opened.payload["session"].as_u64().expect("a session id");
    let line = Frame::Terminal {
        session,
        data: b"still-here\r".to_vec(),
    };
    channel
        .send(&protocol::encode(&line))
        .await
        .expect("type on the console");
    screen.until(&mut channel, "GOT:still-here").await;
    channel.close().await;
}

#[tokio::test]
async fn a_peer_channel_sends_a_frame_past_one_piece_whole() {
    let peer = peer().await;
    let mut channel = console(&peer).await;
    let (_opened, mut screen) = ready(&mut channel).await;
    let resize = protocol::encode(&Frame::Command(Command {
        id: 0,
        verb: "resize".to_string(),
        payload: serde_json::json!({ "rows": 40, "cols": 100, "pad": "x".repeat(PIECE_CAP) }),
    }));
    assert!(resize.len() > PIECE_CAP, "{} bytes", resize.len());
    channel.send(&resize).await.expect("send the resize");
    screen.until(&mut channel, "SIZE 100x40").await;
    channel.close().await;
}

#[tokio::test]
async fn a_peer_channel_to_a_protocol_4_peer_is_a_version_mismatch() {
    let (listener, port) = bind().await;
    let hello = serde_json::json!({
        "daemon_id": PEER_ID,
        "environment": ENVIRONMENT,
        "protocol_version": 4,
    });
    let app = axum::Router::new().route(
        "/api/peer/hello",
        axum::routing::get(move || async move { axum::Json(hello) }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve the stand-in peer");
    });
    let opened = open_channel(&descriptor(port), ME, "console", serde_json::json!({})).await;
    match opened {
        Err(SocketError::Peer(status)) => assert_eq!(
            status,
            PeerStatus::VersionMismatch {
                theirs: 4,
                ours: PEER_PROTOCOL_VERSION
            }
        ),
        Err(SocketError::Http { status, body }) => {
            panic!("expected a mismatch, got {status}: {body}")
        }
        Ok(_) => panic!("expected a mismatch, the channel opened"),
    }
}
