//! The attention tick (docs/adr/0085 D10, D11): every [`EVERY`] it reads the
//! live run snapshots of each registered repo, works out the attention list,
//! and pushes `attention.dirty` when the list changed. A run is another
//! process that writes only files, so only a read of the files shows its
//! change; a console's agent state also changes by the clock alone (a
//! console goes silent with no event), and the same tick sees that.
//!
//! The route reads the runs this tick read last, so the list it serves and
//! the list the push announced are the same list.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use tokio::sync::broadcast;

use super::{list, AttentionRequest, RunSource};
use crate::registry;
use crate::routes::{push, Push};
use crate::session::SessionManager;

/// How often the tick reads the runs.
pub const EVERY: Duration = Duration::from_secs(3);

/// The run snapshots of the last tick, shared with the route.
pub(crate) type Runs = Arc<Mutex<Vec<RunSource>>>;

/// The live runs of every registered repo, in key order. A repo that is not
/// reachable has none; an unreadable registry is logged and has none.
fn read_runs(registry_path: &Path) -> Vec<RunSource> {
    let store = match registry::load_from(registry_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(error = %e, "the attention tick could not read the repo registry");
            return Vec::new();
        }
    };
    let mut runs = Vec::new();
    for (slug, entry) in store.repos.iter().filter(|(_, e)| e.reachable()) {
        let listing =
            ralphy_run_snapshot::list_runs(Path::new(&entry.path), ralphy_proc_util::pid_is_alive);
        runs.extend(listing.live.into_iter().map(|snapshot| RunSource {
            repo: slug.clone(),
            snapshot,
        }));
    }
    runs
}

/// What the last tick saw: the id and the detail of each request.
#[derive(Debug, Default)]
pub(crate) struct Seen {
    last: Vec<(String, Option<String>)>,
}

impl Seen {
    /// Whether the ids or a detail of `requests` differ from the last list
    /// seen, which `requests` then becomes. A request's age is not a change.
    fn changed(&mut self, requests: &[AttentionRequest]) -> bool {
        let next: Vec<(String, Option<String>)> = requests
            .iter()
            .map(|r| (r.id.clone(), r.detail.clone()))
            .collect();
        let changed = next != self.last;
        self.last = next;
        changed
    }
}

/// One tick: read the runs, keep them for the route, and push
/// `attention.dirty` when the list changed.
pub(crate) async fn tick_once(
    registry_path: &Path,
    sessions: &SessionManager,
    runs: &Runs,
    seen: &mut Seen,
    pushes: &broadcast::Sender<Push>,
) {
    let path = registry_path.to_path_buf();
    let read = match tokio::task::spawn_blocking(move || read_runs(&path)).await {
        Ok(read) => read,
        Err(e) => {
            tracing::warn!(error = %e, "the attention tick could not read the runs");
            return;
        }
    };
    let requests = list(&sessions.list_at(SystemTime::now()), &read);
    match runs.lock() {
        Ok(mut kept) => *kept = read,
        Err(e) => tracing::warn!(error = %e, "the attention runs are poisoned"),
    }
    if seen.changed(&requests) {
        push(pushes, Push::Attention);
    }
}

/// Tick every [`EVERY`] until the daemon stops.
pub(crate) async fn run(
    registry_path: std::path::PathBuf,
    sessions: Arc<SessionManager>,
    runs: Runs,
    pushes: broadcast::Sender<Push>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut seen = Seen::default();
    let mut every = tokio::time::interval(EVERY);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = every.tick() => {
                tick_once(&registry_path, &sessions, &runs, &mut seen, &pushes).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralphy_run_snapshot::{AgentBlock, RunSnapshot};

    fn snapshot(detail: &str) -> RunSnapshot {
        let mut snap = RunSnapshot {
            runid: "01TICK".to_string(),
            pid: std::process::id(),
            exec_agent: "claude".to_string(),
            ..RunSnapshot::default()
        };
        snap.phase.state = "executing".to_string();
        snap.phase.agent = Some(AgentBlock {
            state: "waiting".to_string(),
            since: "2026-10-28T11:00:00-03:00".to_string(),
            detail: Some(detail.to_string()),
        });
        snap
    }

    /// Whether the tick sent one push, and only one.
    fn pushed(rx: &mut broadcast::Receiver<Push>) -> bool {
        match rx.try_recv() {
            Ok(p) => {
                assert_eq!(p, Push::Attention);
                assert!(rx.try_recv().is_err(), "one push per tick");
                true
            }
            Err(_) => false,
        }
    }

    #[tokio::test]
    async fn attention_push_fires_only_on_a_change() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("create the repo dir");
        let registry_path = dir.path().join("repos.toml");
        let mut reg = registry::RegistryStore::default();
        reg.upsert("owner/tick", &repo.to_string_lossy());
        registry::save_to(&reg, &registry_path).expect("save the registry");
        let sessions = SessionManager::new();
        let runs = Runs::default();
        let mut seen = Seen::default();
        let (tx, mut rx) = broadcast::channel(8);
        macro_rules! tick {
            () => {
                tick_once(&registry_path, &sessions, &runs, &mut seen, &tx)
            };
        }

        tick!().await;
        assert!(!pushed(&mut rx), "an empty list at the start is no change");

        ralphy_run_snapshot::write_atomic(&repo, &snapshot("permission: Bash")).expect("write");
        tick!().await;
        assert!(pushed(&mut rx), "a run that waits is a change");
        tick!().await;
        assert!(!pushed(&mut rx), "the same list is no change");

        ralphy_run_snapshot::write_atomic(&repo, &snapshot("question: Which?")).expect("write");
        tick!().await;
        assert!(pushed(&mut rx), "a new detail is a change");

        std::fs::remove_file(ralphy_run_snapshot::snapshot_path(&repo, "01TICK"))
            .expect("remove the snapshot");
        tick!().await;
        assert!(pushed(&mut rx), "a run that left is a change");
        tick!().await;
        assert!(!pushed(&mut rx), "the same empty list is no change");
    }
}
