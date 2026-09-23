//! The byte bridge of an attached `/ws/session`: replay, live stream, input,
//! resize, and the eviction/exit teardown.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};

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
pub(crate) async fn session_ws(
    mut socket: WebSocket,
    mut attach: session::Attachment,
    id: session::SessionId,
    daemon_id: String,
    environment: String,
    labels: SessionLabels,
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

    let open = Frame::Command(Command {
        id,
        verb: "session-open".to_string(),
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
        }),
    });
    if socket
        .send(Message::Binary(protocol::encode(&open).into()))
        .await
        .is_err()
    {
        return;
    }

    // Replay the backlog first so a reattaching client sees history before the
    // live stream resumes. Skip an empty snapshot (a fresh session).
    if !attach.snapshot.is_empty() {
        let frame = Frame::Terminal {
            session: id,
            data: std::mem::take(&mut attach.snapshot),
        };
        if socket
            .send(Message::Binary(protocol::encode(&frame).into()))
            .await
            .is_err()
        {
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
                if socket.send(Message::Ping(Default::default())).await.is_err() {
                    break;
                }
            }
            recv = attach.rx.recv() => match recv {
                Ok(bytes) => {
                    let frame = Frame::Terminal { session: id, data: bytes };
                    if socket
                        .send(Message::Binary(protocol::encode(&frame).into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                // A burst outran this slow attach; scrollback already replayed and
                // xterm.js tolerates a gap, so keep streaming.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    end = Some(session::EndReason::ChildExited);
                    break;
                }
            },
            incoming = socket.recv() => {
                if matches!(incoming, Some(Ok(_))) {
                    heard = Instant::now();
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
                        _ => {} // other frames carry no session meaning here
                    },
                    Some(Ok(Message::Close(_))) | None => {
                        break;
                    },
                    Some(Ok(_)) => {} // text/ping/pong: counted in `heard`, nothing else
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
    // Detach, do NOT close: dropping `attach` releases the single-writer slot; the
    // session (and its child) live on for a later reattach.
    drop(attach);
}
