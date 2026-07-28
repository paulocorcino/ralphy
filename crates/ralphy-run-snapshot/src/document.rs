//! The snapshot document (ADR-0047 §5) and where it lives on disk (§3).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The document version this build writes and is willing to read.
///
/// Change within a version is additive only; a reader that meets a higher `v`
/// refuses the document rather than guessing (ADR-0047 §6).
pub const SNAPSHOT_VERSION: u32 = 1;

/// A run's `RunState` projected at a moment in time — the panel's view of a run.
///
/// Every field EXCEPT `v` carries `#[serde(default)]`, so a reader parses a
/// newer document permissively within its version, ignoring what it does not
/// know. `v` deliberately has no default: a document that cannot state its own
/// version is malformed, not version 0 ([`crate::list_runs`] classifies it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub v: u32,
    #[serde(default)]
    pub runid: String,
    /// The publishing process. An identity fact, not a liveness assertion —
    /// the reader classifies it (ADR-0047 §7).
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub plan_agent: String,
    #[serde(default)]
    pub exec_agent: String,
    #[serde(default)]
    pub started_at: String,
    /// Repo-relative path of the run's plan, never its text (ADR-0047 §5).
    #[serde(default)]
    pub plan_path: String,
    #[serde(default)]
    pub queue: QueueBlock,
    #[serde(default)]
    pub issues: Vec<IssueBlock>,
    #[serde(default)]
    pub phase: PhaseBlock,
    /// The active issue's plan steps, so the panel renders accumulated progress
    /// instead of re-deriving it from an event feed (ADR-0047 §A1).
    #[serde(default)]
    pub plan: PlanBlock,
}

/// The plan the run is working, keyed by the issue it belongs to: between issues
/// `plan.md` still holds the PREVIOUS issue's plan, so a reader that trusted the
/// steps alone would attribute them to the wrong issue (ADR-0047 §A3).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlanBlock {
    #[serde(default)]
    pub issue: Option<u64>,
    #[serde(default)]
    pub steps: Vec<PlanStepBlock>,
}

/// One checkbox step: its normalized identity text and its `open`/`checked`/
/// `noticed` status — the same vocabulary the `plan.step` events ship.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlanStepBlock {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QueueBlock {
    #[serde(default)]
    pub total: usize,
    #[serde(default)]
    pub order: Vec<u64>,
    #[serde(default)]
    pub stop_before: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IssueBlock {
    #[serde(default)]
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub blocked_by: Vec<u64>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub budget_min: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PhaseBlock {
    #[serde(default)]
    pub active: Option<u64>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub sleep: Option<SleepBlock>,
    #[serde(default)]
    pub final_summary: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SleepBlock {
    #[serde(default)]
    pub reset: Option<String>,
    /// Unix seconds, so the browser computes its own countdown.
    #[serde(default)]
    pub target_epoch: i64,
}

impl Default for RunSnapshot {
    fn default() -> Self {
        Self {
            v: SNAPSHOT_VERSION,
            runid: String::new(),
            pid: 0,
            title: String::new(),
            repo: String::new(),
            branch: String::new(),
            plan_agent: String::new(),
            exec_agent: String::new(),
            started_at: String::new(),
            plan_path: String::new(),
            queue: QueueBlock::default(),
            issues: Vec::new(),
            phase: PhaseBlock::default(),
            plan: PlanBlock::default(),
        }
    }
}

/// `<repo>/.ralphy/runstate` — deliberately not `.ralphy/runs/`, which is keyed
/// by stamp rather than `runid` (ADR-0047 §3).
pub fn snapshot_dir(repo_root: &Path) -> PathBuf {
    repo_root.join(".ralphy").join("runstate")
}

/// `<repo>/.ralphy/runstate/<runid>.json`.
pub fn snapshot_path(repo_root: &Path, runid: &str) -> PathBuf {
    snapshot_dir(repo_root).join(format!("{runid}.json"))
}

/// `<repo>/.ralphy/runstate/<runid>.stop` — the cooperative-stop sentinel
/// (docs/adr/0054). Written by `ralphy stop`, noticed by the run's own snapshot
/// tick, removed by the run at exit.
///
/// It shares the snapshot directory because that is the only place a run is
/// already addressable by `runid`, and it is scoped to the runid so a sentinel
/// that outlives its run is INERT for the next one. `.stop` rather than `.json`
/// is load-bearing: [`crate::list_runs`] skips every non-`.json` entry, so the
/// sentinel is invisible to the reader by construction rather than by a filter
/// someone has to remember.
pub fn stop_path(repo_root: &Path, runid: &str) -> PathBuf {
    snapshot_dir(repo_root).join(format!("{runid}.stop"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_path_is_runid_keyed_under_runstate() {
        let p = snapshot_path(Path::new("/repo"), "01ABC");
        assert_eq!(p, snapshot_dir(Path::new("/repo")).join("01ABC.json"));
        assert!(p.ends_with("01ABC.json"));
    }

    #[test]
    fn missing_fields_parse_permissively() {
        // An older reader meeting a document that only carries `v`.
        let snap: RunSnapshot = serde_json::from_str(r#"{"v":1}"#).unwrap();
        assert_eq!(snap.v, SNAPSHOT_VERSION);
        assert!(snap.issues.is_empty());
        assert_eq!(snap.phase.state, "");
    }

    #[test]
    fn plan_block_parses_and_is_absent_by_default() {
        let snap: RunSnapshot = serde_json::from_str(
            r#"{"v":1,"plan":{"issue":7,"steps":[{"text":"a","status":"checked"}]},"tomorrows_field":1}"#,
        )
        .unwrap();
        assert_eq!(snap.plan.issue, Some(7));
        assert_eq!(snap.plan.steps[0].status, "checked");
        assert_eq!(snap.plan.steps[0].text, "a");

        let bare: RunSnapshot = serde_json::from_str(r#"{"v":1}"#).unwrap();
        assert_eq!(
            bare.plan,
            PlanBlock::default(),
            "a document without the block still parses"
        );
        // The block is additive within v = 1 (ADR-0047 §A1): no version bump.
        assert_eq!(SNAPSHOT_VERSION, 1);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let snap: RunSnapshot =
            serde_json::from_str(r#"{"v":1,"runid":"X","tomorrows_field":{"a":1}}"#).unwrap();
        assert_eq!(snap.runid, "X");
    }
}
