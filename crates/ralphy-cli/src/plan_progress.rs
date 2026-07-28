//! Plan-step progress (#330): the **pure** parse/diff of a plan's checkbox lines,
//! shared by the event sink's `plan.step` poller and the run-snapshot engine.
//!
//! No clock, no transport, and no filesystem here — the one impure edge, the
//! mtime-guarded read, lives in [`watch`] so both engines share one guard rather
//! than duplicating an oracle.

mod watch;

pub use watch::PlanFileWatch;

/// A checkbox step's status, in the ADR-0019 `plan.step` vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StepStatus {
    #[default]
    Open,
    Checked,
    Noticed,
}

impl StepStatus {
    /// The wire word both consumers ship: the CloudEvents `data.status` and the
    /// snapshot document's `plan.steps[].status`.
    pub fn wire(self) -> &'static str {
        match self {
            StepStatus::Open => "open",
            StepStatus::Checked => "checked",
            StepStatus::Noticed => "noticed",
        }
    }

    /// Parse a wire word back; anything unknown reads as `Open` — the permissive
    /// direction the poller's `static_status` has always taken.
    pub fn from_wire(s: &str) -> StepStatus {
        match s {
            "checked" => StepStatus::Checked,
            "noticed" => StepStatus::Noticed,
            _ => StepStatus::Open,
        }
    }
}

/// One checkbox step: its identity text (already normalized) and its status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStep {
    pub text: String,
    pub status: StepStatus,
}

/// The plan a run is currently working, and whose issue it belongs to.
///
/// `issue` is the arming key: while it is `None` the plan on disk belongs to no
/// active issue (or to the PREVIOUS one) and must not be attributed to anybody.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanProgress {
    pub issue: Option<u64>,
    pub steps: Vec<PlanStep>,
}

/// Normalize a checkbox step's text to its identity key (#96): drop markdown
/// emphasis/code markers and collapse runs of whitespace — mirroring
/// `ralphy_core::acceptance::normalize_ac`'s technique, kept crate-local so the poll
/// does not widen a core API for one caller.
pub fn normalize_step(s: &str) -> String {
    let stripped: String = s
        .chars()
        .filter(|c| !matches!(c, '*' | '_' | '`'))
        .collect();
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse a plan's checkbox lines into steps (#96): a `- [ ]` line is `open`, a
/// `- [x]`/`- [X]` line is `checked`, a `- [!]` line is `noticed`. The text is
/// normalized so a whitespace/emphasis edit that leaves a step's meaning unchanged
/// is not a new step.
pub fn parse_steps(md: &str) -> Vec<PlanStep> {
    md.lines()
        .filter_map(|line| {
            let t = line.trim_start();
            let (status, rest) = if let Some(r) = t.strip_prefix("- [ ]") {
                (StepStatus::Open, r)
            } else if let Some(r) = t.strip_prefix("- [x]").or_else(|| t.strip_prefix("- [X]")) {
                (StepStatus::Checked, r)
            } else {
                let r = t.strip_prefix("- [!]")?;
                (StepStatus::Noticed, r)
            };
            Some(PlanStep {
                text: normalize_step(rest),
                status,
            })
        })
        .collect()
}

/// The steps that MOVED to `checked`/`noticed` between two parses, looked up by
/// normalized text. A step absent from `prev` that arrives already checked counts
/// as a transition; a status that did not move, or moved backwards, yields nothing.
pub fn transitions<'a>(prev: &[PlanStep], next: &'a [PlanStep]) -> Vec<&'a PlanStep> {
    next.iter()
        .filter(|step| {
            if step.status == StepStatus::Open {
                return false;
            }
            prev.iter()
                .find(|p| p.text == step.text)
                .map(|p| p.status != step.status)
                .unwrap_or(true)
        })
        .collect()
}

/// Test-only helpers shared by the poller, the watch and the snapshot engine, so
/// no `filetime` dev-dependency appears for one mtime bump.
#[cfg(test)]
pub(crate) mod testutil {
    use std::path::Path;
    use std::time::{Duration, Instant, SystemTime};

    /// Bump a file's mtime forward so an mtime guard sees a change even when the
    /// two writes land in the same clock tick (coarse FS timestamps).
    pub fn advance_mtime(path: &Path) {
        let now = SystemTime::now() + Duration::from_secs(2);
        // Re-write with an explicit later mtime via a set_modified where available;
        // fall back to a spin until the OS mtime actually advances.
        if std::fs::File::open(path)
            .and_then(|f| f.set_modified(now))
            .is_err()
        {
            let start = Instant::now();
            let initial = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            while std::fs::metadata(path).and_then(|m| m.modified()).ok() == initial {
                if start.elapsed() > Duration::from_secs(3) {
                    break;
                }
                std::fs::write(path, std::fs::read(path).unwrap()).ok();
            }
        }
    }
}

/// A scratch directory under the system temp dir, unique per test name.
#[cfg(test)]
pub(crate) fn test_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ralphy-{tag}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("temp dir is writable");
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_identity_across_whitespace_and_emphasis() {
        assert_eq!(normalize_step("do  a **`thing`**"), "do a thing");
        let prev = parse_steps("- [ ] do a **thing**\n");
        let next = parse_steps("- [x] do  a thing\n");
        let moved = transitions(&prev, &next);
        assert_eq!(
            moved.len(),
            1,
            "an emphasis/whitespace edit is the SAME step, not a new one: {moved:?}"
        );
        assert_eq!(moved[0].text, "do a thing");
    }

    #[test]
    fn parses_the_three_markers() {
        let md = "## Steps\nprose line\n- not a step\n- [ ] open one\n- [x] checked one\n- [!] noticed one — noticed: surprise\n  - [X] indented checked\n";
        let steps = parse_steps(md);
        assert_eq!(
            steps.iter().map(|s| s.status.wire()).collect::<Vec<_>>(),
            ["open", "checked", "noticed", "checked"],
            "only checkbox lines parse, and all three markers do"
        );
        assert_eq!(steps[3].text, "indented checked");
    }

    #[test]
    fn transitions_only_forward() {
        let checked = parse_steps("- [x] a\n");
        let open = parse_steps("- [ ] a\n");
        let noticed = parse_steps("- [!] a\n");
        assert_eq!(
            transitions(&checked, &checked).len(),
            0,
            "checked → checked is not a transition"
        );
        assert_eq!(
            transitions(&checked, &open).len(),
            0,
            "a regression to open emits nothing"
        );
        assert_eq!(transitions(&open, &noticed).len(), 1);
        assert_eq!(
            transitions(&[], &checked).len(),
            1,
            "a step first seen already checked is a transition"
        );
        assert_eq!(
            transitions(&[], &open).len(),
            0,
            "an open step is never a transition"
        );
    }

    #[test]
    fn wire_words_round_trip() {
        for status in [StepStatus::Open, StepStatus::Checked, StepStatus::Noticed] {
            assert_eq!(StepStatus::from_wire(status.wire()), status);
        }
        assert_eq!(StepStatus::from_wire("tomorrows_status"), StepStatus::Open);
    }
}
