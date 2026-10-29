//! The attention tick (docs/adr/0085 D10, D11): every [`EVERY`] it reads the
//! live run snapshots of each registered repo and the final snapshots in its
//! run folders, works out the attention list,
//! and pushes `attention.dirty` when the list changed. A run is another
//! process that writes only files, so only a read of the files shows its
//! change; a console's agent state also changes by the clock alone (a
//! console goes silent with no event), and the same tick sees that.
//!
//! The tick also looks at the turns the consoles finished, and opens a
//! request for the ones the operator marked ([`done`]).
//!
//! The route reads the runs this tick read last, so the list it serves and
//! the list the push announced are the same list.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use tokio::sync::broadcast;

use super::dismissals::Dismissals;
use super::done::{self, DeskFiles, Marks};
use super::{list, AttentionRequest, FinishedRun, RunSource, RunsRead};
use crate::registry;
use crate::routes::{push, Push};
use crate::session::SessionManager;

/// How often the tick reads the runs.
pub const EVERY: Duration = Duration::from_secs(3);

/// The run snapshots of the last tick, shared with the route.
pub(crate) type Runs = Arc<Mutex<RunsRead>>;

/// The dismissals, shared by the tick and the route.
pub(crate) type Dismissed = Arc<Mutex<Dismissals>>;

/// The live and the finished runs of every registered repo, in key order. A
/// repo that is not reachable has none; an unreadable registry is logged and
/// has none.
fn read_runs(registry_path: &Path) -> RunsRead {
    let store = match registry::load_from(registry_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(error = %e, "the attention tick could not read the repo registry");
            return RunsRead::default();
        }
    };
    let mut runs = RunsRead::default();
    for (slug, entry) in store.repos.iter().filter(|(_, e)| e.reachable()) {
        let root = Path::new(&entry.path);
        let listing = ralphy_run_snapshot::list_runs(root, ralphy_proc_util::pid_is_alive);
        runs.live
            .extend(listing.live.into_iter().map(|snapshot| RunSource {
                repo: slug.clone(),
                snapshot,
            }));
        runs.finished.extend(read_finished(root, slug));
    }
    runs
}

/// The final snapshots in the run folders of the repo at `root`, in folder
/// name order. `<repo>/.ralphy/runs/` is the run folders' directory
/// (`Workspace::runs_dir` in ralphy-core, which the daemon does not import).
/// A folder with no final snapshot this build reads is not a finished run.
fn read_finished(root: &Path, slug: &str) -> Vec<FinishedRun> {
    let Ok(entries) = std::fs::read_dir(root.join(".ralphy").join("runs")) else {
        return Vec::new();
    };
    let mut finished: Vec<FinishedRun> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let dir = entry.path();
            let snapshot = ralphy_run_snapshot::read_final(&dir)?;
            let stamp = entry.file_name().into_string().ok()?;
            Some(FinishedRun {
                repo: slug.to_string(),
                stamp,
                ended: ended_at(&dir).unwrap_or_else(|| snapshot.started_at.clone()),
                snapshot,
            })
        })
        .collect();
    finished.sort_by(|a, b| a.stamp.cmp(&b.stamp));
    finished
}

