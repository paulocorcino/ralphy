//! The HTTP transport for the CloudEvents sink (ADR-0019).
//!
//! Delivery sits behind an [`EventSink`] trait so the worker's retry/drop policy is
//! testable without a network: [`UreqEventTransport`] is the real blocking `ureq`
//! POST, and the sink tests inject a fake that returns canned [`PostOutcome`]s.
//! The transport classifies the endpoint's response into the three outcomes the
//! contract defines: any `2xx` is [`PostOutcome::Delivered`]; a `4xx` is a
//! configuration error dropped without retry ([`PostOutcome::Permanent`]); a `5xx`,
//! timeout, or network error is retried ([`PostOutcome::Transient`]).

use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

/// The structured content type every event is POSTed as (CloudEvents 1.0).
const CONTENT_TYPE: &str = "application/cloudevents+json";

/// How the endpoint (or the network) responded to one POST, mapped to the
/// retry/drop policy the sink worker applies (docs/events.md transport contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostOutcome {
    /// A `2xx` — the event is acknowledged (body ignored).
    Delivered,
    /// A `5xx`, timeout, or network error — retry with backoff, then drop.
    Transient,
    /// A `4xx` — a configuration error; drop without retry.
    Permanent,
}

/// The delivery transport the sink worker POSTs each envelope through. One call
/// per event; the returned [`PostOutcome`] drives retry vs. drop.
pub trait EventSink {
    /// POST one CloudEvents envelope, classifying the response.
    fn post(&self, body: &Value) -> Result<PostOutcome>;
}

/// The real blocking transport: POST each envelope to the configured `url` with
/// `ureq`, carrying `Content-Type: application/cloudevents+json` and, when a token
/// is configured, `Authorization: Bearer <token>`.
pub struct UreqEventTransport {
    url: String,
    token: Option<String>,
    agent: ureq::Agent,
}

impl UreqEventTransport {
    /// Build a transport for `url` with an optional bearer `token`. The agent
    /// carries connect/read timeouts so a wedged endpoint classifies as a
    /// [`PostOutcome::Transient`] instead of hanging the worker thread.
    pub fn new(url: impl Into<String>, token: Option<String>) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(20))
            .build();
        Self {
            url: url.into(),
            token,
            agent,
        }
    }
}

impl EventSink for UreqEventTransport {
    fn post(&self, body: &Value) -> Result<PostOutcome> {
        // Serialize once and POST as raw bytes with an explicit content type, so
        // the CloudEvents media type is never overridden by `send_json`'s default.
        let bytes = serde_json::to_vec(body)?;
        let mut req = self.agent.post(&self.url).set("Content-Type", CONTENT_TYPE);
        if let Some(token) = self.token.as_deref().filter(|t| !t.is_empty()) {
            req = req.set("Authorization", &format!("Bearer {token}"));
        }
        match req.send_bytes(&bytes) {
            // ureq treats any non-2xx as `Err(Status)`, so `Ok` is a 2xx ack.
            Ok(_) => Ok(PostOutcome::Delivered),
            Err(ureq::Error::Status(code, _)) => Ok(classify_status(code)),
            // A transport error (timeout, DNS, refused connection) is transient.
            Err(ureq::Error::Transport(_)) => Ok(PostOutcome::Transient),
        }
    }
}

/// Classify an HTTP status code into a [`PostOutcome`]: a `4xx` is a permanent
/// configuration error (drop), everything else (`5xx` and any other non-2xx) is
/// transient (retry). `2xx` never reaches here — `ureq` reports it as `Ok`.
pub fn classify_status(code: u16) -> PostOutcome {
    if (400..500).contains(&code) {
        PostOutcome::Permanent
    } else {
        PostOutcome::Transient
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_status_splits_4xx_from_5xx() {
        assert_eq!(classify_status(404), PostOutcome::Permanent);
        assert_eq!(classify_status(400), PostOutcome::Permanent);
        assert_eq!(classify_status(499), PostOutcome::Permanent);
        assert_eq!(classify_status(503), PostOutcome::Transient);
        assert_eq!(classify_status(500), PostOutcome::Transient);
    }
}
