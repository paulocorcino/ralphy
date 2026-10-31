//! The `console` channel of a console this daemon hosts: a launch, a resume
//! by byte count, input, the audit record, the traffic line and the holder
//! rule, each over a real page socket.

#![expect(
    clippy::disallowed_methods,
    reason = "nextest runs each test in its own process, so a test may change its environment"
)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};

use super::support::{close, connect, next_closed, next_inner, open, send_on, serve_with, Ws};

const WITHIN: Duration = Duration::from_secs(10);

/// Point the session launcher at `session_test_child` instead of a real agent
/// or shell. Copied from `tests/sessions/main.rs`: the env var is set exactly
/// once, because a repeated `set_var` across threads is a data race.
fn point_launcher_at_test_child() {
    static OVERRIDE: std::sync::Once = std::sync::Once::new();
    OVERRIDE.call_once(|| {
        std::env::set_var(
            "RALPHY_DAEMON_AGENT_OVERRIDE",
            env!("CARGO_BIN_EXE_session_test_child"),
        );
        std::env::set_var(
            "RALPHY_EXE_OVERRIDE",
            env!("CARGO_BIN_EXE_command_test_child"),
        );
    });
}

/// A daemon whose launcher runs the test child, and the path of its store.
async fn daemon() -> (super::support::Daemon, std::path::PathBuf) {
    point_launcher_at_test_child();
    let mut store = None;
    let daemon = serve_with(|dir| store = Some(dir.to_path_buf())).await;
    (daemon, store.expect("the setup ran"))
}

/// The next inner frame of `channel`, decoded.
async fn next_frame(ws: &mut Ws, channel: u32) -> Frame {
    let inner = next_inner(ws, channel, WITHIN)
        .await
        .unwrap_or_else(|| panic!("a frame on channel {channel}"));
    protocol::decode(&inner).expect("a well-formed inner frame")
}

/// The next frame of `channel`, which must be the command `verb`.
async fn next_command(ws: &mut Ws, channel: u32, verb: &str) -> Command {
    match next_frame(ws, channel).await {
        Frame::Command(command) if command.verb == verb => command,
        other @ (Frame::Terminal { .. }
        | Frame::Command(_)
        | Frame::Presence(_)
        | Frame::Channel { .. }) => {
            panic!("expected {verb} on channel {channel}, got {other:?}")
        }
    }
}

/// What a console channel showed: the byte count of its first byte, and
/// every terminal byte since, replay included.
struct Shown {
    offset: u64,
    bytes: Vec<u8>,
}

