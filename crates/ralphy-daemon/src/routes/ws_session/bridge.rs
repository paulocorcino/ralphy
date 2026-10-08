//! The byte bridge of an attached `/ws/session`: replay, live stream, input,
//! resize, and the eviction/exit teardown.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};

use super::traffic::{Leave, Tab, Traffic, SESSION_OPEN};
use super::SessionLabels;
use crate::protocol::{Command, Frame};
use crate::routes::send_command;
use crate::{protocol, session};

/// How often the bridge pings its client, and how long the client may stay
/// silent before the bridge gives up on it (ADR-0051 §9 amendment 2026-09-22).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Liveness {
    pub(crate) ping_every: Duration,
    pub(crate) silent_after: Duration,
}

/// The share of the 5 s end window that the queued output may use. The
/// remaining second is for `session-end` and `Close`, which must always be
/// sent (#334): a full queue on a slow link can otherwise use the whole window.
const DRAIN_LIMIT: Duration = Duration::from_secs(4);

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

/// Bridge one WebSocket to one daemon-owned session (the tmux model, #166).
/// FIRST announces `session-open` with the hosting identity, then replays the
/// scrollback snapshot and loops: session output (via the broadcast `rx`) →
/// `Frame::Terminal`; client `Frame::Terminal` → PTY stdin; client
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
#[allow(clippy::too_many_arguments)]
pub(crate) async fn session_ws(
    mut socket: WebSocket,
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
    let mut traffic = Traffic::local(id, tab, Instant::now());

    let open = Frame::Command(Command {
        id,
        verb: SESSION_OPEN.to_string(),
        payload: serde_json::json!({
            "session": id,
            "daemon_id": daemon_id,
            "environment": environment,
            // Absent for every vendor but Claude, and for the free console —
            // which is the honest signal that those are not addressable.
            "name": labels.name,
            // The worktree NAME the console lives in, `null` for the primary;
            // re-announced on a reattach from the record (ADR-0063 §3).
            "checkout": labels.checkout,
            // Read-only from the start: this launch joined a session another
            // page drives (ADR-0050 amendment 2026-10-04).
            "watch": labels.watching,
            // A scrollback replay follows. Read by a relaying daemon for its
            // traffic summary; the page does not need it.
            "replay": !attach.snapshot.is_empty(),
        }),
    });
    if socket
        .send(Message::Binary(protocol::encode(&open).into()))
        .await
        .is_err()
    {
        traffic.log(None, Instant::now());
        return;
    }

    // Replay the backlog first so a reattaching client sees history before the
    // live stream resumes. Skip an empty snapshot (a fresh session).
    if !attach.snapshot.is_empty() {
        let frame = Frame::Terminal {
            session: id,
            data: std::mem::take(&mut attach.snapshot),
        };
        let encoded = protocol::encode(&frame);
        traffic.replay(encoded.len());
        if socket.send(Message::Binary(encoded.into())).await.is_err() {
            traffic.log(None, Instant::now());
            return;
        }
    }
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

    // A DELIBERATE end (daemon shutdown, takeover/child-exit eviction, or the
    // broadcast sender closing) is ANNOUNCED after the loop — a data frame naming
    // the reason, then the Close frame. `None` means the loop fell out some other
    // way (client close, network drop, write failure) and the bridge stays SILENT:
    // announcing there would tell a client its session ended when it did not, and
    // the client would park instead of recovering the flaky link (issue #334).
    let mut end: Option<session::EndReason> = None;
    loop {
        tokio::select! {
            _ = shutdown.changed() => { end = Some(session::EndReason::DaemonShutdown); break; }
            // Taken over, closed, or the child exited — the token carries which.
            _ = &mut notified => {
                end = Some(evict.reason().unwrap_or(session::EndReason::ChildExited));
                break;
            }
            _ = ping.tick() => {
                // Silent, not announced: a client that is in fact alive reads
                // the drop as a flaky link and reattaches (issue #334).
                if liveness.is_silent(heard.elapsed()) {
                    break;
                }
                ping_seq += 1;
                let payload = ping_seq.to_le_bytes();
                traffic.ping_sent(&payload, Instant::now());
                if socket.send(Message::Ping(payload.to_vec().into())).await.is_err() {
                    break;
                }
            }
            recv = attach.rx.recv() => match recv {
                Ok(bytes) => {
                    if send_output(&mut socket, id, bytes, &mut traffic).await.is_err() {
                        break;
                    }
                }
                // A burst outran this slow attach; scrollback already replayed and
                // xterm.js tolerates a gap, so keep streaming.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    traffic.lagged(skipped);
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    end = Some(session::EndReason::ChildExited);
                    break;
                }
            },
            incoming = socket.recv() => {
                if matches!(incoming, Some(Ok(_))) {
                    heard = Instant::now();
                }
                if let Some(Ok(Message::Binary(bytes))) = &incoming {
                    traffic.inbound(bytes.len());
                }
                match incoming {
                    Some(Ok(Message::Binary(bytes))) => match protocol::decode(&bytes) {
                        Ok(Frame::Terminal { data, .. }) => {
                            if attach.write(&data).is_err() {
                                break;
                            }
                        }
                        Ok(Frame::Command(cmd)) if cmd.verb == "resize" => {
                            // `try_into` rejects a garbage/oversized dimension rather
                            // than truncating it into a wrong terminal size.
                            let rows: Option<u16> =
                                cmd.payload.get("rows").and_then(|v| v.as_u64()?.try_into().ok());
                            let cols: Option<u16> =
                                cmd.payload.get("cols").and_then(|v| v.as_u64()?.try_into().ok());
                            if let (Some(rows), Some(cols)) = (rows, cols) {
                                let _ = attach.resize(rows, cols);
                            }
                        }
                        Ok(Frame::Command(cmd)) => {
                            if let Some(leave) = Leave::from_command(&cmd) {
                                traffic.client_left(leave);
                            }
                        }
                        _ => {} // other frames carry no session meaning here
                    },
                    Some(Ok(Message::Close(_))) => {
                        traffic.client_closed();
                        break;
                    }
                    None => break,
                    Some(Ok(Message::Pong(payload))) => {
                        traffic.pong_received(&payload, Instant::now());
                    }
                    Some(Ok(_)) => {} // text/ping: counted in `heard`, nothing else
                    Some(Err(_)) => break,
                }
            }
        }
    }
    // ANNOUNCEMENT-BEFORE-CLOSE INVARIANT (issue #334), to hold on every return
    // path: a deliberate end is named in a DATA frame first, and only then does
    // the Close frame follow and the attachment drop. Meaning placed in the close
    // metadata is meaning lost — the browser reports `1005 / wasClean=false` for
    // this very `Close(None)`, which is why the client could not tell an eviction
    // from a flaky link and stole the session back. The early `return` in the
    // snapshot replay above announces nothing because the socket is already gone,
    // and `end == None` stays silent by design (see the declaration).
    // Bounded: these are the only sends made AFTER the loop that would surface a
    // wedged peer as an error, so an unbounded await here would defer
    // `drop(attach)` — and the session's scrollback ring with it — indefinitely.
    if let Some(reason) = end {
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            // The child's last output is sent before its end is named. Only a
            // child end drains: a takeover leaves the stream to the new owner,
            // whose replay already holds those bytes, and the old client is
            // about to be told it lost the session.
            if reason == session::EndReason::ChildExited
                && tokio::time::timeout(
                    DRAIN_LIMIT,
                    send_queued(&mut socket, id, &mut attach.rx, &mut traffic),
                )
                .await
                .is_err()
            {
                // A client that cannot take the queue in time loses the rest of
                // it from this socket; the scrollback keeps it for a later view.
                tracing::debug!("the queued output did not drain in time; naming the end");
            }
            send_command(
                &mut socket,
                0,
                "session-end",
                serde_json::json!({
                    "reason": reason.as_wire(),
                    "daemon_id": daemon_id,
                    "environment": environment,
                }),
            )
            .await;
            let _ = socket.send(Message::Close(None)).await;
        })
        .await;
    }
    traffic.log(end.map(|r| r.as_wire()), Instant::now());
    // Detach, do NOT close: dropping `attach` releases the single-writer slot; the
    // session (and its child) live on for a later reattach.
    drop(attach);
}

