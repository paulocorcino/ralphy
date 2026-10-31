//! How a console open says no.
//!
//! Every refused open ends its channel with a `channel.closed` whose reason
//! is `refused`. A NEW launch has no session to retry, so it is told why
//! first, by a `session-end` with `reason: "refused"` and the reason as
//! `message` — the same announcement-before-close rule as every other
//! deliberate end (#334).
//!
//! A REATTACH is not told why: the browser's retry rules count those failed
//! opens, and a refused reattach must stay a failed open, not an announced
//! end. The one exception is a superseded claim (ADR-0079 D7): it is
//! announced on every shape, because a retry would only raise the attempt
//! and evict the socket that replaced it.

use axum::http::StatusCode;

use super::traffic::Tab;

/// The wire value of `session-end`'s `reason` for a launch the daemon refused.
pub(crate) const REFUSED: &str = "refused";

/// The wire value of `session-end`'s `reason` for a late claim of a tab whose
/// newer socket already holds the writer slot (ADR-0079 D7).
pub(crate) const SUPERSEDED: &str = "superseded";

/// A console open the daemon refused, as it was decided: the channel tells
/// its client what [`Refuser::tell`] decides.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Refuse with `message`. `status` is logged; a new launch announces
    /// `message` too.
    Refuse { status: StatusCode, message: String },
    /// A claim whose holder already holds the slot with a higher attempt.
    Supersede { attempt: Option<u64> },
}

impl Refusal {
    pub(crate) fn refuse(status: StatusCode, message: impl Into<String>) -> Refusal {
        Refusal::Refuse {
            status,
            message: message.into(),
        }
    }
}

/// How the client of a refused open is told: by the channel's close alone
/// for a reattach, or first by a `session-end` with this payload.
#[derive(Debug)]
pub(crate) enum Told {
    Close,
    End(serde_json::Value),
}

/// Refuses one console open in the way its client can read, and logs each
/// refusal: a console that does not open is the failure the operator sees.
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

    /// Log the decided `refusal`, and decide how its client is told.
    pub(crate) fn tell(&self, refusal: Refusal) -> Told {
        match refusal {
            Refusal::Refuse { status, message } => self.refuse(status, message),
            Refusal::Supersede { attempt } => self.supersede(attempt),
        }
    }

    /// Refuse with `message`. A reattach is told nothing more; a new launch
    /// announces `message`.
    fn refuse(&self, status: StatusCode, message: String) -> Told {
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
            return Told::Close;
        }
        Told::End(self.end(REFUSED, message))
    }

    /// Refuse a claim whose holder already holds the slot with a higher
    /// attempt, on a reattach and on a launch alike. Nobody is evicted.
    fn supersede(&self, attempt: Option<u64>) -> Told {
        tracing::info!(
            reason = SUPERSEDED,
            launch = self.session.is_none(),
            session = self.session,
            holder = self.tab.holder.as_deref(),
            attempt,
            device = self.tab.device.map(|d| d.to_string()),
            "console socket refused"
        );
        Told::End(self.end(
            SUPERSEDED,
            "a newer connection from this tab has the console".to_string(),
        ))
    }

    /// The payload of the `session-end` that names a refusal.
    fn end(&self, reason: &'static str, message: String) -> serde_json::Value {
        serde_json::json!({
            "reason": reason,
            "message": message,
            "daemon_id": self.daemon_id,
            "environment": self.environment,
        })
    }
}
