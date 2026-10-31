//! What the bridge of an attached console sends the page: the opening
//! (`session-open`, the size, the replay), live output, a resync after a lag,
//! and the announced end. Each goes through the queue of the console's
//! channel on the page socket's writer.

use std::collections::VecDeque;
use std::io::Write;
use std::time::Duration;

use super::traffic::{Traffic, SESSION_OPEN};
use super::SessionLabels;
use crate::mux::{ChannelSender, Out, Permit, WriterClosed};
use crate::protocol::{Command, Frame};
use crate::routes::send_command_on;
use crate::{protocol, session};

/// The share of the 5 s end window that the queued output may use. The
/// remaining second is for `session-end`, which must always be sent (#334):
/// a full queue on a slow link can otherwise use the whole window.
const DRAIN_LIMIT: Duration = Duration::from_secs(4);

/// The whole end window: the drain and `session-end`. Past it the bridge
/// gives up, so a client that does not read cannot keep the session's
/// scrollback ring alive (#334).
const END_WINDOW: Duration = Duration::from_secs(5);

/// The command verb that tells a client the session size (ADR-0079 D6).
pub(super) const SESSION_SIZE: &str = "session-size";

/// The `replay_encoding` of a gzipped replay frame (ADR-0079 D9).
const GZIP: &str = "gzip";

/// The bridge's queue on the page socket's writer, and the items made for
/// the page that the writer has not taken yet, in order. The bridge loop
/// hands them on one at a time when the writer has room, and keeps reading
/// input while they wait (ADR-0082 D5).
pub(super) struct Outbox {
    pub(super) out: ChannelSender,
    pub(super) queued: VecDeque<Out>,
}

impl Outbox {
    /// An outbox on the queue `out` of a writer the page socket owns.
    pub(super) fn on(out: ChannelSender) -> Outbox {
        Outbox {
            out,
            queued: VecDeque::new(),
        }
    }

    fn frame(&mut self, encoded: Vec<u8>) {
        self.queued.push_back(Out::Frame(encoded));
    }

    /// Hand every queued item to the writer, waiting for room for each.
    async fn flush(&mut self) -> Result<(), WriterClosed> {
        while let Some(out) = self.queued.pop_front() {
            self.out.put(out).await?;
        }
        Ok(())
    }
}

/// Hand the first item of `queued` to the writer through the room `permit`
/// holds. Fails when the writer has ended: the socket is gone.
pub(super) fn hand_on(
    permit: Result<Permit, WriterClosed>,
    queued: &mut VecDeque<Out>,
) -> Result<(), WriterClosed> {
    let permit = permit?;
    if let Some(out) = queued.pop_front() {
        permit.put(out);
    }
    Ok(())
}

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

/// Name a deliberate end to the page. The socket stays open for its other
/// channels; the kind closes the channel after it.
///
/// ANNOUNCEMENT-BEFORE-CLOSE INVARIANT (issue #334), to hold on every return
/// path: a deliberate end is named in a DATA frame first, and only then does
/// the channel close and the attachment drop. Meaning placed in the close
/// metadata is meaning lost: the page could not tell an eviction from a
/// flaky link and stole the session back.
///
/// Bounded by [`END_WINDOW`]: these are the only sends made AFTER the loop
/// that would surface a wedged peer as an error, so an unbounded await here
/// would defer `drop(attach)` — and the session's scrollback ring with it —
/// indefinitely.
pub(super) async fn announce_end(
    outbox: &mut Outbox,
    id: session::SessionId,
    opening: &Opening,
    attach: &mut session::Attachment,
    traffic: &mut Traffic,
    reason: session::EndReason,
) {
    let farewell = async {
        if tokio::time::timeout(
            DRAIN_LIMIT,
            drain(outbox, id, opening, attach, traffic, reason),
        )
        .await
        .is_err()
        {
            // A client that cannot take the queue in time loses the rest of
            // it. The session is already gone, so nothing sends it later.
            tracing::debug!("the queued output did not drain in time; naming the end");
        }
        outbox.queued.clear();
        let end = serde_json::json!({
            "reason": reason.as_wire(),
            "daemon_id": opening.daemon_id,
            "environment": opening.environment,
        });
        send_command_on(&outbox.out, 0, "session-end", end).await
    };
    match tokio::time::timeout(END_WINDOW, farewell).await {
        Ok(Ok(())) => {}
        Ok(Err(WriterClosed)) => tracing::debug!("the client left before the end was named"),
        Err(_) => tracing::debug!("the client did not take the end within the end window"),
    }
}

