//! The byte bridge of an attached console: replay, live stream, input,
//! resize, and the eviction/exit teardown. [`Liveness`] is the page
//! socket's.

use std::ops::ControlFlow;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::broadcast::error::RecvError;

// ADR-0082 D4: a console served on a channel of the page socket.
mod channel;

pub(crate) use channel::{serve_console, ConsoleEnd, PeerRelay};

use super::outbound::{resync, send_output, send_size, Opening, Outbox};
use super::traffic::Traffic;
use crate::protocol::{Command, Frame};
use crate::{protocol, session};

/// How often the page socket pings its browser, and how long the browser may
/// stay silent before the socket gives up on it (ADR-0051 §9 amendment
/// 2026-09-22).
///
/// The browser answers every ping with a pong from its network stack, even
/// in a background tab, so a browser that has sent NOTHING for
/// `silent_after` is gone. A send succeeding proves nothing: a tunnel agent
/// (measured: TunnelDeck for dev tunnels, 2026-09-22) keeps its leg to the
/// daemon open for minutes after the browser behind it vanished, and that
/// leg would hold a console's writer slot against the page's own reattach.
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
    /// client is released 45 s after the last thing it sent.
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
