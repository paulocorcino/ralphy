//! The `console` channel of a console a peer hosts, on two real daemons: the
//! local daemon relays the peer's session, cuts its frames into pieces, and
//! answers the peer's pings itself.

#![expect(
    clippy::disallowed_methods,
    reason = "nextest runs each test in its own process, so a test may change its environment"
)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::auth::{AuthPolicy, AuthState};
use ralphy_daemon::epoch::SessionEpoch;
use ralphy_daemon::identity::Identity;
use ralphy_daemon::peer::{self, PeerDescriptor, PEER_PROTOCOL_VERSION};
use ralphy_daemon::protocol::{self, Command, Frame, Joiner};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio_tungstenite::tungstenite::Message;

use super::support::{close, open, send_on, Ws};

const LOCAL_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
const PEER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB1";
const SLUG: &str = "owner/shared";
const ENVIRONMENT: &str = "Linux: peer";
const TOKEN: &str = "peer-token";
const WITHIN: Duration = Duration::from_secs(15);

/// The largest piece a channel frame carries, as the check states it.
const PIECE_CAP: usize = 16384;

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

/// Ping every 300 ms, so a socket that answers no ping ends within a second.
/// Set before the first daemon starts: the period is read once per process.
fn fast_pings() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| std::env::set_var("RALPHY_DAEMON_WS_PING_MS", "300"));
}

struct Daemon {
    port: u16,
    _shutdown: tokio::sync::watch::Sender<bool>,
}

/// A local daemon that knows one peer, and the peer, each with its own store
/// and its own checkout of `SLUG`.
struct Fleet {
    local: Daemon,
    _peer: Daemon,
    local_repo: tempfile::TempDir,
    peer_repo: tempfile::TempDir,
    _stores: [tempfile::TempDir; 2],
}

fn identity(id: &str, name: &str) -> Identity {
    Identity {
        id: id.parse().expect("the test id parses"),
        name: name.to_string(),
        avatar: "🐙".to_string(),
    }
}

