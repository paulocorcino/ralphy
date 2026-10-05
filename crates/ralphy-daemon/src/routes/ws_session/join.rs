//! A NEW launch that names its window record joins the live session already
//! serving that record, instead of starting a second one (ADR-0050 amendment
//! 2026-10-04).

use std::sync::Arc;

use axum::extract::ws::WebSocketUpgrade;
use axum::response::Response;

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
/// there, so the tab that launched it can reclaim it later.
pub(super) fn hold(att: &session::Attachment, holder: Option<&str>) {
    if let Some(holder) = holder {
        att.hold_as(holder);
    }
}

/// A launch that joined the live session already serving its record, instead
/// of starting a second one (ADR-0050 amendment 2026-10-04).
pub(super) struct Joined {
    id: session::SessionId,
    att: session::Attachment,
    labels: SessionLabels,
    environment: String,
}

impl Joined {
    /// Join the session `claim` found: as the writer when the slot is free or
    /// is this holder's, else as a watcher. `None` when there is none, or it
    /// ended meanwhile, so the launch goes on and spawns.
    pub(super) fn find(
        sessions: &Arc<session::SessionManager>,
        claim: Option<&session::RecordClaim>,
        holder: Option<&str>,
        environment: &str,
    ) -> Option<Joined> {
        let id = claim?.live()?;
        let info = sessions.get(id)?;
        let (att, watching) = match sessions.attach_as(id, false, holder) {
            Ok(att) => (att, false),
            Err(session::AttachError::Busy) => (sessions.watch(id).ok()?, true),
            Err(session::AttachError::Unknown) => return None,
        };
        Some(Joined {
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
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Response {
        let Joined {
            id,
            att,
            labels,
            environment,
        } = self;
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
