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

use std::time::Duration;

use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::routes::send_command;

/// The wire value of `session-end`'s `reason` for a launch the daemon refused.
pub(crate) const REFUSED: &str = "refused";

/// Refuses one `/ws/session` request in the way its client can read.
#[derive(Clone)]
pub(crate) struct Refuser {
    new_launch: bool,
    daemon_id: String,
    environment: String,
}

impl Refuser {
    pub(crate) fn new(new_launch: bool, daemon_id: &str, environment: &str) -> Self {
        Self {
            new_launch,
            daemon_id: daemon_id.to_string(),
            environment: environment.to_string(),
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
        if !self.new_launch {
            return (status, message).into_response();
        }
        let daemon_id = self.daemon_id.clone();
        let environment = self.environment.clone();
        ws.on_upgrade(move |mut socket| async move {
            // Bounded like the bridge's own end: a browser that does not read
            // must not hold this task.
            let announce = async {
                send_command(
                    &mut socket,
                    0,
                    "session-end",
                    serde_json::json!({
                        "reason": REFUSED,
                        "message": message,
                        "daemon_id": daemon_id,
                        "environment": environment,
                    }),
                )
                .await;
                socket.send(Message::Close(None)).await
            };
            match tokio::time::timeout(Duration::from_secs(5), announce).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::debug!(error = %e, "the browser left before the refusal was closed");
                }
                Err(_) => tracing::debug!("the browser did not read the refusal within 5 s"),
            }
        })
    }
}
