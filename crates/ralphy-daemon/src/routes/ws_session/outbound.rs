//! What the bridge of an attached `/ws/session` sends the client: the opening
//! (`session-open`, the size, the replay), live output, a resync after a lag,
//! and the announced end.

use std::io::Write;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};

use super::traffic::{Traffic, SESSION_OPEN};
use super::SessionLabels;
use crate::protocol::{Command, Frame};
use crate::routes::send_command;
use crate::{protocol, session};

/// The share of the 5 s end window that the queued output may use. The
/// remaining second is for `session-end` and `Close`, which must always be
/// sent (#334): a full queue on a slow link can otherwise use the whole window.
const DRAIN_LIMIT: Duration = Duration::from_secs(4);

/// The command verb that tells a client the session size (ADR-0079 D6).
pub(super) const SESSION_SIZE: &str = "session-size";

/// The `replay_encoding` of a gzipped replay frame (ADR-0079 D9).
const GZIP: &str = "gzip";

/// What every `session-open` of one socket says, and whether its tab asked
/// for a gzip replay.
pub(super) struct Opening {
    fields: serde_json::Value,
    gzip: bool,
    daemon_id: String,
    environment: String,
}

impl Opening {
    pub(super) fn new(
        id: session::SessionId,
        daemon_id: String,
        environment: String,
        labels: &SessionLabels,
        gzip: bool,
    ) -> Opening {
        let fields = serde_json::json!({
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
        });
        Opening {
            fields,
            gzip,
            daemon_id,
            environment,
        }
    }
}

/// Name a deliberate end to the client, then close the socket.
///
/// ANNOUNCEMENT-BEFORE-CLOSE INVARIANT (issue #334), to hold on every return
/// path: a deliberate end is named in a DATA frame first, and only then does
/// the Close frame follow and the attachment drop. Meaning placed in the close
/// metadata is meaning lost — the browser reports `1005 / wasClean=false` for
/// this very `Close(None)`, which is why the client could not tell an eviction
/// from a flaky link and stole the session back.
///
/// Bounded: these are the only sends made AFTER the loop that would surface a
/// wedged peer as an error, so an unbounded await here would defer
/// `drop(attach)` — and the session's scrollback ring with it — indefinitely.
pub(super) async fn announce_end(
    socket: &mut WebSocket,
    id: session::SessionId,
    opening: &Opening,
    attach: &mut session::Attachment,
    traffic: &mut Traffic,
    reason: session::EndReason,
) {
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        // The child's last output is sent before its end is named. Only a
        // child end drains: a takeover leaves the stream to the new owner,
        // whose replay already holds those bytes, and the old client is
        // about to be told it lost the session.
        if reason == session::EndReason::ChildExited
            && tokio::time::timeout(
                DRAIN_LIMIT,
                send_queued(socket, id, opening, attach, traffic),
            )
            .await
            .is_err()
        {
            // A client that cannot take the queue in time loses the rest of
            // it. The session is already gone, so nothing sends it later.
            tracing::debug!("the queued output did not drain in time; naming the end");
        }
        send_command(
            socket,
            0,
            "session-end",
            serde_json::json!({
                "reason": reason.as_wire(),
                "daemon_id": opening.daemon_id,
                "environment": opening.environment,
            }),
        )
        .await;
        let _ = socket.send(Message::Close(None)).await;
    })
    .await;
}

