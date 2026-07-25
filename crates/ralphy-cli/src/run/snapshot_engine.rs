//! The run-snapshot publisher: the third destination on the ADR-0024 delivery
//! seam (ADR-0047 §2).
//!
//! It is not a new mechanism — the ring, the `DeliveryLayer` and the bounded
//! worker are the shared [`crate::delivery`] spine; this is one
//! [`DeliveryEngine`] fold that projects its own `RunState` into the versioned
//! document and writes it atomically under the repo's `.ralphy/`. The write is
//! therefore off the run path, coalesced by the 250 ms poll, and best-effort:
//! a failure warns ONCE per run and never reaches the run.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ralphy_run_snapshot::RunSnapshot;
use tracing::warn;

use crate::delivery::{spawn_worker, DeliveryEngine, EventQueue, WorkerHandle};
use crate::runstate::snapshot::{project, SnapshotCtx};
use crate::runstate::{RunEvent, RunState};

/// The snapshot fold (ADR-0024, ADR-0047 §2): folds each drained event into its
/// own [`RunState`] exactly as the Telegram engine does, and rewrites the
/// document whenever the projection differs from the one last written.
struct SnapshotEngine {
    ctx: SnapshotCtx,
    state: RunState,
    last: Option<RunSnapshot>,
    repo_root: PathBuf,
    warned: AtomicBool,
}

impl SnapshotEngine {
    /// Project and write, but only when the document would differ from the last
    /// one written — an unchanged fold costs no write (ADR-0047 §8).
    fn publish(&mut self, force: bool) {
        let next = project(&self.ctx, &self.state);
        if !force && self.last.as_ref() == Some(&next) {
            return;
        }
        match ralphy_run_snapshot::write_atomic(&self.repo_root, &next) {
            Ok(()) => self.last = Some(next),
            // Best-effort by decision: a full disk delays a panel update, never
            // the run. `last` is left alone so the next tick retries.
            Err(e) => warn_once(&self.warned, &e),
        }
    }
}

impl DeliveryEngine for SnapshotEngine {
    fn on_start(&mut self) {
        // Force the first write so the run appears in the panel before its first
        // event, even though the projection has not "changed" yet.
        self.publish(true);
    }

    fn on_event(&mut self, event: RunEvent) {
        self.state.apply(event);
    }

    fn on_tick(&mut self, _changed: bool) {
        self.publish(false);
    }

    fn on_finish(&mut self) {
        self.publish(false);
    }
}

/// Emit the single non-spamming write-failure warning for the run, under the
/// engine's OWN `tracing` target so [`crate::delivery::DeliveryLayer`]'s
/// self-target filter drops it instead of folding it back into the ring.
fn warn_once(warned: &AtomicBool, error: &std::io::Error) {
    if warned.swap(true, Ordering::SeqCst) {
        return;
    }
    warn!(
        target: "ralphy_cli::run::snapshot",
        error = %error,
        "could not write the run snapshot — the Runs panel will not see this run (further failures silenced this run)"
    );
}

/// The engine's detach-warn hook (ADR-0024), under the same self-target.
fn detach_warn() {
    warn!(target: "ralphy_cli::run::snapshot", "snapshot worker did not finish in time — detaching");
}

/// Spawn the `"ralphy-snapshot"` worker draining `queue` through a
/// [`SnapshotEngine`]. A spawn failure leaves the installed Layer inert (the ring
/// just fills and drops) rather than aborting the run.
pub fn try_start_snapshot(
    ctx: SnapshotCtx,
    state: RunState,
    repo_root: PathBuf,
    queue: Arc<EventQueue>,
) -> Option<WorkerHandle> {
    let engine = SnapshotEngine {
        ctx,
        state,
        last: None,
        repo_root,
        warned: AtomicBool::new(false),
    };
    spawn_worker("ralphy-snapshot", engine, queue, detach_warn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runstate::UsageLite;
    use ralphy_run_snapshot::{snapshot_path, SnapshotGuard};

    fn test_ctx(runid: &str) -> SnapshotCtx {
        SnapshotCtx {
            runid: runid.into(),
            pid: std::process::id(),
            repo: "owner/repo".into(),
            branch: "afk/run-1".into(),
            started_at: "2026-07-24T10:00:00-03:00".into(),
            plan_path: ".ralphy/plan.md".into(),
        }
    }

    /// The whole spine, end to end: events buffered on the ring BEFORE the worker
    /// exists (the `deliver_from_pre_start_ring` pattern), the REAL delivery
    /// worker driving the real engine over a tempdir repo, then the RAII guard.
    #[test]
    fn spine_writes_then_removes_the_document() {
        let dir = tempfile::tempdir().unwrap();
        let runid = "01SPINETESTRUNID";
        let queue = Arc::new(EventQueue::new());
        queue.push(RunEvent::QueueBuilt {
            count: 3,
            order: vec![71, 72, 73],
            stop_before: None,
            issues: serde_json::json!([
                {"number": 71, "title": "seventy-one"},
                {"number": 72, "title": "seventy-two"},
                {"number": 73, "title": "seventy-three"},
            ]),
            assignee_filter: None,
            scope: None,
        });
        queue.push(RunEvent::IssueStarted {
            number: 71,
            title: "seventy-one".into(),
        });
        queue.push(RunEvent::Executing {
            number: 71,
            budget_min: 45,
            model: "opus".into(),
            effort: None,
        });

        let handle = try_start_snapshot(
            test_ctx(runid),
            RunState::new("spine", 3),
            dir.path().to_path_buf(),
            queue,
        )
        .expect("the snapshot worker spawns");
        handle.shutdown();

        let path = snapshot_path(dir.path(), runid);
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["phase"]["state"], "executing");
        assert_eq!(doc["phase"]["active"], 71);
        assert_eq!(doc["queue"]["total"], 3);
        assert_eq!(doc["issues"].as_array().unwrap().len(), 3);
        assert_eq!(doc["runid"], runid);

        drop(SnapshotGuard::new(dir.path(), runid));
        assert!(!path.exists(), "the guard removes the document at exit");
    }

    #[test]
    fn engine_rewrites_only_when_the_projection_changes() {
        let dir = tempfile::tempdir().unwrap();
        let runid = "01UNCHANGED";
        let mut engine = SnapshotEngine {
            ctx: test_ctx(runid),
            state: RunState::new("t", 1),
            last: None,
            repo_root: dir.path().to_path_buf(),
            warned: AtomicBool::new(false),
        };
        engine.on_start();
        let path = snapshot_path(dir.path(), runid);
        assert!(path.exists(), "on_start publishes before any event");

        // An idle tick must not rewrite: delete the file and prove the unchanged
        // tick leaves it deleted (a rewrite would recreate it).
        std::fs::remove_file(&path).unwrap();
        engine.on_tick(false);
        assert!(!path.exists(), "an unchanged fold costs no write");

        // A folded event changes the projection, so the next tick does write.
        engine.on_event(RunEvent::IssueClosed {
            number: 1,
            tokens: 0,
            invocations: 0,
            usage: UsageLite::default(),
        });
        engine.on_tick(true);
        assert!(path.exists(), "a changed fold rewrites the document");
    }
}