fn descriptor(id: &str, port: u16) -> PeerDescriptor {
    PeerDescriptor {
        daemon_id: id.to_string(),
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

/// A store whose registry holds `SLUG` at a new checkout.
fn store() -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
    let store = tempfile::tempdir().expect("create a temporary store");
    let repo = tempfile::tempdir().expect("create a temporary checkout");
    let path = store.path().join("repos.toml");
    let mut registry = registry::RegistryStore::default();
    registry.upsert(SLUG, &repo.path().to_string_lossy());
    registry::save_to(&registry, &path).expect("save the registry");
    (store, repo, path)
}

async fn serve(identity: Identity, registry_path: PathBuf, auth: Arc<AuthState>) -> Daemon {
    point_launcher_at_test_child();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let port = listener
        .local_addr()
        .expect("read the bound address")
        .port();
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
    Daemon {
        port,
        _shutdown: shutdown,
    }
}

async fn fleet() -> Fleet {
    let (peer_store, peer_repo, peer_registry) = store();
    let peer = serve(
        identity(PEER_ID, "peer"),
        peer_registry,
        AuthState::fixed(
            AuthPolicy::Bearer(TOKEN.to_string()),
            SessionEpoch::in_memory_detached(),
        ),
    )
    .await;
    let (local_store, local_repo, local_registry) = store();
    peer::write_descriptor(local_store.path(), &descriptor(PEER_ID, peer.port))
        .expect("write the peer's descriptor");
    let local = serve(
        identity(LOCAL_ID, "local"),
        local_registry,
        AuthState::localhost(),
    )
    .await;
    peer::write_descriptor(peer_store.path(), &descriptor(LOCAL_ID, local.port))
        .expect("write the local daemon's descriptor");
    Fleet {
        local,
        _peer: peer,
        local_repo,
        peer_repo,
        _stores: [peer_store, local_store],
    }
}

/// The repository reference of `SLUG` on the peer.
fn peer_repo() -> String {
    format!("{PEER_ID}/{SLUG}")
}

/// A page socket on the local daemon's `/ws`, read frame by frame.
struct Page {
    ws: Ws,
    joiners: HashMap<u32, Joiner>,
    pieces: HashMap<u32, Vec<usize>>,
    /// The pings with a payload that reached the page. The page socket's own
    /// pings carry none; a peer's pings carry their number.
    peer_pings: usize,
    /// The terminal bytes each channel showed, every frame read so far.
    text: HashMap<u32, Vec<u8>>,
}

/// One inner frame the page read, with the sizes of its pieces.
struct Inner {
    channel: u32,
    frame: Frame,
    pieces: Vec<usize>,
}

impl Page {
    async fn connect(fleet: &Fleet) -> Page {
        let url = format!("ws://127.0.0.1:{}/ws", fleet.local.port);
        let (ws, _resp) = tokio_tungstenite::connect_async(url)
            .await
            .expect("connect to /ws");
        Page {
            ws,
            joiners: HashMap::new(),
            pieces: HashMap::new(),
            peer_pings: 0,
            text: HashMap::new(),
        }
    }

    /// The next whole inner frame of any channel; bare frames are channel 0.
    async fn next(&mut self) -> Inner {
        tokio::time::timeout(WITHIN, self.read())
            .await
            .expect("a frame within the wait")
    }

    async fn read(&mut self) -> Inner {
        loop {
            let message = self
                .ws
                .next()
                .await
                .expect("the page socket stays open")
                .expect("the page socket reads");
            let bytes = match message {
                Message::Binary(bytes) => bytes,
                Message::Ping(payload) => {
                    self.peer_pings += usize::from(!payload.is_empty());
                    continue;
                }
                Message::Pong(_) => continue,
                other @ (Message::Text(_) | Message::Close(_) | Message::Frame(_)) => {
                    panic!("an unexpected message on the page socket: {other:?}")
                }
            };
            match protocol::decode(&bytes).expect("a well-formed frame") {
                Frame::Channel {
                    channel,
                    more,
                    piece,
                } => {
                    self.pieces.entry(channel).or_default().push(piece.len());
                    let joined = self
                        .joiners
                        .entry(channel)
                        .or_default()
                        .push(more, &piece)
                        .expect("a frame within the cap");
                    if let Some(inner) = joined {
                        let frame = protocol::decode(&inner).expect("a well-formed inner frame");
                        let pieces = self.pieces.remove(&channel).unwrap_or_default();
                        return self.seen(channel, frame, pieces);
                    }
                }
                frame @ (Frame::Terminal { .. } | Frame::Command(_) | Frame::Presence(_)) => {
                    return self.seen(0, frame, Vec::new())
                }
            }
        }
    }

    fn seen(&mut self, channel: u32, frame: Frame, pieces: Vec<usize>) -> Inner {
        match &frame {
            Frame::Terminal { data, .. } => self
                .text
                .entry(channel)
                .or_default()
                .extend_from_slice(data),
            Frame::Command(command) if channel == 0 && command.verb == "channel.closed" => {
                panic!("a channel ended: {command:?}")
            }
            Frame::Command(_) | Frame::Presence(_) | Frame::Channel { .. } => {}
        }
        Inner {
            channel,
            frame,
            pieces,
        }
    }

    /// The text `channel` showed, line breaks removed: a long line wraps.
    fn shown(&self, channel: u32) -> String {
        self.text
            .get(&channel)
            .map(|text| {
                String::from_utf8_lossy(text)
                    .replace("\r\n", "")
                    .replace('\n', "")
            })
            .unwrap_or_default()
    }

    /// The next frame of `channel`, which must be the command `verb`. Frames
    /// of other channels are read and kept in `text`.
    async fn command(&mut self, channel: u32, verb: &str) -> Command {
        tokio::time::timeout(WITHIN, self.read_command(channel, verb))
            .await
            .unwrap_or_else(|_| panic!("{verb} on channel {channel} within the wait"))
    }

    async fn read_command(&mut self, channel: u32, verb: &str) -> Command {
        loop {
            let inner = self.read().await;
            if inner.channel != channel {
                continue;
            }
            match inner.frame {
                Frame::Command(command) if command.verb == verb => return command,
                other @ (Frame::Terminal { .. }
                | Frame::Command(_)
                | Frame::Presence(_)
                | Frame::Channel { .. }) => {
                    panic!("expected {verb} on channel {channel}, got {other:?}")
                }
            }
        }
    }

    /// Read until the text of `channel` holds `needle`.
    async fn until(&mut self, channel: u32, needle: &str) {
        self.until_count(channel, needle, 1).await;
    }

    /// Read until the text of `channel` holds `needle` `count` times. A
    /// cursor position request is answered, as the page's terminal answers it.
    async fn until_count(&mut self, channel: u32, needle: &str, count: usize) {
        let read = self.read_until(channel, needle, count);
        if tokio::time::timeout(WITHIN, read).await.is_err() {
            panic!("{needle:?} on channel {channel}: {}", self.shown(channel));
        }
    }

    async fn read_until(&mut self, channel: u32, needle: &str, count: usize) {
        while self.shown(channel).matches(needle).count() < count {
            let inner = self.read().await;
            let Frame::Terminal { session, data } = inner.frame else {
                continue;
            };
            if data
                .windows(CURSOR_POSITION_REQUEST.len())
                .any(|w| w == CURSOR_POSITION_REQUEST)
            {
                let reply = Frame::Terminal {
                    session,
                    data: CURSOR_POSITION_REPLY.to_vec(),
                };
                send_on(&mut self.ws, inner.channel, &protocol::encode(&reply)).await;
            }
        }
    }

    /// Launch a free console of `repo` on `channel` and read it until the
    /// child is ready. Returns its `session-open`.
    async fn launch(&mut self, channel: u32, repo: &str) -> Command {
        let fields = serde_json::json!({ "console": 1, "repo": repo });
        open(&mut self.ws, channel, "console", fields).await;
        let opened = self.command(channel, "session-open").await;
        self.command(channel, "session-size").await;
        self.until(channel, "READY").await;
        opened
    }

    /// Send one inner frame on `channel`.
    async fn send(&mut self, channel: u32, frame: &Frame) {
        send_on(&mut self.ws, channel, &protocol::encode(frame)).await;
    }

    /// Type one line into session `id` on `channel`.
    async fn type_line(&mut self, channel: u32, id: u64, line: &str) {
        let frame = Frame::Terminal {
            session: id,
            data: format!("{line}\r").into_bytes(),
        };
        self.send(channel, &frame).await;
    }
}

fn session_of(opened: &Command) -> u64 {
    opened.payload["session"].as_u64().expect("a session id")
}

/// Whether the child's `CWD:` line names `dir`, as given or as the OS
/// resolved it (`/private/var/…` on macOS; no `\\?\` prefix on Windows).
fn names_cwd(text: &str, dir: &Path) -> bool {
    let raw = format!("CWD:{}", dir.display());
    let canonical = dir
        .canonicalize()
        .map(|c| c.display().to_string())
        .unwrap_or_default();
    let canonical = format!("CWD:{}", canonical.trim_start_matches("\\\\?\\"));
    text.contains(&raw) || text.contains(&canonical)
}

#[tokio::test]
async fn a_peer_console_on_a_channel_streams_and_resizes() {
    let fleet = fleet().await;
    let mut page = Page::connect(&fleet).await;
    let opened = page.launch(1, &peer_repo()).await;
    assert_eq!(opened.payload["daemon_id"], PEER_ID, "{opened:?}");
    assert!(
        names_cwd(&page.shown(1), fleet.peer_repo.path()),
        "the console runs in the peer's checkout: {}",
        page.shown(1)
    );
    let id = session_of(&opened);

    page.type_line(1, id, "peer-input").await;
    page.until(1, "GOT:peer-input").await;

    let resize = Frame::Command(Command {
        id: 0,
        verb: "resize".to_string(),
        payload: serde_json::json!({ "rows": 40, "cols": 100 }),
    });
    page.send(1, &resize).await;
    page.until(1, "SIZE 100x40").await;
}

#[tokio::test]
async fn a_peer_replay_is_cut_into_pieces() {
    let fleet = fleet().await;
    let mut page = Page::connect(&fleet).await;
    let opened = page.launch(1, &peer_repo()).await;
    let id = session_of(&opened);
    let since = opened.payload["offset"].as_u64().expect("a byte count");
    // A terminal that draws by frames (ConPTY) writes only part of a fast
    // flood, so the child floods until the session wrote 100 KiB.
    let mut floods = 0;
    while page.text.get(&1).map_or(0, Vec::len) < 100 * 1024 {
        floods += 1;
        page.type_line(1, id, "flood 100").await;
        page.until_count(1, "FLOOD-DONE", floods).await;
    }
    page.type_line(1, id, "after-flood").await;
    page.until(1, "GOT:after-flood").await;
    let live = page.text.remove(&1).unwrap_or_default();
    close(&mut page.ws, 1).await;

    let fields = serde_json::json!({ "id": id, "repo": peer_repo(), "watch": 1, "since": since });
    open(&mut page.ws, 2, "console", fields).await;
    page.command(2, "session-open").await;
    page.command(2, "session-size").await;
    let replay = loop {
        let inner = page.next().await;
        if inner.channel == 2 {
            break inner;
        }
    };
    let Frame::Terminal { data, .. } = replay.frame else {
        panic!("expected the replay on channel 2, got {:?}", replay.frame);
    };
    assert!(
        replay.pieces.len() > 1,
        "a replay of 100 KiB comes in pieces: {:?}",
        replay.pieces
    );
    assert!(
        replay.pieces.iter().all(|len| *len <= PIECE_CAP),
        "every piece is at most {PIECE_CAP} bytes: {:?}",
        replay.pieces
    );
    assert!(
        data.starts_with(&live),
        "the pieces join to the bytes the session wrote: {} joined, {} live",
        data.len(),
        live.len()
    );
}

#[tokio::test]
async fn a_peer_ping_is_answered_by_the_local_daemon() {
    fast_pings();
    let fleet = fleet().await;
    let mut page = Page::connect(&fleet).await;
    let opened = page.launch(1, &peer_repo()).await;
    let id = session_of(&opened);

    // The page reads nothing for many of the peer's silent windows, so it
    // answers no ping. It sends pings of its own, which its daemon answers,
    // so its own socket stays heard.
    let quiet_until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < quiet_until {
        page.ws
            .send(Message::Ping(Vec::new().into()))
            .await
            .expect("send a ping on the page socket");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    page.type_line(1, id, "still-here").await;
    page.until(1, "GOT:still-here").await;
    assert_eq!(page.peer_pings, 0, "no peer ping reaches the page");
}

#[tokio::test]
async fn two_daemons_with_the_same_session_id_are_two_channels() {
    let fleet = fleet().await;
    let mut page = Page::connect(&fleet).await;
    let local = page.launch(1, SLUG).await;
    let remote = page.launch(2, &peer_repo()).await;
    assert_eq!(local.payload["daemon_id"], LOCAL_ID, "{local:?}");
    assert_eq!(remote.payload["daemon_id"], PEER_ID, "{remote:?}");
    let id = session_of(&local);
    assert_eq!(session_of(&remote), id, "each daemon gave its first id");
    assert!(names_cwd(&page.shown(1), fleet.local_repo.path()));
    assert!(names_cwd(&page.shown(2), fleet.peer_repo.path()));

    page.type_line(1, id, "on-local").await;
    page.type_line(2, id, "on-peer").await;
    page.until(1, "GOT:on-local").await;
    page.until(2, "GOT:on-peer").await;
    assert!(!page.shown(1).contains("GOT:on-peer"), "{}", page.shown(1));
    assert!(!page.shown(2).contains("GOT:on-local"), "{}", page.shown(2));
}
