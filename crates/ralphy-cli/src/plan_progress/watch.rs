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
    pub fn changed_text(&mut self, path: &Path) -> Option<String> {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
        if self.last_mtime == Some(mtime) {
            return None; // unchanged since the last poll
        }
        self.last_mtime = Some(mtime);
        std::fs::read_to_string(path).ok()
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
}
