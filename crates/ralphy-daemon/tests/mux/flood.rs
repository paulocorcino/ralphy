//! A channel that sends a lot cannot hold back the others: one console runs
//! a child that prints without end on a channel, and a second console on the
//! same real page socket still gets its echo. Two large replies queued behind
//! the flood leave one piece at a time, with at most one piece of the flood
//! between two pieces of them.

#![expect(
    clippy::disallowed_methods,
    reason = "nextest runs each test in its own process, so a test may change its environment"
)]

use std::time::Duration;

use ralphy_daemon::protocol::{self, Frame, Joiner};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio_tungstenite::tungstenite::Message;

use futures_util::StreamExt;

use super::support::{connect, next_inner, open, send_on, serve, Written, Ws};

const WITHIN: Duration = Duration::from_secs(10);
/// How long the test waits for a state of the daemon that it reads in the log.
const LOGGED_WITHIN: Duration = Duration::from_secs(5);
/// The log line of the writer when the socket is full.
const WAITS: &str = "socket writer waits: the socket is full";
/// The log line of a console bridge that queued a replay.
const REPLAY_QUEUED: &str = "console replay queued";

/// The channel of the console that floods.
const FLOODING: u32 = 1;
/// The channel of the console that types and reads.
const QUIET: u32 = 2;
/// The two channels that read the quiet console again, both at once.
const REPLAYS: [u32; 2] = [3, 4];

/// Point the session launcher at `session_test_child` instead of a real agent
/// or shell, as `console.rs` does: the variable is set once per process.
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

