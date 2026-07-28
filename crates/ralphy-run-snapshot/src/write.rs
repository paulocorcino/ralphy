//! The atomic writer (ADR-0047 §3).

use std::path::Path;

use crate::document::{snapshot_dir, snapshot_path, RunSnapshot};

/// Publish `snap` under `repo_root`, replacing any previous document for the
/// same `runid`.
///
/// Temp file in the SAME directory plus `fs::rename`: `rename` replaces the
/// destination on both Unix and Windows (there `MoveFileExW` with
/// `MOVEFILE_REPLACE_EXISTING`), so a concurrent reader sees either the old
/// document or the new one — never a truncated one and never a gap. Deleting
/// the destination first would open exactly that gap. The temp name carries
/// the writing pid so two concurrent runs in one repo cannot collide.
pub fn write_atomic(repo_root: &Path, snap: &RunSnapshot) -> std::io::Result<()> {
    let dir = snapshot_dir(repo_root);
    std::fs::create_dir_all(&dir)?;
    let bytes = serde_json::to_vec(snap).map_err(std::io::Error::other)?;
    let tmp = dir.join(format!("{}.{}.tmp", snap.runid, std::process::id()));
    std::fs::write(&tmp, &bytes)?;
    match std::fs::rename(&tmp, snapshot_path(repo_root, &snap.runid)) {
        Ok(()) => Ok(()),
        Err(e) => {
            // The rename error is what the caller must see; a failure to clean
            // the temp file would only mask it, and the next write overwrites
            // that name (it carries this pid).
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::IssueBlock;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn snap_with(runid: &str, issues: usize) -> RunSnapshot {
        RunSnapshot {
            runid: runid.into(),
            pid: std::process::id(),
            issues: (0..issues)
                .map(|i| IssueBlock {
                    number: i as u64,
                    title: format!("issue {i}"),
                    status: "pending".into(),
                    ..IssueBlock::default()
                })
                .collect(),
            ..RunSnapshot::default()
        }
    }

    #[test]
    fn write_atomic_replaces_and_never_shows_a_partial_document() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let path = snapshot_path(&root, "01WRITER");

        // Seed one document so the reader thread has something from the start.
        write_atomic(&root, &snap_with("01WRITER", 0)).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let reader_stop = Arc::clone(&stop);
        let reader_path = path.clone();
        let reader = std::thread::spawn(move || {
            let mut reads = 0usize;
            let mut parsed = 0usize;
            while !reader_stop.load(Ordering::Relaxed) {
                // A rename is atomic, but the destination may momentarily be
                // unobservable on Windows (sharing violation) — only a read
                // that SUCCEEDED is required to parse.
                if let Ok(raw) = std::fs::read_to_string(&reader_path) {
                    reads += 1;
                    assert!(
                        serde_json::from_str::<RunSnapshot>(&raw).is_ok(),
                        "reader observed a truncated document: {raw:?}"
                    );
                    parsed += 1;
                }
            }
            (reads, parsed)
        });

        for n in 1..=200 {
            write_atomic(&root, &snap_with("01WRITER", n)).unwrap();
        }
        stop.store(true, Ordering::Relaxed);
        let (reads, parsed) = reader.join().unwrap();
        assert!(reads > 0, "reader never observed the document");
        assert_eq!(reads, parsed);

        let last: RunSnapshot =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(last.issues.len(), 200);
    }

    #[test]
    fn write_atomic_leaves_no_temp_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        write_atomic(dir.path(), &snap_with("01TEMP", 1)).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(snapshot_dir(dir.path()))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
    }
}
