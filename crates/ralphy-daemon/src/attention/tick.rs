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
//!
//! A daemon that sends Telegram messages also reads each peer's own list on
//! the tick ([`peer_requests`]), for the peers' console requests and for
//! whether the operator is at a page of the peer.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use tokio::sync::broadcast;

use super::dismissals::Dismissals;
use super::done::{self, DeskFiles, Marks};
use super::finals::Finals;
use super::telegram::Notifier;
use super::{list, AttentionRequest, RunSource, RunsRead};
use crate::registry;
use crate::routes::{push, read_peer_lists, PeerList, Push};
use crate::session::{SessionInfo, SessionManager};

/// How often the tick reads the runs.
pub const EVERY: Duration = Duration::from_secs(3);

/// The run snapshots of the last tick, shared with the route.
pub(crate) type Runs = Arc<Mutex<RunsRead>>;

/// The dismissals, shared by the tick and the route.
pub(crate) type Dismissed = Arc<Mutex<Dismissals>>;

/// The live and the finished runs of every registered repo, in key order. A
/// repo that is not reachable has none; an unreadable registry is logged and
/// has none. `finals` keeps the final snapshots already read.
fn read_runs(registry_path: &Path, finals: &mut Finals) -> RunsRead {
    let store = match registry::load_from(registry_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(error = %e, "the attention tick could not read the repo registry");
            finals.end_pass();
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
        runs.finished
            .extend(finals.of_repo(root, slug, &ralphy_run_snapshot::read_final));
    }
    finals.end_pass();
    runs
}

/// The list of `sessions` now, the runs `runs` and the `dismissed` ids. A
/// poisoned dismissal lock is logged and dismisses nothing.
pub(crate) fn list_with(
    sessions: &SessionManager,
    runs: &RunsRead,
    dismissed: &Dismissed,
) -> Vec<AttentionRequest> {
    list_of(&sessions.list_at(SystemTime::now()), runs, dismissed)
}

/// [`list_with`], for the consoles `consoles` already read.
fn list_of(
    consoles: &[SessionInfo],
    runs: &RunsRead,
    dismissed: &Dismissed,
) -> Vec<AttentionRequest> {
    match dismissed.lock() {
        Ok(dismissed) => list(consoles, runs, &|id| dismissed.contains(id)),
        Err(e) => {
            tracing::warn!(error = %e, "the attention dismissals are poisoned");
            list(consoles, runs, &|_| false)
        }
    }
}

/// What the peers' own lists hold for the Telegram sender.
#[derive(Debug, Default)]
pub(crate) struct PeerRequests {
    /// The peers' requests, with the peer's daemon id before their `id` and
    /// their `console`: two daemons' `console:1` stay two requests, and the
    /// link names the peer's console. The repo stays the peer's slug, as a
    /// person reads it.
    pub(crate) requests: Vec<AttentionRequest>,
    /// What each peer that answered said of the operator's presence.
    pub(crate) present: Vec<bool>,
    /// The peers that did not answer.
    pub(crate) unanswered: Vec<String>,
}

/// The own list of each peer in `peers_dir`, read as the route reads it.
/// An empty `peers_dir` is no peers folder: a daemon built with no store.
pub(crate) async fn peer_requests(peers_dir: PathBuf) -> PeerRequests {
    if peers_dir.as_os_str().is_empty() {
        return PeerRequests::default();
    }
    let (lists, unanswered) = read_peer_lists(peers_dir).await;
    let mut read = PeerRequests {
        unanswered,
        ..PeerRequests::default()
    };
    for PeerList { daemon_id, list } in lists {
        read.present.push(list.present.unwrap_or(false));
        read.requests
            .extend(list.requests.into_iter().map(|mut request| {
                request.id = format!("{daemon_id}/{}", request.id);
                request.console = request.console.map(|c| format!("{daemon_id}/{c}"));
                request
            }));
    }
    read
}

/// What the last tick saw: the id and the detail of each request, the done
/// marks it looked at, and the final snapshots it read.
#[derive(Default)]
pub(crate) struct Seen {
    last: Vec<(String, Option<String>)>,
    marks: Marks,
    finals: Finals,
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
/// operator marked, keep both for the route, push `attention.dirty` when
/// the list changed, and send the console requests that wait for an
/// operator who is away ([`Notifier`]).
pub(crate) async fn tick_once(
    files: &DeskFiles,
    sessions: &SessionManager,
    (runs, dismissed): (&Runs, &Dismissed),
    (seen, telegram): (&mut Seen, &mut Notifier),
    pushes: &broadcast::Sender<Push>,
) {
    let path = files.registry.clone();
    let mut finals = std::mem::take(&mut seen.finals);
    let joined = tokio::task::spawn_blocking(move || (read_runs(&path, &mut finals), finals)).await;
    let mut read = match joined {
        Ok((read, finals)) => {
            seen.finals = finals;
            read
        }
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
    let consoles = sessions.list_at(SystemTime::now());
    let requests = list_of(&consoles, &read, dismissed);
    match runs.lock() {
        Ok(mut kept) => *kept = read,
        Err(e) => tracing::warn!(error = %e, "the attention runs are poisoned"),
    }
    if seen.changed(&requests) {
        push(pushes, Push::Attention);
    }
    telegram.tick(&requests).await;
}

/// Tick every [`EVERY`] until the daemon stops.
pub(crate) async fn run(
    files: DeskFiles,
    sessions: Arc<SessionManager>,
    (runs, dismissed): (Runs, Dismissed),
    (pushes, mut telegram): (broadcast::Sender<Push>, Notifier),
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut seen = Seen::default();
    let mut every = tokio::time::interval(EVERY);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = every.tick() => {
                tick_once(&files, &sessions, (&runs, &dismissed), (&mut seen, &mut telegram), &pushes).await;
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

    /// A sender with no config: it sends nothing.
    fn no_telegram() -> Notifier {
        Notifier::new(
            Default::default(),
            Default::default(),
            Default::default(),
            None,
        )
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
        let mut telegram = no_telegram();
        let (tx, mut rx) = broadcast::channel(8);
        macro_rules! tick {
            () => {
                tick_once(
                    &files,
                    &sessions,
                    (&runs, &dismissed),
                    (&mut seen, &mut telegram),
                    &tx,
                )
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
        let mut telegram = no_telegram();
        let (tx, mut rx) = broadcast::channel(8);
        let ids = |runs: &Runs| -> Vec<String> {
            let read = runs.lock().expect("the runs lock").clone();
            list_with(&sessions, &read, &dismissed)
                .into_iter()
                .map(|r| r.id)
                .collect()
        };

        tick_once(
            &files,
            &sessions,
            (&runs, &dismissed),
            (&mut seen, &mut telegram),
            &tx,
        )
        .await;
        assert!(pushed(&mut rx), "a finished run's request is a change");
        assert_eq!(ids(&runs), ["issue:20261028-100000:71"]);

        std::fs::remove_dir_all(&run_dir).expect("remove the run folder");
        tick_once(
            &files,
            &sessions,
            (&runs, &dismissed),
            (&mut seen, &mut telegram),
            &tx,
        )
        .await;
        assert!(pushed(&mut rx), "a request that ended is a change");
        assert!(ids(&runs).is_empty());
    }
}
