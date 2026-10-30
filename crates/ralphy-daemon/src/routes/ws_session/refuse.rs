//! How `/ws/session` says no.
//!
//! A browser `WebSocket` cannot read the status or the body of a refused
//! upgrade: it sees only a close. A NEW launch has no session to retry, so the
//! console used to print `[session closed]` and the reason was lost. A new
//! launch is therefore refused AFTER the upgrade, by a `session-end` frame with
//! `reason: "refused"` and the reason as `message`, then a Close — the same
//! announcement-before-close rule as every other deliberate end (#334).
//!
//! A REATTACH keeps its HTTP status: the browser's retry rules count those
//! failed opens, and a `404`/`409` must stay a failed open, not an announced end.
//! The one exception is a superseded claim (ADR-0079 D7): it is announced on
//! every shape, because a retry would only raise the attempt and evict the
//! socket that replaced it.

use std::time::Duration;

use axum::extract::ws::WebSocketUpgrade;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;

use super::traffic::Tab;
use crate::mux::{Mode, Out, Writer, WriterClosed};
use crate::routes::send_command_on;

/// The wire value of `session-end`'s `reason` for a launch the daemon refused.
pub(crate) const REFUSED: &str = "refused";

/// The wire value of `session-end`'s `reason` for a late claim of a tab whose
/// newer socket already holds the writer slot (ADR-0079 D7).
pub(crate) const SUPERSEDED: &str = "superseded";

/// Refuses one `/ws/session` request in the way its client can read, and
/// logs each refusal: a console that does not open is the failure the
/// operator sees.
#[derive(Clone)]
pub(crate) struct Refuser {
    /// The session a reattach names; `None` for a new launch.
    session: Option<u64>,
    daemon_id: String,
    environment: String,
    tab: Tab,
}

impl Refuser {
    pub(crate) fn new(session: Option<u64>, daemon_id: &str, environment: &str, tab: Tab) -> Self {
        Self {
            session,
            daemon_id: daemon_id.to_string(),
            environment: environment.to_string(),
            tab,
        }
    }

    /// Refuse with `message`. `status` is what a reattach answers; a new
    /// launch upgrades and announces `message` instead.
    pub(crate) fn refuse(
        &self,
        ws: WebSocketUpgrade,
        status: StatusCode,
        message: impl Into<String>,
    ) -> Response {
        let message = message.into();
        tracing::info!(
            status = status.as_u16(),
            reason = message.as_str(),
            launch = self.session.is_none(),
            session = self.session,
            holder = self.tab.holder.as_deref(),
            device = self.tab.device.map(|d| d.to_string()),
            "console socket refused"
        );
        if self.session.is_some() {
            return (status, message).into_response();
        }
        self.announce(ws, REFUSED, message)
    }

    /// Refuse a claim whose holder already holds the slot with a higher
    /// attempt, on a reattach and on a launch alike. Nobody is evicted.
    pub(crate) fn supersede(&self, ws: WebSocketUpgrade, attempt: Option<u64>) -> Response {
        tracing::info!(
            reason = SUPERSEDED,
            launch = self.session.is_none(),
            session = self.session,
            holder = self.tab.holder.as_deref(),
            attempt,
            device = self.tab.device.map(|d| d.to_string()),
            "console socket refused"
        );
        self.announce(
            ws,
            SUPERSEDED,
            "a newer connection from this tab has the console".to_string(),
        )
    }

    /// Upgrade, send `session-end` with `reason` and `message`, then Close.
    fn announce(&self, ws: WebSocketUpgrade, reason: &'static str, message: String) -> Response {
        let daemon_id = self.daemon_id.clone();
        let environment = self.environment.clone();
        ws.on_upgrade(move |socket| async move {
            // The read half is kept, unread, until the writer is stopped.
            let (sink, _from_browser) = socket.split();
            let writer = Writer::start(sink, Mode::Plain);
            let out = writer.channel(0);
            // Bounded like the bridge's own end: a browser that does not read
            // must not hold this task.
            let announce = async {
                let end = serde_json::json!({
                    "reason": reason,
                    "message": message,
                    "daemon_id": daemon_id,
                    "environment": environment,
                });
                send_command_on(&out, 0, "session-end", end).await?;
                out.put(Out::Close).await?;
                writer.ended().await;
                Ok(())
            };
            match tokio::time::timeout(Duration::from_secs(5), announce).await {
                Ok(Ok(())) => {}
                Ok(Err(WriterClosed)) => {
                    tracing::debug!("the browser left before the refusal was closed");
                }
                Err(_) => tracing::debug!("the browser did not read the refusal within 5 s"),
            }
            writer.abort();
        })
    }
}
