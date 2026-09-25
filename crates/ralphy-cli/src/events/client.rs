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
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_recv_response(Some(Duration::from_secs(20)))
            .timeout_recv_body(Some(Duration::from_secs(20)))
            // ureq 3 reads HTTPS_PROXY and friends by default; Ralphy never has
            // (#443).
            .proxy(None)
            .build()
            .into();
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
        let mut req = self
            .agent
            .post(&self.url)
            .header("Content-Type", CONTENT_TYPE);
        if let Some(token) = self.token.as_deref().filter(|t| !t.is_empty()) {
            req = req.header("Authorization", &format!("Bearer {token}"));
        }
        match req.send(&bytes[..]) {
            // ureq reports a status >= 400 as `Err(StatusCode)`, so `Ok` is an ack.
            Ok(_) => Ok(PostOutcome::Delivered),
            Err(ureq::Error::StatusCode(code)) => Ok(classify_status(code)),
            // Any other error (timeout, DNS, refused connection) is transient.
            Err(_) => Ok(PostOutcome::Transient),
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

    /// ureq 3 reads a proxy from the environment when an agent is built; Ralphy
    /// never has, and #443 keeps it that way. The default agent proves the
    /// variable is one ureq reads, so the check on ours is not empty.
    #[test]
    fn the_event_agent_ignores_a_proxy_in_the_environment() {
        let saved = std::env::var_os("ALL_PROXY");
        std::env::set_var("ALL_PROXY", "http://127.0.0.1:9");
        let default_uses_it = ureq::Agent::new_with_defaults().config().proxy().is_some();
        let ours_uses_it = UreqEventTransport::new("http://127.0.0.1:1/", None)
            .agent
            .config()
            .proxy()
            .is_some();
        match saved {
            Some(v) => std::env::set_var("ALL_PROXY", v),
            None => std::env::remove_var("ALL_PROXY"),
        }
        assert!(default_uses_it, "ureq no longer reads ALL_PROXY");
        assert!(!ours_uses_it, "the agent picked up ALL_PROXY");
    }

    #[test]
    fn classify_status_splits_4xx_from_5xx() {
        assert_eq!(classify_status(404), PostOutcome::Permanent);
        assert_eq!(classify_status(400), PostOutcome::Permanent);
        assert_eq!(classify_status(499), PostOutcome::Permanent);
        assert_eq!(classify_status(503), PostOutcome::Transient);
        assert_eq!(classify_status(500), PostOutcome::Transient);
    }
}
