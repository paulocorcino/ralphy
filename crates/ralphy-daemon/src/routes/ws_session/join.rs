//! A NEW launch that names its window record joins the live session already
//! serving that record, instead of starting a second one (ADR-0050 amendment
//! 2026-10-04).

use std::sync::Arc;

use axum::extract::ws::WebSocketUpgrade;
use axum::response::Response;

use super::refuse::Refuser;
use super::traffic::Tab;
use super::{session_ws, SessionLabels};
use crate::session;

/// The claim a NEW launch holds on its record from the check to the insert,
/// or `None` for a launch that named no record.
pub(super) async fn claim_for(
    sessions: &Arc<session::SessionManager>,
    record: &Option<String>,
) -> Option<session::RecordClaim> {
    match record {
        Some(record) => Some(sessions.claim_record(record).await),
        None => None,
    }
}

/// A fresh launch claims its writer slot inside the spawn; name its holder
/// and the attempt of its claim there, so the tab that launched it can
/// reclaim it later.
pub(super) fn hold(att: &session::Attachment, holder: Option<&str>, attempt: Option<u64>) {
    if let Some(holder) = holder {
        att.hold_as(holder, attempt);
    }
}

/// A launch that joined the live session already serving its record, instead
/// of starting a second one (ADR-0050 amendment 2026-10-04).
pub(super) enum Joined {
    Attached {
        id: session::SessionId,
        att: session::Attachment,
        labels: SessionLabels,
        environment: String,
    },
    /// This holder's newer socket already writes to the session. Not a
    /// watcher: it would watch its own session (ADR-0079 D7).
    Superseded { attempt: Option<u64> },
}

impl Joined {
    /// Join the session `claim` found: as the writer when the slot is free or
    /// is this holder's, else as a watcher. `None` when there is none, or it
    /// ended meanwhile, so the launch goes on and spawns.
    pub(super) fn find(
        sessions: &Arc<session::SessionManager>,
        claim: Option<&session::RecordClaim>,
        holder: Option<&str>,
        attempt: Option<u64>,
        environment: &str,
    ) -> Option<Joined> {
        let id = claim?.live()?;
        let info = sessions.get(id)?;
        // A launch opens a new terminal, so it has no byte count to resume at.
        let (att, watching) = match sessions.attach_as(id, false, holder, attempt, None) {
            Ok(att) => (att, false),
            Err(session::AttachError::Busy) => (sessions.watch(id, None).ok()?, true),
            Err(session::AttachError::Superseded) => return Some(Joined::Superseded { attempt }),
            Err(session::AttachError::Unknown) => return None,
        };
        Some(Joined::Attached {
            id,
            att,
            labels: SessionLabels {
                name: info.name,
                checkout: info.checkout,
                watching,
            },
            environment: info.environment.unwrap_or_else(|| environment.to_string()),
        })
    }

    pub(super) fn upgrade(
        self,
        ws: WebSocketUpgrade,
        daemon_id: String,
        tab: Tab,
        refuser: &Refuser,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Response {
        let (id, att, labels, environment) = match self {
            Joined::Attached {
                id,
                att,
                labels,
                environment,
            } => (id, att, labels, environment),
            Joined::Superseded { attempt } => return refuser.supersede(ws, attempt),
        };
        ws.on_upgrade(move |socket| {
            session_ws(
                socket,
                att,
                id,
                daemon_id,
                environment,
                labels,
                tab,
                shutdown,
            )
        })
    }
}
