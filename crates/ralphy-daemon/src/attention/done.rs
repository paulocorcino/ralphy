//! The done mark (docs/adr/0085 D11): the operator turns on "tell me when it
//! is done" for one console, and the next turn that console finishes opens
//! one `console-done` request. The mark is `DeskRecord::done_mark`, so every
//! device shows it; the daemon clears it when the request opens, so the
//! request is asked for once.
//!
//! The finished turns come from the console's hook tail
//! (`SessionManager::turn_ends`): a `done` caused by a turn that ended,
//! never by a session that started. The attention tick keeps what it already
//! looked at in [`Marks`].

use std::collections::HashMap;

use tokio::sync::broadcast;

use super::{AttentionRequest, Kind};
use crate::desk;
use crate::routes::{desk_put_route, PeerRepoCache, Push};
use crate::session::{SessionId, SessionInfo, TurnEnd};

/// A finished turn of a console that had the mark: one open request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoneFired {
    pub session: SessionId,
    /// The console's count of finished turns at that turn. It makes the id
    /// of the request, so a console asked for twice has two requests, and a
    /// dismissal of the first hides only the first.
    pub count: u64,
    /// The hook time of the turn's end.
    pub since: String,
}

/// What the tick keeps of the done marks between ticks.
#[derive(Debug, Default)]
pub(crate) struct Marks {
    /// The count of finished turns the tick already looked at, per console.
    handled: HashMap<SessionId, u64>,
    open: Vec<DoneFired>,
}

impl Marks {
    /// The requests that are open now.
    pub(crate) fn open(&self) -> &[DoneFired] {
        &self.open
    }

    /// The finished turns in `ends` the tick did not look at before. A console
    /// that left takes its handled count and its requests with it.
    pub(crate) fn fresh(&mut self, ends: &[TurnEnd]) -> Vec<TurnEnd> {
        let alive = |id: &SessionId| ends.iter().any(|e| e.id == *id);
        self.handled.retain(|id, _| alive(id));
        self.open.retain(|fired| alive(&fired.session));
        let mut fresh = Vec::new();
        for end in ends {
            let handled = self.handled.entry(end.id).or_default();
            if end.count > *handled {
                *handled = end.count;
                fresh.push(end.clone());
            }
        }
        fresh
    }

    fn fire(&mut self, end: &TurnEnd) {
        self.open.push(DoneFired {
            session: end.id,
            count: end.count,
            since: end.since.clone(),
        });
    }
}

/// Where the tick finds the desk and writes it back.
pub(crate) struct DeskFiles {
    pub(crate) desk: std::path::PathBuf,
    pub(crate) registry: std::path::PathBuf,
    pub(crate) peers: PeerRepoCache,
}

/// Open a request for each of the `fresh` turn ends whose window record has
/// the mark, then clear those marks through the desk's own write path, which
/// pushes `desk.dirty`. A request is opened even when the mark cannot be
/// cleared: the operator asked for it.
pub(crate) async fn fire(
    marks: &mut Marks,
    fresh: &[TurnEnd],
    files: &DeskFiles,
    pushes: &broadcast::Sender<Push>,
) {
    let stored = match desk::load_from(&files.desk) {
        Ok(stored) => stored,
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "the done marks could not be read from the desk");
            return;
        }
    };
    let marked = |record: &str| stored.windows.iter().any(|w| w.id == record && w.done_mark);
    let mut cleared = Vec::new();
    for end in fresh {
        let Some(record) = end.record.as_deref().filter(|r| marked(r)) else {
            continue;
        };
        marks.fire(end);
        cleared.push(record.to_string());
    }
    if !cleared.is_empty() {
        clear(&cleared, stored.generation, files, pushes).await;
    }
}

/// Set `doneMark: false` on each record in `records`, as one desk upload.
async fn clear(
    records: &[String],
    generation: u64,
    files: &DeskFiles,
    pushes: &broadcast::Sender<Push>,
) {
    let changes: Vec<_> = records
        .iter()
        .map(|id| {
            serde_json::json!({
                "op": "set", "type": "window", "id": id, "fields": { "doneMark": false }
            })
        })
        .collect();
    let body = serde_json::json!({ "seq": 0, "generation": generation, "changes": changes });
    let raw = body.to_string();
    let reply = desk_put_route(
        files.desk.clone(),
        files.registry.clone(),
        files.peers.clone(),
        pushes.clone(),
        None,
        raw.as_bytes(),
    )
    .await;
    if !reply.status().is_success() {
        tracing::warn!(status = %reply.status(), "the done mark could not be cleared");
    }
}

/// One `console-done` request per open [`DoneFired`] whose console still
/// runs: the vendor and the project come from the console.
pub fn requests<'a>(
    sessions: &'a [SessionInfo],
    open: &'a [DoneFired],
) -> impl Iterator<Item = AttentionRequest> + 'a {
    open.iter().filter_map(move |fired| {
        let info = sessions.iter().find(|s| s.id == fired.session)?;
        let kind = Kind::ConsoleDone;
        Some(AttentionRequest {
            id: format!("done:{}:{}", fired.session, fired.count),
            kind,
            vendor: info.agent.clone(),
            console: Some(info.id.to_string()),
            repo: Some(info.repo.clone()),
            run: None,
            issue: None,
            since: fired.since.clone(),
            detail: None,
            dismissable: kind.dismissable(),
        })
    })
}
