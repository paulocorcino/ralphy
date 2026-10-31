//! The byte bridge of an attached `/ws/session`: replay, live stream, input,
//! resize, and the eviction/exit teardown.

use std::ops::ControlFlow;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use futures_util::StreamExt;
use tokio::sync::broadcast::error::RecvError;

// ADR-0082 D4: a console served on a channel of the page socket.
mod channel;

pub(crate) use channel::{serve_console, ConsoleEnd, PeerRelay};

use super::outbound::{
    announce_end, hand_on, open, resync, send_output, send_size, Opening, Outbox,
};
use super::traffic::{Tab, Traffic};
use super::SessionLabels;
use crate::mux::Out;
use crate::protocol::{Command, Frame};
use crate::{protocol, session};

/// How often the bridge pings its client, and how long the client may stay
/// silent before the bridge gives up on it (ADR-0051 §9 amendment 2026-09-22).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Liveness {
    pub(crate) ping_every: Duration,
    pub(crate) silent_after: Duration,
}

/// Test seam, like `RALPHY_DAEMON_AGENT_OVERRIDE`: the ping period in
/// milliseconds. A test cannot wait out the production window.
const PING_MS_ENV: &str = "RALPHY_DAEMON_WS_PING_MS";

impl Liveness {
    /// Two whole ping periods plus a quarter of slack: one lost pong on a lossy
    /// link is not a dead peer, two in a row is.
    fn from_ping(ping_every: Duration) -> Liveness {
        Liveness {
            ping_every,
            silent_after: ping_every * 9 / 4,
        }
    }

    /// 20 s pings (well under common 30–60 s proxy idle windows), so a silent
    /// client is released after 45 s.
    pub(crate) fn current() -> Liveness {
        static LIVENESS: OnceLock<Liveness> = OnceLock::new();
        *LIVENESS.get_or_init(|| {
            let ms = std::env::var(PING_MS_ENV)
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|ms| *ms > 0)
                .unwrap_or(20_000);
            Liveness::from_ping(Duration::from_millis(ms))
        })
    }

    /// Whether a client last heard from `heard` ago has gone silent.
    pub(crate) fn is_silent(&self, heard: Duration) -> bool {
        heard >= self.silent_after
    }
}

/// Whether the bridge loop goes on. `Break` carries the DELIBERATE end to
/// announce (daemon shutdown, takeover/child-exit eviction, or the broadcast
/// sender closing). `Break(None)` means the loop fell out some other way
/// (client close, network drop, write failure) and the bridge stays SILENT:
/// announcing there would tell a client its session ended when it did not, and
/// the client would park instead of recovering the flaky link (issue #334).
type Flow = ControlFlow<Option<session::EndReason>>;

/// A failed send ends the loop without an announcement: the socket is gone.
fn silent_if_err<T, E>(sent: Result<T, E>) -> Flow {
    match sent {
        Ok(_) => ControlFlow::Continue(()),
        Err(_) => ControlFlow::Break(None),
    }
}