/// Send one chunk of session output to the client as a `Terminal` frame.
async fn send_output(
    socket: &mut WebSocket,
    id: session::SessionId,
    bytes: Vec<u8>,
    traffic: &mut Traffic,
) -> Result<(), axum::Error> {
    let frame = Frame::Terminal {
        session: id,
        data: bytes,
    };
    let encoded = protocol::encode(&frame);
    traffic.live(encoded.len());
    socket.send(Message::Binary(encoded.into())).await
}

/// Send every chunk already queued on `rx` when the loop ended. The pump
/// broadcasts the child's last output and only then fires the eviction, so
/// both can be ready together, and the loop's `select!` may take the eviction
/// first. Bounded by the count queued now: on a `close` the eviction fires
/// before the kill, and the child can still print while this drains.
async fn send_queued(
    socket: &mut WebSocket,
    id: session::SessionId,
    rx: &mut tokio::sync::broadcast::Receiver<Vec<u8>>,
    traffic: &mut Traffic,
) {
    use tokio::sync::broadcast::error::TryRecvError;
    for _ in 0..rx.len() {
        match rx.try_recv() {
            Ok(bytes) => {
                if send_output(socket, id, bytes, traffic).await.is_err() {
                    return;
                }
            }
            Err(TryRecvError::Lagged(skipped)) => traffic.lagged(skipped),
            Err(TryRecvError::Empty | TryRecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use axum::extract::ws::WebSocketUpgrade;
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite;

    /// A console whose child has exited: the eviction is signalled and the
    /// attachment's output stream is the test's own, holding `chunks`, so the
    /// client receives exactly these and nothing the shell prints.
    fn exited_session(
        chunks: &[Vec<u8>],
        capacity: usize,
    ) -> (session::SessionId, session::Attachment) {
        let manager = Arc::new(session::SessionManager::new());
        let spec = session::console_spec(std::env::temp_dir(), 24, 80, None);
        let (id, mut attach) = manager
            .spawn_attached(
                "~".to_string(),
                "console".to_string(),
                "console".to_string(),
                None,
                None,
                None,
                spec,
            )
            .expect("the platform shell must spawn — the free console depends on it");
        let (tx, rx) = tokio::sync::broadcast::channel::<Vec<u8>>(capacity);
        attach.rx = rx;
        attach.snapshot.clear();
        for chunk in chunks {
            tx.send(chunk.clone())
                .expect("the attachment holds a receiver");
        }
        assert!(manager.close(id), "the session was live");
        assert_eq!(
            attach.evict.reason(),
            Some(session::EndReason::ChildExited),
            "the eviction is signalled before the bridge starts"
        );
        (id, attach)
    }

    /// Serve `session_ws` for one client on `listener`.
    fn serve_bridge(
        listener: tokio::net::TcpListener,
        id: session::SessionId,
        attach: session::Attachment,
    ) -> tokio::task::JoinHandle<std::io::Result<()>> {
        let slot = Arc::new(Mutex::new(Some(attach)));
        let (stop, shutdown) = tokio::sync::watch::channel(false);
        let app = axum::Router::new().route(
            "/",
            axum::routing::get(move |ws: WebSocketUpgrade| {
                let attach = slot
                    .lock()
                    .expect("slot mutex")
                    .take()
                    .expect("one client connects");
                let shutdown = shutdown.clone();
                async move {
                    ws.on_upgrade(move |socket| {
                        session_ws(
                            socket,
                            attach,
                            id,
                            "daemon".to_string(),
                            "test".to_string(),
                            SessionLabels {
                                name: None,
                                checkout: None,
                                watching: false,
                            },
                            Tab {
                                holder: None,
                                device: None,
                            },
                            shutdown,
                        )
                    })
                }
            }),
        );
        tokio::spawn(async move {
            // The sender stays alive as long as the server runs.
            let _stop = stop;
            axum::serve(listener, app).await
        })
    }

    /// What a client saw: the output, where `session-end` came in it, and
    /// whether the `Close` frame followed.
    struct Seen {
        received: Vec<u8>,
        ended_after: Option<usize>,
        closed: bool,
    }

    /// Read until the `Close` frame, pausing `pause` after every message.
    async fn read_until_close(
        client: &mut (impl futures_util::Stream<Item = Result<tungstenite::Message, tungstenite::Error>>
                  + Unpin),
        pause: Duration,
    ) -> Seen {
        let mut seen = Seen {
            received: Vec::new(),
            ended_after: None,
            closed: false,
        };
        // The bridge drops the socket right after its Close frame, so the
        // read that follows the Close can fail: stop at the Close.
        while let Some(Ok(message)) = client.next().await {
            let bytes = match message {
                tungstenite::Message::Binary(bytes) => bytes,
                tungstenite::Message::Close(_) => {
                    seen.closed = true;
                    break;
                }
                _ => continue,
            };
            match protocol::decode(&bytes).expect("a well-formed frame") {
                Frame::Terminal { data, .. } => {
                    assert!(
                        seen.ended_after.is_none(),
                        "output arrived after session-end"
                    );
                    seen.received.extend_from_slice(&data);
                }
                Frame::Command(cmd) if cmd.verb == "session-end" => {
                    assert_eq!(cmd.payload["reason"], "child-exited");
                    seen.ended_after = Some(seen.received.len());
                }
                _ => {}
            }
            tokio::time::sleep(pause).await;
        }
        seen
    }

    /// A child that exits leaves its last output queued on the attachment
    /// while the eviction is already signalled. Every queued chunk reaches the
    /// client before `session-end`. With both arms ready, an unbiased select
    /// takes the eviction about half the time, so a loop that drops queued
    /// output keeps all 32 chunks with a chance near 2^-32.
    #[tokio::test]
    async fn queued_output_reaches_the_client_before_the_session_end() {
        let chunks: Vec<Vec<u8>> = (0..32)
            .map(|n| format!("line {n}\r\n").into_bytes())
            .collect();
        let (id, attach) = exited_session(&chunks, 64);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a loopback port");
        let addr = listener
            .local_addr()
            .expect("a bound listener has an address");
        let server = serve_bridge(listener, id, attach);

        let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .expect("the bridge accepts the socket");
        let seen = tokio::time::timeout(
            Duration::from_secs(10),
            read_until_close(&mut client, Duration::ZERO),
        )
        .await
        .expect("the bridge closes the socket after session-end");
        server.abort();

        assert_eq!(
            String::from_utf8_lossy(&seen.received),
            String::from_utf8_lossy(&chunks.concat()),
            "every queued chunk reaches the client"
        );
        assert!(seen.ended_after.is_some(), "the end is announced");
    }

    /// A client that takes the queue too slowly cannot hold back the end:
    /// the drain stops at its own limit, so `session-end` and `Close` are
    /// still sent inside the 5 s end window. Both socket buffers are small
    /// and the client reads one chunk per 100 ms, so 100 chunks need about
    /// 10 s. This runs in real time: the daemon's tokio has no `test-util`.
    #[tokio::test]
    async fn a_drain_that_cannot_finish_still_sends_the_end_and_the_close() {
        let chunks: Vec<Vec<u8>> = (0..100u8).map(|n| vec![n; 64 * 1024]).collect();
        let (id, attach) = exited_session(&chunks, 128);
        let socket = tokio::net::TcpSocket::new_v4().expect("a loopback socket");
        socket
            .set_send_buffer_size(4096)
            .expect("the send buffer is settable");
        socket
            .bind("127.0.0.1:0".parse().expect("a socket address"))
            .expect("bind a loopback port");
        let listener = socket.listen(1).expect("listen on the loopback port");
        let addr = listener
            .local_addr()
            .expect("a bound listener has an address");
        let server = serve_bridge(listener, id, attach);

        let socket = tokio::net::TcpSocket::new_v4().expect("a loopback socket");
        socket
            .set_recv_buffer_size(4096)
            .expect("the receive buffer is settable");
        let stream = socket.connect(addr).await.expect("the bridge accepts");
        let (mut client, _) = tokio_tungstenite::client_async(format!("ws://{addr}/"), stream)
            .await
            .expect("the bridge upgrades the socket");
        let seen = tokio::time::timeout(
            Duration::from_secs(15),
            read_until_close(&mut client, Duration::from_millis(100)),
        )
        .await
        .expect("the bridge closes the socket inside its end window");
        server.abort();

        assert!(seen.ended_after.is_some(), "the end is announced");
        assert!(seen.closed, "the Close frame follows the end");
        assert!(
            seen.received.len() < chunks.concat().len(),
            "the drain stopped at its limit instead of sending everything"
        );
    }
}
