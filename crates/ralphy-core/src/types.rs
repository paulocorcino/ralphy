//! The domain vocabulary. None of these types name an agent vendor, an execution
//! mode, a PTY, or a model — that is the boundary the adapter sits behind.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One GitHub issue, as the queue sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub labels: Vec<String>,
}

/// A planning artifact produced by an [`crate::Agent`] for one issue. The plan
/// itself lives on disk at [`Plan::path`]; the counts are read off it.
#[derive(Debug, Clone)]
pub struct Plan {
    /// Where the plan markdown was written (`.ralphy/plan.md`).
    pub path: PathBuf,
    /// Number of open `- [ ]` steps. Zero means the planner judged the issue
    /// infeasible (the core treats it as a skip, not a failure).
    pub open_steps: usize,
    /// The planner's complexity judgment, if it emitted one. An adapter
    /// capability, never a core guarantee — the core only carries it across.
    pub recommended_model: Option<String>,
}

impl Plan {
    /// A plan is feasible when it carries at least one actionable step.
    pub fn is_feasible(&self) -> bool {
        self.open_steps > 0
    }
}

/// How one issue's execution finished. `Done` is the only green outcome; every
/// other variant stops the run and hands back the branch as it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Blocked(String),
    Timeout,
    Stuck,
    Limit,
}

/// The repository a run operates on, in place. Owns the paths under the
/// gitignored `.ralphy/` scratch dir that planner and executor read and write.
#[derive(Debug, Clone)]
pub struct Workspace {
    repo_root: PathBuf,
}

impl Workspace {
    pub fn new(repo_root: impl AsRef<Path>) -> Self {
        Self {
            repo_root: repo_root.as_ref().to_path_buf(),
        }
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// `<repo>/.ralphy` — the gitignored scratch root.
    pub fn ralphy_dir(&self) -> PathBuf {
        self.repo_root.join(".ralphy")
    }

    /// `<repo>/.ralphy/issue.json` — the fetched issue the planner reads.
    pub fn issue_json_path(&self) -> PathBuf {
        self.ralphy_dir().join("issue.json")
    }

    /// `<repo>/.ralphy/plan.md` — where the planner writes its plan.
    pub fn plan_path(&self) -> PathBuf {
        self.ralphy_dir().join("plan.md")
    }

    /// `<repo>/.ralphy/runs/<stamp>` — per-run logs and scratch.
    pub fn run_dir(&self, stamp: &str) -> PathBuf {
        self.ralphy_dir().join("runs").join(stamp)
    }
}