/// When the run in `run_dir` ended: the time its final snapshot was written.
fn ended_at(run_dir: &Path) -> Option<String> {
    let modified = std::fs::metadata(ralphy_run_snapshot::final_snapshot_path(run_dir))
        .and_then(|m| m.modified())
        .ok()?;
    Some(
        chrono::DateTime::<chrono::Utc>::from(modified)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
}

/// The list of `sessions` now, the runs `runs` and the `dismissed` ids. A
/// poisoned dismissal lock is logged and dismisses nothing.
pub(crate) fn list_with(
    sessions: &SessionManager,
    runs: &RunsRead,
    dismissed: &Dismissed,
) -> Vec<AttentionRequest> {
    let consoles = sessions.list_at(SystemTime::now());
    match dismissed.lock() {
        Ok(dismissed) => list(&consoles, runs, &|id| dismissed.contains(id)),
        Err(e) => {
            tracing::warn!(error = %e, "the attention dismissals are poisoned");
            list(&consoles, runs, &|_| false)
        }
    }
}

/// What the last tick saw: the id and the detail of each request, and the
/// done marks it looked at.
#[derive(Default)]
pub(crate) struct Seen {
    last: Vec<(String, Option<String>)>,
    marks: Marks,
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

/// One tick: read the runs, open the requests of the finished turns the
/// operator marked, keep both for the route, and push `attention.dirty` when
/// the list changed.
pub(crate) async fn tick_once(
    files: &DeskFiles,
    sessions: &SessionManager,
    (runs, dismissed): (&Runs, &Dismissed),
    seen: &mut Seen,
    pushes: &broadcast::Sender<Push>,
) {
    let path = files.registry.clone();
    let mut read = match tokio::task::spawn_blocking(move || read_runs(&path)).await {
        Ok(read) => read,
        Err(e) => {
            tracing::warn!(error = %e, "the attention tick could not read the runs");
            return;
        }
    };
    let fresh = seen.marks.fresh(&sessions.turn_ends());
    if !fresh.is_empty() {
        done::fire(&mut seen.marks, &fresh, files, pushes).await;
    }
    read.done = seen.marks.open().to_vec();
    let requests = list_with(sessions, &read, dismissed);
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
    files: DeskFiles,
    sessions: Arc<SessionManager>,
    (runs, dismissed): (Runs, Dismissed),
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
                tick_once(&files, &sessions, (&runs, &dismissed), &mut seen, &pushes).await;
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

    /// The files the tick reads: the registry, and a desk that is not there.
    fn files_of(dir: &tempfile::TempDir, registry_path: &std::path::Path) -> DeskFiles {
        DeskFiles {
            desk: dir.path().join("desk.toml"),
            registry: registry_path.to_path_buf(),
            peers: Default::default(),
        }
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
        let files = files_of(&dir, &registry_path);
        let sessions = SessionManager::new();
        let runs = Runs::default();
        let dismissed = Dismissed::default();
        let mut seen = Seen::default();
        let (tx, mut rx) = broadcast::channel(8);
        macro_rules! tick {
            () => {
                tick_once(&files, &sessions, (&runs, &dismissed), &mut seen, &tx)
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

    #[tokio::test]
    async fn a_removed_run_folder_ends_the_request() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let repo = dir.path().join("repo");
        let run_dir = repo.join(".ralphy").join("runs").join("20261028-100000");
        std::fs::create_dir_all(&run_dir).expect("create the run folder");
        let registry_path = dir.path().join("repos.toml");
        let mut reg = registry::RegistryStore::default();
        reg.upsert("owner/tick", &repo.to_string_lossy());
        registry::save_to(&reg, &registry_path).expect("save the registry");
        let mut done = snapshot("unused");
        done.phase.agent = None;
        done.issues = vec![ralphy_run_snapshot::IssueBlock {
            number: 71,
            status: "blocked".to_string(),
            ..ralphy_run_snapshot::IssueBlock::default()
        }];
        std::fs::write(
            ralphy_run_snapshot::final_snapshot_path(&run_dir),
            serde_json::to_vec(&done).expect("a snapshot serializes"),
        )
        .expect("write the final snapshot");
        let files = files_of(&dir, &registry_path);
        let sessions = SessionManager::new();
        let runs = Runs::default();
        let dismissed = Dismissed::default();
        let mut seen = Seen::default();
        let (tx, mut rx) = broadcast::channel(8);
        let ids = |runs: &Runs| -> Vec<String> {
            let read = runs.lock().expect("the runs lock").clone();
            list_with(&sessions, &read, &dismissed)
                .into_iter()
                .map(|r| r.id)
                .collect()
        };

        tick_once(&files, &sessions, (&runs, &dismissed), &mut seen, &tx).await;
        assert!(pushed(&mut rx), "a finished run's request is a change");
        assert_eq!(ids(&runs), ["issue:20261028-100000:71"]);

        std::fs::remove_dir_all(&run_dir).expect("remove the run folder");
        tick_once(&files, &sessions, (&runs, &dismissed), &mut seen, &tx).await;
        assert!(pushed(&mut rx), "a request that ended is a change");
        assert!(ids(&runs).is_empty());
    }
}
