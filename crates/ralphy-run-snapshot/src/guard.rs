//! The RAII removal guard (ADR-0047 §8), modelled on `runlock::RunLockGuard`.

use std::path::{Path, PathBuf};

use crate::document::{snapshot_path, stop_path};

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
}

impl SnapshotGuard {
    pub fn new(repo_root: &Path, runid: &str) -> Self {
        Self {
            path: snapshot_path(repo_root, runid),
            stop: stop_path(repo_root, runid),
        }
    }
}

impl Drop for SnapshotGuard {
    fn drop(&mut self) {
        // Never panic in Drop, and this crate carries no logger; a file left
        // behind is the orphan the reader's dead-pid sweep deletes.
        let _ = std::fs::remove_file(&self.path);
        // The sentinel is already inert once this run is gone (the next run has
        // a different runid), so this is hygiene rather than correctness.
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
}