/// Read the terminal frames of `channel` until their text holds `needle`.
/// Returns the session id. A cursor position request is answered, as the
/// terminal of the page answers it: `ConPTY` waits for the answer before the
/// child's output.
async fn read_until(ws: &mut Ws, channel: u32, needle: &str) -> u64 {
    let mut shown = String::new();
    loop {
        let inner = next_inner(ws, channel, WITHIN)
            .await
            .ok_or_else(|| format!("`{needle}` on channel {channel}, after {shown:?}"))
            .expect("this test needs the value");
        let Ok(Frame::Terminal { session, data }) = protocol::decode(&inner) else {
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
        shown.push_str(&String::from_utf8_lossy(&data));
        if shown.contains(needle) {
            return session;
        }
    }
}

/// Launch a free console on `channel` and read it until the child is ready.
/// Returns the session id.
async fn launch(ws: &mut Ws, channel: u32) -> u64 {
    open(ws, channel, "console", serde_json::json!({ "console": 1 })).await;
    read_until(ws, channel, "READY").await
}

/// Type one line into the console of `channel`.
async fn type_line(ws: &mut Ws, channel: u32, session: u64, line: &str) {
    let frame = Frame::Terminal {
        session,
        data: format!("{line}\r").into_bytes(),
    };
    send_on(ws, channel, &protocol::encode(&frame)).await;
}

/// Read the socket until each of `REPLAYS` has joined a terminal frame that
/// holds `FLOOD-DONE`. Returns every piece the socket carried meanwhile, as
/// its channel and its MORE flag, in arrival order.
async fn pieces_until_replays(ws: &mut Ws) -> Vec<(u32, bool)> {
    let mut seen = Vec::new();
    let mut joiners = [Joiner::default(), Joiner::default()];
    let mut whole = [false, false];
    let read = tokio::time::timeout(WITHIN, async {
        while !whole.iter().all(|&done| done) {
            let bytes = match ws.next().await {
                Some(Ok(Message::Binary(bytes))) => bytes,
                Some(Ok(_)) => continue,
                None | Some(Err(_)) => return false,
            };
            let Ok(Frame::Channel {
                channel,
                more,
                piece,
            }) = protocol::decode(&bytes)
            else {
                continue;
            };
            seen.push((channel, more));
            let Some(at) = REPLAYS.iter().position(|&replay| replay == channel) else {
                continue;
            };
            let joined = joiners[at]
                .push(more, &piece)
                .expect("a frame within the cap");
            if let Some(Ok(Frame::Terminal { data, .. })) =
                joined.map(|inner| protocol::decode(&inner))
            {
                whole[at] |= String::from_utf8_lossy(&data).contains("FLOOD-DONE");
            }
        }
        true
    })
    .await;
    assert!(
        read.is_ok(),
        "the replays did not arrive within {WITHIN:?}: a flooding channel holds them back \
         ({} pieces seen, the last {:?})",
        seen.len(),
        seen.last()
    );
    assert_eq!(
        read.ok(),
        Some(true),
        "the socket ended before the replays were whole"
    );
    seen
}

/// The most pieces of `FLOODING` between two pieces of the replays.
fn longest_flood_run(pieces: &[(u32, bool)]) -> usize {
    let mut run = 0;
    let mut longest = 0;
    for &(channel, _more) in pieces {
        if REPLAYS.contains(&channel) {
            longest = longest.max(run);
            run = 0;
        } else if channel == FLOODING {
            run += 1;
        }
    }
    longest
}

// The runtime has one thread, so the subscriber set for this thread sees
// every task of the daemon.
#[tokio::test(flavor = "current_thread")]
async fn one_flooding_console_delays_another_channel_by_one_piece() {
    point_launcher_at_test_child();
    let written = Written::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(written.clone())
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .finish();
    let _log = tracing::subscriber::set_default(subscriber);
    let daemon = serve().await;
    let mut ws = connect(&daemon).await;
    // The flooding channel opens first: a writer that serves channels in
    // the order they opened would serve it to the end before the others.
    let flooding = launch(&mut ws, FLOODING).await;
    let quiet = launch(&mut ws, QUIET).await;
    assert_eq!(
        written.lines_with(WAITS),
        0,
        "the socket is full before the flood"
    );

    // About 1 GiB of lines, which no test reads to the end.
    type_line(&mut ws, FLOODING, flooding, "flood 1000000").await;

    // The quiet console gets its input and its echo while the other floods.
    type_line(&mut ws, QUIET, quiet, "hello-quiet").await;
    read_until(&mut ws, QUIET, "GOT:hello-quiet").await;

    // The replay of this much output is one inner frame, the screen with its
    // scrollback, cut into pieces.
    type_line(&mut ws, QUIET, quiet, "flood 200").await;
    read_until(&mut ws, QUIET, "FLOOD-DONE").await;

    // Nothing reads the socket from here, so the writer soon waits on the
    // full socket with the flood queued, and it waits until the test reads.
    // Both replays are asked for meanwhile and queue whole behind it.
    let waits = written.lines_with(WAITS);
    assert!(
        written.more_lines_with(WAITS, waits, LOGGED_WITHIN).await,
        "the writer did not wait on a full socket within {LOGGED_WITHIN:?}"
    );
    // A launch can queue a replay of its own (on Linux it does), so the
    // two asked for here are counted from this point.
    let replays_before = written.lines_with(REPLAY_QUEUED);
    for replay in REPLAYS {
        let fields = serde_json::json!({ "id": quiet, "watch": 1, "since": 0 });
        open(&mut ws, replay, "console", fields).await;
    }
    assert!(
        written
            .more_lines_with(REPLAY_QUEUED, replays_before + 1, LOGGED_WITHIN)
            .await,
        "both replays were not queued within {LOGGED_WITHIN:?}"
    );
    let seen = pieces_until_replays(&mut ws).await;

    // From the first piece that has more behind it, a replay is one frame
    // that was queued whole.
    let start = seen
        .iter()
        .position(|&(channel, more)| REPLAYS.contains(&channel) && more)
        .expect("a replay comes in more than one piece");
    let seen = &seen[start..];
    let replay_pieces: Vec<u32> = seen
        .iter()
        .map(|&(channel, _more)| channel)
        .filter(|channel| REPLAYS.contains(channel))
        .collect();
    let count = |replay| replay_pieces.iter().filter(|&&c| c == replay).count();
    let both = count(REPLAYS[0]).min(count(REPLAYS[1]));
    assert!(
        both >= 3,
        "a replay is a frame of several pieces, got {both}"
    );
    assert!(
        longest_flood_run(seen) <= 1,
        "{} pieces of the flood came between two pieces of the replays",
        longest_flood_run(seen)
    );
    // While both replays have pieces left, they leave in turn.
    let in_turn = replay_pieces[..2 * both - 1]
        .windows(2)
        .all(|pair| pair[0] != pair[1]);
    assert!(in_turn, "the replays left in runs: {replay_pieces:?}");
}
