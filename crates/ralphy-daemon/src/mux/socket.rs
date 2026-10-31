//! The page socket `/ws`: channel 0 and the channels a page opens on it.
//!
//! Channel 0 is the socket itself: the presence heartbeat, every push, and
//! the bare frames the page sends (`presence.report`, `channel.open`,
//! `channel.close`), exactly as before the socket took channels. A page that
//! never sends a channel frame sees the socket it always saw.
//!
//! Three tasks serve one socket. The writer sends every channel's frames in
//! turn. The reader ([`read`]) reads the socket on its own, so input never
//! waits behind output: it keeps the table of open channels and hands each
//! one the frames the page sends on it. The loop here sends the heartbeat,
//! the pushes and the pings, and ends the socket. When the socket ends,
//! every channel ends with it.

mod page;
mod read;

use axum::extract::ws::WebSocket;
use futures_util::StreamExt;
use tokio::sync::{broadcast, watch};
use tokio::time::Instant;

use super::kinds::Ctx;
use super::{ChannelSender, Mode, Out, Writer};
use crate::routes::{Liveness, Push};
use crate::{identity, presence_board, protocol};
use page::{Arms, PageEntry, HEARTBEAT_EVERY};

/// The bytes of a channel frame's header: the tag, the channel id and the
/// flags.
const CHANNEL_HEADER: usize = 1 + 4 + 1;

/// The largest message a page socket takes: one piece and its header.
pub(crate) const MAX_MESSAGE: usize = protocol::MAX_PIECE + CHANNEL_HEADER;

/// What one page socket serves from.
pub(crate) struct Page {
    pub(crate) identity: Option<identity::Identity>,
    pub(crate) start: std::time::Instant,
    pub(crate) shutdown: watch::Receiver<bool>,
    pub(crate) sessions: broadcast::Receiver<()>,
    pub(crate) pushes: broadcast::Receiver<Push>,
    pub(crate) board: presence_board::Shared,
    pub(crate) ctx: Ctx,
}

/// Serve one page socket until the page goes, it stays silent too long, a
/// send fails, or the daemon shuts down.
///
/// The page sends `presence.report` on channel 0 (docs/adr/0085 D14): each
/// one goes on the presence board, under this socket's entry, which goes
/// when the socket ends.
pub(crate) async fn serve(socket: WebSocket, page: Page) {
    serve_with(socket, page, Liveness::current()).await;
}

async fn serve_with(socket: WebSocket, page: Page, liveness: Liveness) {
    let Page {
        identity,
        start,
        mut shutdown,
        sessions,
        pushes,
        board,
        ctx,
    } = page;
    let (sink, stream) = socket.split();
    let writer = Writer::start(sink, Mode::Channels);
    let out = writer.channel(0);
    let (heard_tx, mut heard) = watch::channel(Instant::now());
    let reader = read::Reader::new(writer.clone(), out.clone(), PageEntry::open(board), ctx);
    let mut reader = tokio::spawn(read::read(stream, reader, heard_tx));
    let mut arms = Arms::new(sessions, pushes);
    let mut tick = tokio::time::interval(HEARTBEAT_EVERY);
    let mut ping = tokio::time::interval(liveness.ping_every);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // The first tick is at once: no ping on connect.
    ping.tick().await;
    let silence = tokio::time::sleep(liveness.silent_after);
    tokio::pin!(silence);
    loop {
        let stop = tokio::select! {
            event = arms.recv() => arms.send(&out, event).await,
            // Ok = the daemon signalled shutdown; Err = the sender was
            // dropped (its runtime is going away). Either way, stop.
            _ = shutdown.changed() => true,
            _ = tick.tick() => !page::send_presence(&out, identity.as_ref(), start).await,
            _ = ping.tick() => !send_ping(&out).await,
            changed = heard.changed() => match changed {
                Ok(()) => {
                    let at = *heard.borrow_and_update();
                    silence.as_mut().reset(at + liveness.silent_after);
                    false
                }
                // The reader has ended: the socket is gone.
                Err(_) => true,
            },
            // A page answers every ping, so a page that sends nothing for
            // this long is gone, whatever the sends say.
            () = &mut silence => true,
            _ = &mut reader => true,
        };
        if stop {
            break;
        }
    }
    // The reader holds the table: stopping it ends every channel. A client
    // that reads nothing keeps the writer blocked in the socket send, so the
    // writer is stopped here, not left to the dropped handle.
    reader.abort();
    writer.abort();
}

/// Queue one ping on channel 0. `false` when the writer has ended.
async fn send_ping(out: &ChannelSender) -> bool {
    out.put(Out::Ping(Vec::new())).await.is_ok()
}

#[cfg(test)]
mod tests;
