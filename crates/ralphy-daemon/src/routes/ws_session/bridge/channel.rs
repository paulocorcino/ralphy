//! A console served on a channel of the page socket `/ws` (the tmux model,
//! #166): the open is decided first, and an attached session then runs the
//! arms of the bridge on the channel. The page socket owns the liveness, so
//! a channel has no ping, and its end closes the channel, not the socket.

use std::ops::ControlFlow;
use std::time::Instant;

use tokio::sync::mpsc;

use super::super::outbound::{announce_end, hand_on, open, Opening, Outbox};
use super::super::refuse::{Refusal, Refuser, Told};
use super::super::traffic::Traffic;
use super::super::upgrade::{Decided, Opened};
use super::super::PendingRecord;
use super::super::{SessionHost, SessionQuery};
use super::{on_client_frame, on_output, on_size, silent_if_err, Flow};
use crate::mux::ChannelSender;
use crate::routes::audit_layer::SocketAudit;
use crate::routes::send_command_on;
use crate::{peer, session};

/// How a console channel ended, for the `channel.closed` its kind sends.
pub(crate) enum ConsoleEnd {
    /// The open was refused. A launch and a superseded claim were told why
    /// in `session-end`; a reattach was not, so that the page counts it as a
    /// failed open.
    Refused,
    /// The bridge ended. A deliberate end was named in `session-end` first;
    /// a silent one, when the socket is gone, was not.
    Ended,
    /// The peer's channel is open: the peer arm relays it.
    Relay(Box<PeerRelay>),
}

/// The channel open on the peer, the traffic summary of the relay, and the
/// audit line the relay writes once the peer accepts the open.
pub(crate) struct PeerRelay {
    pub(crate) peer: peer::client::PeerChannel,
    pub(crate) traffic: Traffic,
    pub(crate) record: Option<PendingRecord>,
}

/// Serve the console that `query` names on the channel whose queue is `out`.
/// `frames` are the inner frames the page sends on the channel.
pub(crate) async fn serve_console(
    query: SessionQuery,
    host: SessionHost,
    who: SocketAudit,
    out: &ChannelSender,
    frames: &mut mpsc::Receiver<Vec<u8>>,
) -> ConsoleEnd {
    let Decided {
        opened,
        refuser,
        daemon_id,
        tab,
        shutdown,
    } = Decided::of(query, host, who).await;
    match opened {
        Opened::Session {
            att,
            id,
            environment,
            labels,
        } => {
            let opening = Opening::new(id, daemon_id, environment, &labels, tab.gzip);
            let attached = Attached {
                attach: att,
                id,
                traffic: Logged::new(Traffic::local(id, tab, Instant::now())),
                opening,
            };
            attached.bridge(out, frames, shutdown).await;
            ConsoleEnd::Ended
        }
        Opened::Relay(relay) => ConsoleEnd::Relay(relay),
        Opened::Refused(refusal) => refuse(&refuser, refusal, out).await,
    }
}

/// Run the bridge of an attached session on the channel whose queue is
/// `out`, as [`serve_console`] runs it once the open is decided. The session
/// has no name and no checkout, and its daemon is `daemon` in `test`.
#[cfg(test)]
pub(super) async fn bridge_attached(
    attach: session::Attachment,
    id: session::SessionId,
    tab: super::super::traffic::Tab,
    out: &ChannelSender,
    frames: &mut mpsc::Receiver<Vec<u8>>,
    shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let labels = super::super::SessionLabels::default();
    let (daemon, environment) = ("daemon".to_string(), "test".to_string());
    let opening = Opening::new(id, daemon, environment, &labels, tab.gzip);
    let attached = Attached {
        attach,
        id,
        traffic: Logged::new(Traffic::local(id, tab, Instant::now())),
        opening,
    };
    attached.bridge(out, frames, shutdown).await;
}

/// Tell the page why its open was refused, when its shape says so.
async fn refuse(refuser: &Refuser, refusal: Refusal, out: &ChannelSender) -> ConsoleEnd {
    if let Told::End(end) = refuser.tell(refusal) {
        if send_command_on(out, 0, "session-end", end).await.is_err() {
            // The socket is gone, and the page with it.
        }
    }
    ConsoleEnd::Refused
}

