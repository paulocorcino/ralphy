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
/// Every field carries `#[serde(default)]` so a reader parses a newer document
/// permissively within its version, ignoring what it does not know.
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
    fn unknown_fields_are_ignored() {
        let snap: RunSnapshot =
            serde_json::from_str(r#"{"v":1,"runid":"X","tomorrows_field":{"a":1}}"#).unwrap();
        assert_eq!(snap.runid, "X");
    }
}