impl Shown {
    fn count(&self) -> u64 {
        self.offset + self.bytes.len() as u64
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

/// Read `session-open` and `session-size` on `channel`. Returns the open.
async fn opened(ws: &mut Ws, channel: u32) -> Command {
    let open = next_command(ws, channel, "session-open").await;
    next_command(ws, channel, "session-size").await;
    open
}

/// Read terminal frames of `channel` into `shown` until its bytes hold
/// `needle`. Commands on the channel are skipped. A cursor position request
/// is answered, as the terminal of the page answers it: ConPTY waits for the
/// answer before the child's output.
async fn read_until(ws: &mut Ws, channel: u32, shown: &mut Shown, needle: &str) {
    while !shown.text().contains(needle) {
        let Frame::Terminal { session, data } = next_frame(ws, channel).await else {
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
            send_on(ws, channel, &protocol::encode(&reply)).await;
        }
        shown.bytes.extend_from_slice(&data);
    }
}

/// Launch a free console on `channel` and read it until the child is ready.
/// Returns the session id and what the channel showed.
async fn launch(ws: &mut Ws, channel: u32, fields: serde_json::Value) -> (u64, Shown) {
    let mut payload = serde_json::json!({ "console": 1 });
    if let (Some(all), serde_json::Value::Object(extra)) = (payload.as_object_mut(), fields) {
        all.extend(extra);
    }
    open(ws, channel, "console", payload).await;
    let open = opened(ws, channel).await;
    let id = open.payload["session"].as_u64().expect("a session id");
    let mut shown = Shown {
        offset: open.payload["offset"].as_u64().expect("a byte count"),
        bytes: Vec::new(),
    };
    read_until(ws, channel, &mut shown, "READY").await;
    (id, shown)
}

/// Type one line into the console of `channel`.
async fn type_line(ws: &mut Ws, channel: u32, id: u64, line: &str) {
    let frame = Frame::Terminal {
        session: id,
        data: format!("{line}\r").into_bytes(),
    };
    send_on(ws, channel, &protocol::encode(&frame)).await;
}

/// The replay frame that follows `session-open` and `session-size`.
async fn replay(ws: &mut Ws, channel: u32) -> Vec<u8> {
    match next_frame(ws, channel).await {
        Frame::Terminal { data, .. } => data,
        other @ (Frame::Command(_) | Frame::Presence(_) | Frame::Channel { .. }) => {
            panic!("expected the replay on channel {channel}, got {other:?}")
        }
    }
}

#[tokio::test]
async fn a_console_channel_launches_and_streams() {
    let (daemon, _store) = daemon().await;
    let mut ws = connect(&daemon).await;

    open(&mut ws, 1, "console", serde_json::json!({ "console": 1 })).await;
    let open = next_command(&mut ws, 1, "session-open").await;
    assert!(open.payload["session"].is_u64(), "{open:?}");
    next_command(&mut ws, 1, "session-size").await;
    let mut shown = Shown {
        offset: 0,
        bytes: Vec::new(),
    };
    read_until(&mut ws, 1, &mut shown, "READY").await;
}

#[tokio::test]
async fn a_console_channel_resumes_from_its_byte_count() {
    let (daemon, _store) = daemon().await;
    let mut ws = connect(&daemon).await;
    let (id, mut shown) = launch(&mut ws, 1, serde_json::json!({})).await;
    let since = shown.count();
    type_line(&mut ws, 1, id, "after-since").await;
    read_until(&mut ws, 1, &mut shown, "GOT:after-since").await;
    let after =
        shown.bytes[usize::try_from(since - shown.offset).expect("a small count")..].to_vec();
    close(&mut ws, 1).await;

    open(
        &mut ws,
        2,
        "console",
        serde_json::json!({ "id": id, "watch": 1, "since": since }),
    )
    .await;
    let open = opened(&mut ws, 2).await;
    assert_eq!(open.payload["resume"], true, "{open:?}");
    let replay = replay(&mut ws, 2).await;
    assert!(
        replay.starts_with(&after),
        "the replay is the bytes after `since`: {:?}",
        String::from_utf8_lossy(&replay)
    );
    assert!(
        !String::from_utf8_lossy(&replay).contains("READY"),
        "no byte before `since` comes again"
    );
}

#[tokio::test]
async fn a_console_channel_without_the_bytes_gets_the_snapshot() {
    let (daemon, _store) = daemon().await;
    let mut ws = connect(&daemon).await;
    let (id, shown) = launch(&mut ws, 1, serde_json::json!({})).await;

    // A count past every byte the session wrote is not in the ring.
    let since = shown.count() + 1_000_000;
    open(
        &mut ws,
        2,
        "console",
        serde_json::json!({ "id": id, "watch": 1, "since": since }),
    )
    .await;
    let open = opened(&mut ws, 2).await;
    assert_eq!(open.payload["resume"], false, "{open:?}");
    let replay = replay(&mut ws, 2).await;
    assert!(
        String::from_utf8_lossy(&replay).contains("READY"),
        "the snapshot holds the screen: {:?}",
        String::from_utf8_lossy(&replay)
    );
}

#[tokio::test]
async fn console_input_on_a_channel_reaches_the_session() {
    let (daemon, _store) = daemon().await;
    let mut ws = connect(&daemon).await;
    let (id, mut shown) = launch(&mut ws, 1, serde_json::json!({})).await;

    type_line(&mut ws, 1, id, "hello-channel").await;
    read_until(&mut ws, 1, &mut shown, "GOT:hello-channel").await;
}

/// The console lines of the audit log in `store`.
fn console_audit(store: &std::path::Path) -> Vec<serde_json::Value> {
    let log = std::fs::read_to_string(store.join("daemon-audit.jsonl")).unwrap_or_default();
    log.lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("a JSON line"))
        .filter(|line| {
            line["event"]
                .as_str()
                .is_some_and(|event| event.starts_with("console_"))
        })
        .collect()
}

#[tokio::test]
async fn a_console_launch_on_a_channel_is_recorded() {
    let (daemon, store) = daemon().await;
    let mut ws = connect(&daemon).await;
    let (id, _shown) = launch(&mut ws, 1, serde_json::json!({ "holder": "tab-a" })).await;

    // The same tab reattaches on a second channel: it reclaims its slot.
    open(
        &mut ws,
        2,
        "console",
        serde_json::json!({ "id": id, "holder": "tab-a" }),
    )
    .await;
    opened(&mut ws, 2).await;

    let lines = console_audit(&store);
    let events: Vec<&str> = lines
        .iter()
        .map(|line| line["event"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(events, ["console_launch"], "{lines:#?}");
    assert_eq!(lines[0]["session"], id, "{lines:#?}");
    assert_eq!(lines[0]["holder"], "tab-a", "{lines:#?}");
}

/// The log lines written while a test runs.
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
    type Writer = Written;
    fn make_writer(&'a self) -> Written {
        self.clone()
    }
}

impl Written {
    fn traffic_lines(&self) -> usize {
        let bytes = self.0.lock().expect("a test lock").clone();
        String::from_utf8_lossy(&bytes)
            .lines()
            .filter(|line| line.contains("console socket traffic"))
            .count()
    }

    /// Wait until `count` traffic lines are written, then a little longer,
    /// and return how many there are.
    async fn traffic_lines_after(&self, count: usize) -> usize {
        let deadline = tokio::time::Instant::now() + WITHIN;
        while self.traffic_lines() < count && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        self.traffic_lines()
    }
}

// The runtime has one thread, so the subscriber set for this thread sees
// every task of the daemon.
#[tokio::test(flavor = "current_thread")]
async fn each_console_channel_logs_one_traffic_line() {
    let written = Written::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(written.clone())
        .with_ansi(false)
        .finish();
    let _log = tracing::subscriber::set_default(subscriber);

    let (daemon, _store) = daemon().await;
    let mut ws = connect(&daemon).await;
    let (id, _shown) = launch(&mut ws, 1, serde_json::json!({})).await;
    open(
        &mut ws,
        2,
        "console",
        serde_json::json!({ "id": id, "watch": 1 }),
    )
    .await;
    opened(&mut ws, 2).await;
    assert_eq!(written.traffic_lines(), 0, "no line while both are open");

    close(&mut ws, 1).await;
    assert_eq!(
        written.traffic_lines_after(1).await,
        1,
        "one line per close"
    );
    close(&mut ws, 2).await;
    assert_eq!(
        written.traffic_lines_after(2).await,
        2,
        "one line per close"
    );
}

#[tokio::test]
async fn a_console_open_with_a_bad_holder_is_refused() {
    let (daemon, _store) = daemon().await;
    let mut ws = connect(&daemon).await;

    // Control: a holder of 64 characters reclaims the slot it holds.
    let good = "h".repeat(64);
    let (id, _shown) = launch(&mut ws, 1, serde_json::json!({ "holder": good })).await;
    open(
        &mut ws,
        2,
        "console",
        serde_json::json!({ "id": id, "holder": good }),
    )
    .await;
    opened(&mut ws, 2).await;

    // A holder of 65 characters is no holder, as in the open of
    // a console channel: it holds no slot, so it cannot reclaim one.
    let bad = "h".repeat(65);
    let (id, _shown) = launch(&mut ws, 3, serde_json::json!({ "holder": bad })).await;
    open(
        &mut ws,
        4,
        "console",
        serde_json::json!({ "id": id, "holder": bad }),
    )
    .await;
    // Channel 1 lost its slot to channel 2, and its own close may come first.
    let closed = loop {
        let closed = next_closed(&mut ws, WITHIN)
            .await
            .expect("a channel.closed");
        if closed.channel != 1 {
            break closed;
        }
    };
    assert_eq!((closed.channel, closed.reason.as_str()), (4, "refused"));
}

#[tokio::test]
async fn a_console_open_past_the_report_cap_is_served() {
    let (daemon, _store) = daemon().await;
    let mut ws = connect(&daemon).await;

    // A key the open does not read pads the open past 512 bytes.
    let pad = "x".repeat(1000);
    launch(&mut ws, 1, serde_json::json!({ "pad": pad })).await;
}
