//! The one impure edge of plan-progress: an mtime-guarded read of `plan.md`.
//!
//! Both the event sink's poller and the snapshot engine tick at 250 ms; without
//! this guard each tick would re-read and re-parse a file that almost never
//! changes.

use std::path::Path;
use std::time::SystemTime;

/// The last-seen mtime of a plan file. Best-effort throughout — a stat or read
/// failure is a silent `None` (the plan may not exist yet between issues).
#[derive(Default)]
pub struct PlanFileWatch {
    last_mtime: Option<SystemTime>,
}

impl PlanFileWatch {
    /// The plan's text, but only when its mtime advanced since the last call.
    ///
    /// The mtime is recorded only after the read SUCCEEDS: `metadata` does not
    /// open the file, so on Windows it answers happily while the read loses to a
    /// sharing violation (an editor holding `plan.md`). Recording first would
    /// retire that revision unread and freeze the step list until the next write.
    pub fn changed_text(&mut self, path: &Path) -> Option<String> {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
        if self.last_mtime == Some(mtime) {
            return None; // unchanged since the last poll
        }
        let text = std::fs::read_to_string(path).ok()?;
        self.last_mtime = Some(mtime);
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan_progress::{test_dir, testutil};

    #[test]
    fn reads_only_when_mtime_advances() {
        let dir = test_dir("plan-watch");
        let plan = dir.join("plan.md");
        std::fs::write(&plan, "- [ ] a\n").unwrap();

        let mut watch = PlanFileWatch::default();
        assert_eq!(watch.changed_text(&plan).as_deref(), Some("- [ ] a\n"));
        assert_eq!(watch.changed_text(&plan), None, "unchanged mtime, no read");

        std::fs::write(&plan, "- [x] a\n").unwrap();
        testutil::advance_mtime(&plan);
        assert_eq!(watch.changed_text(&plan).as_deref(), Some("- [x] a\n"));

        assert_eq!(
            PlanFileWatch::default().changed_text(&dir.join("nope.md")),
            None,
            "a missing plan is a silent no-op"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A revision whose read fails must NOT be retired: the next call re-reads
    /// the same mtime rather than waiting for another write.
    #[test]
    fn a_failed_read_does_not_retire_the_revision() {
        let dir = test_dir("plan-watch-failed-read");
        let plan = dir.join("plan.md");
        let mut watch = PlanFileWatch::default();

        // A DIRECTORY at the plan's path stats fine and reads as an error —
        // the portable stand-in for a Windows sharing violation.
        std::fs::create_dir(&plan).unwrap();
        let lost = std::fs::metadata(&plan).and_then(|m| m.modified()).unwrap();
        assert_eq!(
            watch.changed_text(&plan),
            None,
            "an unreadable plan reads None"
        );
        std::fs::remove_dir(&plan).unwrap();

        // The same mtime comes back readable: recording it on the failed read
        // would make this return None (the step list frozen until a new write).
        // `set_modified` needs a WRITE handle on Windows (`File::open` alone
        // fails with "access denied").
        std::fs::write(&plan, "- [x] a\n").unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&plan)
            .and_then(|f| f.set_modified(lost))
            .unwrap();
        assert_eq!(
            watch.changed_text(&plan).as_deref(),
            Some("- [x] a\n"),
            "the lost revision did not poison the guard"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