/// The output made before the end goes out before the end is named. Only a
/// child end also drains the receiver: a takeover leaves the stream to the
/// new owner, whose replay already holds those bytes, and the old client is
/// about to be told it lost the session.
async fn drain(
    outbox: &mut Outbox,
    id: session::SessionId,
    opening: &Opening,
    attach: &mut session::Attachment,
    traffic: &mut Traffic,
    reason: session::EndReason,
) -> Result<(), WriterClosed> {
    outbox.flush().await?;
    if reason == session::EndReason::ChildExited {
        send_queued(outbox, id, opening, attach, traffic).await?;
    }
    Ok(())
}

/// Queue `session-open` (the fields of `opening`, then the replay's), the
/// session size, then the replay frame of `attach`: the screen snapshot, or
/// the bytes after the tab's `since`.
pub(super) fn open(
    outbox: &mut Outbox,
    id: session::SessionId,
    opening: &Opening,
    attach: &mut session::Attachment,
    traffic: &mut Traffic,
) {
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
    outbox.frame(protocol::encode(&open));
    // The size the snapshot below was written at: a watcher paints at it. A
    // resize after the attach is still marked changed on `attach.size`, so
    // the loop sends it after the replay.
    send_size(outbox, id, attach.replay_size);
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
        outbox.frame(encoded);
    }
}

/// The live receiver lagged: the page missed bytes, so its screen is wrong
/// and its byte count would make a later resume repeat bytes. Start it again
/// from the screen as it is now, as a reattach does: a new `session-open`
/// with `resume: false`, the size, and the snapshot.
pub(super) fn resync(
    outbox: &mut Outbox,
    id: session::SessionId,
    opening: &Opening,
    attach: &mut session::Attachment,
    traffic: &mut Traffic,
) {
    attach.resync();
    open(outbox, id, opening, attach, traffic);
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

/// Queue the session size for the client. Not counted in the traffic
/// summary, like `session-open`.
pub(super) fn send_size(outbox: &mut Outbox, id: session::SessionId, (rows, cols): (u16, u16)) {
    let frame = Frame::Command(Command {
        id,
        verb: SESSION_SIZE.to_string(),
        payload: serde_json::json!({ "rows": rows, "cols": cols }),
    });
    outbox.frame(protocol::encode(&frame));
}

/// Queue one chunk of session output for the client as a `Terminal` frame.
pub(super) fn send_output(
    outbox: &mut Outbox,
    id: session::SessionId,
    bytes: Vec<u8>,
    traffic: &mut Traffic,
) {
    let frame = Frame::Terminal {
        session: id,
        data: bytes,
    };
    let encoded = protocol::encode(&frame);
    traffic.live(encoded.len());
    outbox.frame(encoded);
}

/// Send every chunk already queued on `rx` when the loop ended. The pump
/// broadcasts the child's last output and only then fires the eviction, so
/// both can be ready together, and the loop's `select!` may take the eviction
/// first. Bounded by the count queued now: on a `close` the eviction fires
/// before the kill, and the child can still print while this drains.
async fn send_queued(
    outbox: &mut Outbox,
    id: session::SessionId,
    opening: &Opening,
    attach: &mut session::Attachment,
    traffic: &mut Traffic,
) -> Result<(), WriterClosed> {
    use tokio::sync::broadcast::error::TryRecvError;
    let mut left = attach.rx.len();
    while left > 0 {
        left -= 1;
        match attach.rx.try_recv() {
            Ok(bytes) => send_output(outbox, id, bytes, traffic),
            Err(TryRecvError::Lagged(skipped)) => {
                traffic.lagged(skipped);
                resync(outbox, id, opening, attach, traffic);
                left = attach.rx.len();
            }
            Err(TryRecvError::Empty | TryRecvError::Closed) => break,
        }
        outbox.flush().await?;
    }
    Ok(())
}