/// Send `session-open` (the fields of `opening`, then the replay's), the
/// session size, then the replay frame of `attach`: the screen snapshot, or
/// the bytes after the tab's `since`.
pub(super) async fn open(
    socket: &mut WebSocket,
    id: session::SessionId,
    opening: &Opening,
    attach: &mut session::Attachment,
    traffic: &mut Traffic,
) -> Result<(), axum::Error> {
    let replay = Replay::of(std::mem::take(&mut attach.snapshot), opening.gzip);
    let mut payload = opening.fields.clone();
    // A replay follows. A relaying daemon reads it for its traffic summary;
    // the page reads it to tell the replay from the live frames.
    payload["replay"] = (!replay.data.is_empty()).into();
    // The count of the session's output bytes at the end of the replay; the
    // page adds each live frame to it and sends it back as `since`.
    payload["offset"] = attach.offset.into();
    // The replay is only the bytes after the tab's `since`: the page keeps
    // its terminal. `false`: it resets it (ADR-0079 D8).
    payload["resume"] = attach.resume.into();
    // Absent when the replay is raw, as from a daemon that never gzips.
    if replay.gzipped {
        payload["replay_encoding"] = GZIP.into();
    }
    let open = Frame::Command(Command {
        id,
        verb: SESSION_OPEN.to_string(),
        payload,
    });
    socket
        .send(Message::Binary(protocol::encode(&open).into()))
        .await?;
    // The size the snapshot below was written at: a watcher paints at it. A
    // resize after the attach is still marked changed on `attach.size`, so
    // the loop sends it after the replay.
    send_size(socket, id, attach.replay_size).await?;
    // The replay goes before the live stream. An empty one is not sent: a
    // fresh session, or a resume with nothing new.
    if !replay.data.is_empty() {
        let sent = replay.data.len();
        let frame = Frame::Terminal {
            session: id,
            data: replay.data,
        };
        let encoded = protocol::encode(&frame);
        traffic.replay(encoded.len(), encoded.len() - sent + replay.raw_len);
        socket.send(Message::Binary(encoded.into())).await?;
    }
    Ok(())
}

/// The live receiver lagged: the page missed bytes, so its screen is wrong
/// and its byte count would make a later resume repeat bytes. Start it again
/// from the screen as it is now, as a reattach does: a new `session-open`
/// with `resume: false`, the size, and the snapshot.
pub(super) async fn resync(
    socket: &mut WebSocket,
    id: session::SessionId,
    opening: &Opening,
    attach: &mut session::Attachment,
    traffic: &mut Traffic,
) -> Result<(), axum::Error> {
    attach.resync();
    open(socket, id, opening, attach, traffic).await
}

/// The replay frame's data: the snapshot, gzipped when the tab asked for it.
struct Replay {
    data: Vec<u8>,
    gzipped: bool,
    /// The length of the snapshot, for the traffic summary.
    raw_len: usize,
}

impl Replay {
    fn of(snapshot: Vec<u8>, gzip: bool) -> Replay {
        let raw_len = snapshot.len();
        if gzip && !snapshot.is_empty() {
            match gzipped(&snapshot) {
                Ok(data) => {
                    return Replay {
                        data,
                        gzipped: true,
                        raw_len,
                    }
                }
                // A replay that does not compress goes raw; it is never dropped.
                Err(error) => tracing::warn!(%error, "the replay did not compress; sent raw"),
            }
        }
        Replay {
            data: snapshot,
            gzipped: false,
            raw_len,
        }
    }
}

fn gzipped(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut enc = flate2::write::GzEncoder::new(
        Vec::with_capacity(bytes.len() / 4),
        flate2::Compression::default(),
    );
    enc.write_all(bytes)?;
    enc.finish()
}

/// Tell the client the session size. Not counted in the traffic summary, like
/// `session-open`.
pub(super) async fn send_size(
    socket: &mut WebSocket,
    id: session::SessionId,
    (rows, cols): (u16, u16),
) -> Result<(), axum::Error> {
    let frame = Frame::Command(Command {
        id,
        verb: SESSION_SIZE.to_string(),
        payload: serde_json::json!({ "rows": rows, "cols": cols }),
    });
    socket
        .send(Message::Binary(protocol::encode(&frame).into()))
        .await
}

/// Send one chunk of session output to the client as a `Terminal` frame.
pub(super) async fn send_output(
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
    opening: &Opening,
    attach: &mut session::Attachment,
    traffic: &mut Traffic,
) {
    use tokio::sync::broadcast::error::TryRecvError;
    let mut left = attach.rx.len();
    while left > 0 {
        left -= 1;
        match attach.rx.try_recv() {
            Ok(bytes) => {
                if send_output(socket, id, bytes, traffic).await.is_err() {
                    return;
                }
            }
            Err(TryRecvError::Lagged(skipped)) => {
                traffic.lagged(skipped);
                if resync(socket, id, opening, attach, traffic).await.is_err() {
                    return;
                }
                left = attach.rx.len();
            }
            Err(TryRecvError::Empty | TryRecvError::Closed) => return,
        }
    }
}
