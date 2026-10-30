//! A NEW launch that names its window record joins the live session already
//! serving that record, instead of starting a second one (ADR-0050 amendment
//! 2026-10-04).

use std::sync::Arc;

use super::refuse::Refusal;
use super::upgrade::Opened;
use super::SessionLabels;
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

/// Join the session `claim` found: as the writer when the slot is free or is
/// this holder's, else as a watcher. A claim this holder's newer socket
/// already holds is refused as superseded: as a watcher it would watch its
/// own session (ADR-0079 D7). `None` when there is no session, or it ended
/// meanwhile, so the launch goes on and spawns.
pub(super) fn join(
    sessions: &Arc<session::SessionManager>,
    claim: Option<&session::RecordClaim>,
    holder: Option<&str>,
    attempt: Option<u64>,
    environment: &str,
) -> Option<Opened> {
    let id = claim?.live()?;
    let info = sessions.get(id)?;
    // A launch opens a new terminal, so it has no byte count to resume at.
    let (att, watching) = match sessions.attach_as(id, false, holder, attempt, None) {
        Ok(att) => (att, false),
        Err(session::AttachError::Busy) => (sessions.watch(id, None).ok()?, true),
        Err(session::AttachError::Superseded) => {
            return Some(Opened::Refused(Refusal::Supersede { attempt }))
        }
        Err(session::AttachError::Unknown) => return None,
    };
    Some(Opened::Session {
        att,
        id,
        labels: SessionLabels {
            name: info.name,
            checkout: info.checkout,
            watching,
        },
        environment: info.environment.unwrap_or_else(|| environment.to_string()),
    })
}