/// The traffic summary of one console channel, logged once when it is
/// dropped: the kind's task can be stopped at any await when the page closes
/// the channel or the socket ends.
struct Logged {
    traffic: Traffic,
    /// The end this daemon decided, if it decided one.
    end: Option<&'static str>,
}

impl Logged {
    fn new(traffic: Traffic) -> Logged {
        Logged { traffic, end: None }
    }
}

impl Drop for Logged {
    fn drop(&mut self) {
        self.traffic.log(self.end, Instant::now());
    }
}

/// A session attached to a console channel.
struct Attached {
    attach: session::Attachment,
    id: session::SessionId,
    traffic: Logged,
    opening: Opening,
}

impl Attached {
    /// Bridge the channel to the session until a deliberate end, which is
    /// named in `session-end`, or until the socket is gone.
    ///
    /// TEARDOWN INVARIANT: every end drops the attachment, which releases
    /// the writer slot, and never closes the session. A closed channel
    /// detaches; the child survives it and a later reattach resumes it. A
    /// session ends only via `POST /api/sessions/close` or its child
    /// exiting, never because a browser tab closed.
    ///
    /// INPUT IS NEVER HELD BEHIND OUTPUT (ADR-0082 D5): the frames for the
    /// page wait in an [`Outbox`] and go to the socket's writer when it has
    /// room, so the loop keeps reading the page while a slow link takes the
    /// output. The session's output is read only once the outbox is empty,
    /// so a slow page still slows the reading of output.
    async fn bridge(
        mut self,
        out: &ChannelSender,
        frames: &mut mpsc::Receiver<Vec<u8>>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        // Register the eviction waiter BEFORE the first await. One pinned
        // `notified` future serves the whole loop (a fresh `notified()` per
        // turn could miss an eviction firing mid-turn), and `enable()` parks
        // it before the replay: an eviction (a `takeover` reattach or the
        // child exiting) that fires during the replay is delivered as a
        // stored permit and breaks the loop on the first poll. Missing this
        // leaks the writer slot AND hangs the bridge forever (the
        // `Attachment` keeps `tx` alive, so `rx.recv()` never returns
        // `Closed`).
        let evict = self.attach.evict.clone();
        let notified = evict.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let mut outbox = Outbox::on(out.clone());
        open(
            &mut outbox,
            self.id,
            &self.opening,
            &mut self.attach,
            &mut self.traffic.traffic,
        );
        let mut sizes = true;
        let end = loop {
            let flow = tokio::select! {
                _ = shutdown.changed() => {
                    ControlFlow::Break(Some(session::EndReason::DaemonShutdown))
                }
                _ = &mut notified => ControlFlow::Break(Some(
                    evict.reason().unwrap_or(session::EndReason::ChildExited),
                )),
                changed = self.attach.size.changed(), if sizes => {
                    on_size(changed, &mut outbox, self.id, &mut self.attach, &mut sizes)
                }
                recv = self.attach.rx.recv(), if outbox.queued.is_empty() => on_output(
                    recv,
                    &mut outbox,
                    self.id,
                    &self.opening,
                    &mut self.attach,
                    &mut self.traffic.traffic,
                ),
                permit = outbox.out.reserve(), if !outbox.queued.is_empty() => {
                    silent_if_err(hand_on(permit, &mut outbox.queued))
                }
                frame = frames.recv() => self.on_frame(frame),
            };
            if let ControlFlow::Break(end) = flow {
                break end;
            }
        };
        if let Some(reason) = end {
            self.traffic.end = Some(reason.as_wire());
            announce_end(
                &mut outbox,
                self.id,
                &self.opening,
                &mut self.attach,
                &mut self.traffic.traffic,
                reason,
            )
            .await;
        }
    }

    /// One inner frame the page sent on the channel. The inbound queue ends
    /// when the page closes the channel, after the frames it sent before the
    /// close, so the leave its `detach` named is read first.
    fn on_frame(&mut self, frame: Option<Vec<u8>>) -> Flow {
        let Some(bytes) = frame else {
            return ControlFlow::Break(None);
        };
        self.traffic.traffic.inbound(bytes.len());
        on_client_frame(&bytes, &self.attach, &mut self.traffic.traffic)
    }
}
