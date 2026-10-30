//! The RAII removal guard (ADR-0047 §8), modelled on `runlock::RunLockGuard`.

use std::path::{Path, PathBuf};

use crate::document::{final_snapshot_path, snapshot_path, stop_path};

/// Removes a run's snapshot document — and its cooperative-stop sentinel, if one
/// was written — when dropped, so a finished run leaves the panel because its
/// document is gone and nothing computes an expiry.
///
/// The sentinel rides THIS guard rather than a second one of its own: it shares
/// the lifetime, the directory and the runid, so a parallel guard would be
/// ceremony around one more `remove_file`.
///
/// Held for the whole run process, so `Drop` covers every normal and
/// `?`-propagated exit. There is deliberately no signal handler: a killed
/// process leaves both files behind, and those orphans are exactly what
/// [`crate::list_runs`]'s dead-pid sweep recovers.
pub struct SnapshotGuard {
    path: PathBuf,
    stop: PathBuf,
    /// Where the last document is kept when the run ends, and what to call
    /// when it cannot be kept ([`Self::keep_final_in`]).
    keep: Option<Keep>,
}

/// The final path of [`SnapshotGuard::keep_final_in`] and its reporter.
type Keep = (PathBuf, fn(&std::io::Error));

impl SnapshotGuard {
    pub fn new(repo_root: &Path, runid: &str) -> Self {
        Self {
            path: snapshot_path(repo_root, runid),
            stop: stop_path(repo_root, runid),
            keep: None,
        }
    }

    /// At drop, move the last document to [`final_snapshot_path`] of
    /// `run_dir` instead of only removing it, so the run's folder keeps what
    /// the run finished with (docs/adr/0085 D11). `report` is called when the
    /// document cannot be kept; the live document is removed either way. This
    /// crate carries no logger, so the caller says how a failure is logged.
    pub fn keep_final_in(mut self, run_dir: &Path, report: fn(&std::io::Error)) -> Self {
        self.keep = Some((final_snapshot_path(run_dir), report));
        self
    }
}

/// Move the live document to `to`. A run that never wrote one keeps nothing,
/// and that is not a failure. `rename` replaces `to` on every OS, and the two
/// paths are in the same repo, so on one file system.
fn keep_final(live: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(live, to) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !live.exists() => Ok(()),
        other => other,
    }
}

impl Drop for SnapshotGuard {
    fn drop(&mut self) {
        if let Some((to, report)) = &self.keep {
            if let Err(e) = keep_final(&self.path, to) {
                report(&e);
            }
        }
        // Never panic in Drop, and this crate carries no logger; a file left
        // behind is the orphan the reader's dead-pid sweep deletes.
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a drop cannot return the error, and the file may be gone already"
        )]
        let _ = std::fs::remove_file(&self.path);
        // The sentinel is already inert once this run is gone (the next run has
        // a different runid), so this is hygiene rather than correctness.
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a drop cannot return the error, and the file may be gone already"
        )]
        let _ = std::fs::remove_file(&self.stop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::RunSnapshot;
    use crate::write::write_atomic;

    #[test]
    fn guard_removes_the_document_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        write_atomic(
            dir.path(),
            &RunSnapshot {
                runid: "01GUARD".into(),
                pid: std::process::id(),
                ..RunSnapshot::default()
            },
        )
        .unwrap();
        let path = snapshot_path(dir.path(), "01GUARD");
        let guard = SnapshotGuard::new(dir.path(), "01GUARD");
        assert!(path.exists());
        drop(guard);
        assert!(!path.exists());
    }

    #[test]
    fn guard_drop_tolerates_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        drop(SnapshotGuard::new(dir.path(), "01NEVERWRITTEN"));
    }

    /// A stopped run must not leave its sentinel behind: the file is inert for
    /// the next run (different runid), but the directory is the operator's.
    #[test]
    fn guard_removes_the_stop_sentinel_too() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(crate::snapshot_dir(dir.path())).unwrap();
        let sentinel = stop_path(dir.path(), "01STOPPED");
        std::fs::write(&sentinel, "{}").unwrap();
        let guard = SnapshotGuard::new(dir.path(), "01STOPPED");
        assert!(sentinel.exists());
        drop(guard);
        assert!(!sentinel.exists());
    }

    /// A run that never wrote a document keeps no final snapshot, and that is
    /// not a failure to report.
    #[test]
    fn a_run_with_no_document_keeps_nothing_and_reports_nothing() {
        static REPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("runs").join("20261028-100000");
        std::fs::create_dir_all(&run_dir).unwrap();
        drop(
            SnapshotGuard::new(dir.path(), "01NEVERWRITTEN").keep_final_in(&run_dir, |_| {
                REPORTED.store(true, std::sync::atomic::Ordering::SeqCst)
            }),
        );
        assert!(!final_snapshot_path(&run_dir).exists());
        assert!(!REPORTED.load(std::sync::atomic::Ordering::SeqCst));
    }
}