/// Bridge one WebSocket to one daemon-owned session (the tmux model, #166).
/// FIRST announces `session-open` with the hosting identity, then the session
/// size (`session-size`), then replays the screen snapshot and loops: session
/// output (via the broadcast `rx`) → `Frame::Terminal`; a new session size →
/// `session-size`; client `Frame::Terminal` → PTY stdin; client
/// `Frame::Command{verb:"resize"}` → PTY resize. The loop breaks on client
/// close/error, a send failure, an eviction (a `takeover` reattach OR the child
/// exiting), or daemon shutdown.
///
/// TEARDOWN INVARIANT (INVERTED vs #162): on EVERY exit path the bridge drops
/// `attach` — releasing the single-writer slot — and does NOT close the session.
/// A WebSocket drop detaches; the child survives it and a later reattach resumes
/// it. A session ends only via `POST /api/sessions/close` or its child exiting,
/// never because a browser tab closed.
///
/// LIVENESS: the browser answers every ping with a pong from its network stack,
/// even in a background tab, so a client that has sent NOTHING for
/// [`Liveness::silent_after`] is gone. A send succeeding proves nothing: a tunnel
/// agent (measured: TunnelDeck for dev tunnels, 2026-09-22) keeps its leg to the
/// daemon open for minutes after the browser behind it vanished, and that leg
/// would hold the writer slot against the client's own reattach.
/// `tab` is the browser tab that opened the socket.
///
/// INPUT IS NEVER HELD BEHIND OUTPUT (ADR-0082 D5): the frames for the client
/// wait in an [`Outbox`] and go to the socket's writer when it has room, so
/// the loop keeps reading the client while a slow link takes the output. The
/// session's output is read only once the outbox is empty, so a slow client
/// still slows the reading of output, as a blocking send did.
#[expect(
    clippy::too_many_arguments,
    reason = "the socket, the attachment, the session identity and the tab, one parameter each"
)]
pub(crate) async fn session_ws(
    socket: WebSocket,
    mut attach: session::Attachment,
    id: session::SessionId,
    daemon_id: String,
    environment: String,
    labels: SessionLabels,
    tab: Tab,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    // Register the eviction waiter BEFORE the first await. Pin ONE `notified`
    // future across the whole loop (a fresh `notified()` per iteration could miss
    // an eviction firing mid-iteration), and `enable()` it up front so the waiter
    // is parked before the snapshot replay below: an eviction (a `takeover`
    // reattach or the child exiting) that fires during that replay `await` — or in
    // the `on_upgrade` scheduling gap — is delivered as a stored permit and breaks
    // the loop on the first poll, never lost. Missing this leaks the single-writer
    // slot AND hangs the bridge forever (the `Attachment` keeps `tx` alive, so
    // `rx.recv()` never returns `Closed`).
    let evict = attach.evict.clone();
    let notified = evict.notify.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();
    let gzip = tab.gzip;
    let mut traffic = Traffic::local(id, tab, Instant::now());

    let (sink, mut from_client) = socket.split();
    let mut outbox = Outbox::start(sink);
    let opening = Opening::new(id, daemon_id, environment, &labels, gzip);
    open(&mut outbox, id, &opening, &mut attach, &mut traffic);
    // Keep the socket warm on quiet/low-quality links: an idle terminal sends no
    // bytes, so a NAT/proxy idle-timeout silently drops it. The pongs the pings
    // earn are what `heard` counts (see LIVENESS above).
    let liveness = Liveness::current();
    let mut ping = tokio::time::interval(liveness.ping_every);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ping.tick().await; // consume the immediate first tick — no ping on connect
    let mut heard = Instant::now();
    // Each ping carries its own number, so a pong is timed against its ping.
    let mut ping_seq: u64 = 0;

    // False once the size sender is gone, so `changed()` cannot return at once
    // on every turn of the loop.
    let mut sizes = true;
    let end = loop {
        let flow = tokio::select! {
            _ = shutdown.changed() => {
                ControlFlow::Break(Some(session::EndReason::DaemonShutdown))
            }
            // Taken over, closed, or the child exited — the token carries which.
            _ = &mut notified => ControlFlow::Break(Some(
                evict.reason().unwrap_or(session::EndReason::ChildExited),
            )),
            _ = ping.tick() => {
                on_ping(&mut outbox, &mut traffic, &liveness, heard, &mut ping_seq)
            }
            changed = attach.size.changed(), if sizes => {
                on_size(changed, &mut outbox, id, &mut attach, &mut sizes)
            }
            recv = attach.rx.recv(), if outbox.queued.is_empty() => {
                on_output(recv, &mut outbox, id, &opening, &mut attach, &mut traffic)
            }
            permit = outbox.out.reserve(), if !outbox.queued.is_empty() => {
                silent_if_err(hand_on(permit, &mut outbox.queued))
            }
            incoming = from_client.next() => {
                on_client(incoming, &mut heard, &attach, &mut traffic)
            }
        };
        if let ControlFlow::Break(end) = flow {
            break end;
        }
    };
    // `None` announces nothing: the socket is already gone there.
    if let Some(reason) = end {
        announce_end(&mut outbox, id, &opening, &mut attach, &mut traffic, reason).await;
    }
    // The socket is dropped here, as the bridge dropped it before it had a
    // writer: whatever the writer still holds is not sent.
    if let Some(writer) = &outbox.writer {
        writer.abort();
    }
    traffic.log(end.map(|r| r.as_wire()), Instant::now());
    // Detach, do NOT close: dropping `attach` releases the single-writer slot; the
    // session (and its child) live on for a later reattach.
    drop(attach);
}

