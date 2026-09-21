//! The byte bridge of an attached `/ws/session`: replay, live stream, input,
//! resize, and the eviction/exit teardown.

use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};

use super::SessionLabels;
use crate::protocol::{Command, Frame};
use crate::routes::send_command;
use crate::{protocol, session};

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
    // bytes, so a NAT/proxy idle-timeout (or a lossy path with nothing to
    // retransmit) silently drops it. A periodic WS ping — which the browser
    // auto-pongs — keeps intermediaries alive and surfaces a truly dead peer as a
    // send error that tears the bridge down (detach-only; the child survives for a
    // reattach). 20s is well under common 30–60s proxy idle windows.
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ping.tick().await; // consume the immediate first tick — no ping on connect

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
            incoming = socket.recv() => match incoming {
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
                Some(Ok(_)) => {} // text/ping/pong: ignore
                Some(Err(_)) => break,
            },
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
