//! The RAII removal guard (ADR-0047 §8), modelled on `runlock::RunLockGuard`.

use std::path::{Path, PathBuf};

use crate::document::snapshot_path;

/// Removes a run's snapshot document when dropped, so a finished run leaves the
/// panel because its document is gone — nothing computes an expiry.
///
/// Held for the whole run process, so `Drop` covers every normal and
/// `?`-propagated exit. There is deliberately no signal handler: a killed
/// process leaves the document behind, and that orphan is exactly what
/// [`crate::list_runs`]'s dead-pid sweep recovers.
pub struct SnapshotGuard {
    path: PathBuf,
}

impl SnapshotGuard {
    pub fn new(repo_root: &Path, runid: &str) -> Self {
        Self {
            path: snapshot_path(repo_root, runid),
        }
    }
}

impl Drop for SnapshotGuard {
    fn drop(&mut self) {
        // Never panic in Drop, and this crate carries no logger; a document
        // left behind is the orphan the reader's dead-pid sweep deletes.
        let _ = std::fs::remove_file(&self.path);
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
}
