//! The final snapshots of the finished runs, as the attention tick reads them
//! (docs/adr/0085 D11). A run writes its final snapshot once, when it ends,
//! and never again, so the tick keeps each one it read, by run folder and
//! with the time its file was written, and reads a file again only when that
//! time changed. A folder that is gone leaves the cache at the next pass.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ralphy_run_snapshot::RunSnapshot;

use super::FinishedRun;

/// What reads the final snapshot of a run folder.
pub(crate) type ReadFinal<'a> = &'a dyn Fn(&Path) -> Option<RunSnapshot>;

/// The final snapshots the tick read, by run folder.
#[derive(Debug, Default)]
pub(crate) struct Finals {
    kept: HashMap<PathBuf, (SystemTime, FinishedRun)>,
    /// The folders seen in the pass under way.
    next: HashMap<PathBuf, (SystemTime, FinishedRun)>,
}

impl Finals {
    /// The final snapshots in the run folders of the repo at `root`, in
    /// folder name order. `<repo>/.ralphy/runs/` is the run folders'
    /// directory (`Workspace::runs_dir` in ralphy-core, which the daemon
    /// does not import). A folder with no final snapshot this build reads is
    /// not a finished run.
    pub(crate) fn of_repo(&mut self, root: &Path, slug: &str, read: ReadFinal) -> Vec<FinishedRun> {
        let Ok(entries) = std::fs::read_dir(root.join(".ralphy").join("runs")) else {
            return Vec::new();
        };
        let mut finished: Vec<FinishedRun> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let stamp = entry.file_name().into_string().ok()?;
                self.of_folder(&entry.path(), slug, stamp, read)
            })
            .collect();
        finished.sort_by(|a, b| a.stamp.cmp(&b.stamp));
        finished
    }

    /// The finished run of the folder `dir`, from the cache when its final
    /// snapshot has the time the cache kept. A file whose time cannot be read
    /// is read each time, and ends at the run's start.
    fn of_folder(
        &mut self,
        dir: &Path,
        slug: &str,
        stamp: String,
        read: ReadFinal,
    ) -> Option<FinishedRun> {
        let written = std::fs::metadata(ralphy_run_snapshot::final_snapshot_path(dir))
            .and_then(|m| m.modified())
            .ok();
        let Some(written) = written else {
            let snapshot = read(dir)?;
            return Some(FinishedRun {
                repo: slug.to_string(),
                stamp,
                ended: snapshot.started_at.clone(),
                snapshot,
            });
        };
        let run = match self.kept.remove(dir) {
            Some((at, mut run)) if at == written => {
                run.repo = slug.to_string();
                run
            }
            _ => FinishedRun {
                repo: slug.to_string(),
                stamp,
                ended: chrono::DateTime::<chrono::Utc>::from(written)
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                snapshot: read(dir)?,
            },
        };
        self.next.insert(dir.to_path_buf(), (written, run.clone()));
        Some(run)
    }

    /// End a pass: the cache keeps only the folders this pass saw.
    pub(crate) fn end_pass(&mut self) {
        self.kept = std::mem::take(&mut self.next);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    /// A run folder `stamp` of the repo at `repo` with a final snapshot.
    fn finished_run(repo: &Path, stamp: &str) -> PathBuf {
        let dir = repo.join(".ralphy").join("runs").join(stamp);
        std::fs::create_dir_all(&dir).expect("the run folder");
        let snapshot = RunSnapshot {
            runid: format!("01{stamp}"),
            ..RunSnapshot::default()
        };
        std::fs::write(
            ralphy_run_snapshot::final_snapshot_path(&dir),
            serde_json::to_vec(&snapshot).expect("a snapshot serializes"),
        )
        .expect("the final snapshot");
        dir
    }

    #[test]
    fn an_unchanged_final_snapshot_is_read_once() {
        let repo = tempfile::tempdir().expect("a temp dir");
        let first = finished_run(repo.path(), "20261028-100000");
        let second = finished_run(repo.path(), "20261028-110000");
        let reads = Cell::new(0);
        let read = |dir: &Path| {
            reads.set(reads.get() + 1);
            ralphy_run_snapshot::read_final(dir)
        };
        let mut finals = Finals::default();
        let pass = |finals: &mut Finals| {
            let runs = finals.of_repo(repo.path(), "owner/r", &read);
            finals.end_pass();
            runs.into_iter()
                .map(|r| r.snapshot.runid)
                .collect::<Vec<_>>()
        };

        let both = ["0120261028-100000", "0120261028-110000"];
        assert_eq!(pass(&mut finals), both);
        assert_eq!(reads.get(), 2);
        assert_eq!(
            pass(&mut finals),
            both,
            "the second pass lists the same runs"
        );
        assert_eq!(reads.get(), 2, "and reads no file again");

        // A final snapshot whose file time changed is read again.
        let path = ralphy_run_snapshot::final_snapshot_path(&second);
        let rewritten = RunSnapshot {
            runid: "01REWRITTEN".to_string(),
            ..RunSnapshot::default()
        };
        std::fs::write(
            &path,
            serde_json::to_vec(&rewritten).expect("a snapshot serializes"),
        )
        .expect("rewrite the final snapshot");
        let later = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .expect("the file time")
            + std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|f| f.set_modified(later))
            .expect("move the file time");
        assert_eq!(pass(&mut finals), ["0120261028-100000", "01REWRITTEN"]);
        assert_eq!(reads.get(), 3, "only the changed file is read again");

        std::fs::remove_dir_all(&first).expect("remove a run folder");
        assert_eq!(pass(&mut finals), ["01REWRITTEN"]);
        assert!(
            !finals.kept.contains_key(&first),
            "a removed folder leaves the cache"
        );
        assert_eq!(finals.kept.len(), 1);
    }
}