/// Silent, not announced: a client that is in fact alive reads the drop as a
/// flaky link and reattaches (issue #334). The ping goes before the frames
/// that wait in the outbox, so its round trip times the link, not the queue.
fn on_ping(
    outbox: &mut Outbox,
    traffic: &mut Traffic,
    liveness: &Liveness,
    heard: Instant,
    ping_seq: &mut u64,
) -> Flow {
    if liveness.is_silent(heard.elapsed()) {
        return ControlFlow::Break(None);
    }
    *ping_seq += 1;
    let payload = ping_seq.to_le_bytes();
    traffic.ping_sent(&payload, Instant::now());
    outbox.queued.push_front(Out::Ping(payload.to_vec()));
    ControlFlow::Continue(())
}

/// A new session size goes to the client. A closed size sender stops the arm.
fn on_size(
    changed: Result<(), tokio::sync::watch::error::RecvError>,
    outbox: &mut Outbox,
    id: session::SessionId,
    attach: &mut session::Attachment,
    sizes: &mut bool,
) -> Flow {
    if changed.is_err() {
        *sizes = false;
        return ControlFlow::Continue(());
    }
    let size = *attach.size.borrow_and_update();
    send_size(outbox, id, size);
    ControlFlow::Continue(())
}

/// One turn of the live output stream. A lag sends the screen again.
fn on_output(
    recv: Result<Vec<u8>, RecvError>,
    outbox: &mut Outbox,
    id: session::SessionId,
    opening: &Opening,
    attach: &mut session::Attachment,
    traffic: &mut Traffic,
) -> Flow {
    match recv {
        Ok(bytes) => {
            send_output(outbox, id, bytes, traffic);
            ControlFlow::Continue(())
        }
        Err(RecvError::Lagged(skipped)) => {
            traffic.lagged(skipped);
            resync(outbox, id, opening, attach, traffic);
            ControlFlow::Continue(())
        }
        Err(RecvError::Closed) => ControlFlow::Break(Some(session::EndReason::ChildExited)),
    }
}

/// One message from the client. Every message that arrives, of any kind,
/// counts as `heard`.
fn on_client(
    incoming: Option<Result<Message, axum::Error>>,
    heard: &mut Instant,
    attach: &session::Attachment,
    traffic: &mut Traffic,
) -> Flow {
    if matches!(incoming, Some(Ok(_))) {
        *heard = Instant::now();
    }
    match incoming {
        Some(Ok(Message::Binary(bytes))) => {
            traffic.inbound(bytes.len());
            on_client_frame(&bytes, attach, traffic)
        }
        Some(Ok(Message::Close(_))) => {
            traffic.client_closed();
            ControlFlow::Break(None)
        }
        Some(Ok(Message::Pong(payload))) => {
            traffic.pong_received(&payload, Instant::now());
            ControlFlow::Continue(())
        }
        // Text and ping: counted in `heard`, nothing else.
        Some(Ok(_)) => ControlFlow::Continue(()),
        None | Some(Err(_)) => ControlFlow::Break(None),
    }
}

fn on_client_frame(bytes: &[u8], attach: &session::Attachment, traffic: &mut Traffic) -> Flow {
    match protocol::decode(bytes) {
        Ok(Frame::Terminal { data, .. }) => silent_if_err(attach.write(&data)),
        Ok(Frame::Command(cmd)) if cmd.verb == "resize" => {
            if let Some((rows, cols)) = resize_of(&cmd) {
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "a resize fails only once the console has ended, which the loop sees"
                )]
                let _ = attach.resize(rows, cols);
            }
            ControlFlow::Continue(())
        }
        Ok(Frame::Command(cmd)) => {
            traffic.client_command(&cmd);
            ControlFlow::Continue(())
        }
        // Other frames carry no session meaning here.
        _ => ControlFlow::Continue(()),
    }
}

/// The size a `resize` command asks for. `try_into` rejects a garbage or
/// oversized dimension rather than truncating it into a wrong terminal size.
fn resize_of(cmd: &Command) -> Option<(u16, u16)> {
    let dimension = |key: &str| -> Option<u16> {
        cmd.payload
            .get(key)
            .and_then(|v| v.as_u64()?.try_into().ok())
    };
    Some((dimension("rows")?, dimension("cols")?))
}

#[cfg(test)]
mod tests;
