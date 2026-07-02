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
    /// The issue's comment bodies, in thread order. Empty as built by the queue
    /// (`gh issue list` does not carry comments); the runner fills it just before
    /// planning by fetching the selected issue's comments, so the planner and
    /// executor see the discussion — not just the original body — via
    /// `.ralphy/issue.json`. `#[serde(default)]` keeps older `issue.json` files
    /// and the queue's comment-less issues deserializing cleanly.
    #[serde(default)]
    pub comments: Vec<String>,
}

/// A normalized, vendor-agnostic token-usage record (ADR-0008 D4). Each adapter
/// fills it from the counts its CLI already reports; the core only sums it and
/// never branches on `model`. `cache_read`/`cache_creation` are kept as separate
/// fields (not folded into `input`) because Claude reports cache reads at ~1/10th
/// the price of fresh input, so collapsing them would overstate cost by an order
/// of magnitude (ADR-0008 D2). `model` rides along because price resolves on it
/// (D8) and is only knowable per-record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
    pub model: Option<String>,
}

impl Usage {
    /// Add another usage's four numeric fields into this one. `model` is left
    /// untouched — summing is across records of (potentially) different models,
    /// and the per-record model is what the price table resolves on (D8).
    pub fn add_tokens(&mut self, other: &Usage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_creation += other.cache_creation;
    }

    /// The flat token total across the four numeric fields — the figure the
    /// run-end footer and project roll-ups present (ADR-0008 D11).
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_creation
    }
}

/// The pairing an [`crate::Agent::execute`] hands back: the domain [`Outcome`]
/// plus the [`Usage`] the phase consumed (ADR-0008 D4). It is a struct rather
/// than a new `Outcome` field because `Outcome` is an enum matched and
/// constructed at many sites across all three adapters and the runner; pairing
/// the two here leaves every `Outcome::` match untouched.
#[derive(Debug, Clone)]
pub struct Execution {
    pub outcome: Outcome,
    pub usage: Usage,
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
    /// The token usage the planning phase consumed, filled by the adapter
    /// (ADR-0008 D4). `Usage::default()` when the adapter does not capture it.
    pub usage: Usage,
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
    Limit(Option<String>),
}

/// A usage/rate limit hit during *planning* — before any plan artifact was
/// produced. Adapters return this (wrapped in `anyhow::Error`) instead of a
/// generic "no plan" failure, so the runner can route a plan-time limit into the
/// same wait-and-resume / stop-and-report machinery as an execute-time
/// [`Outcome::Limit`]. `reset` carries the parsed reset hint when present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanLimit {
    pub reset: Option<String>,
}

impl std::fmt::Display for PlanLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.reset {
            Some(r) => write!(f, "usage limit hit during planning (reset ~{r})"),
            None => write!(f, "usage limit hit during planning"),
        }
    }
}

impl std::error::Error for PlanLimit {}

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

    /// `<repo>/.ralphy/diagnosis.json` — where `ralphy init` persists the
    /// read-only repo-diagnosis report (ADR-0012 stage 2) after validating it
    /// against the [`crate::DiagnosisReport`] schema.
    pub fn diagnosis_path(&self) -> PathBuf {
        self.ralphy_dir().join("diagnosis.json")
    }

    /// `<repo>/.ralphy/init-state.json` — the `ralphy init` checkpoint
    /// (ADR-0012 stage 9): which onboarding stages completed and the numbers of
    /// issues already published, so a re-run skips done stages and never
    /// recreates published issues. Gitignored like everything else under
    /// `.ralphy/`.
    pub fn init_state_path(&self) -> PathBuf {
        self.ralphy_dir().join("init-state.json")
    }

    /// `<repo>/.ralphy/issues-draft.json` — the local preview draft a judgment
    /// session writes (ADR-0012 stage 8): the issues/milestone `ralphy init`
    /// summarizes for the dev to confirm before any of them are published to
    /// GitHub. Validated against the [`crate::IssuesDraft`] schema.
    pub fn issues_draft_path(&self) -> PathBuf {
        self.ralphy_dir().join("issues-draft.json")
    }

    /// `<repo>/.ralphy/handoffs.md` — handoffs collected from the closed
    /// issues the current one depends on, written by the runner before the
    /// plan pass and read by the planner as predecessor context.
    pub fn handoffs_path(&self) -> PathBuf {
        self.ralphy_dir().join("handoffs.md")
    }

    /// `<repo>/.ralphy/references.md` — the source title, state, and body of
    /// the issues named in the current issue's `## Blocked by` / `## Parent`
    /// sections, written by the runner before the plan pass so the planner reads
    /// the referenced spec rather than paraphrasing a `#N` mention. Refreshed
    /// (or removed) per issue like `handoffs.md`.
    pub fn references_path(&self) -> PathBuf {
        self.ralphy_dir().join("references.md")
    }

    /// `<repo>/.ralphy/knowledge` — the accumulated local knowledge cache:
    /// one `issue-<N>.md` per issue closed green, holding the environment
    /// facts and working commands mechanically extracted from its handoff at
    /// close. Unlike `handoffs.md` it is never cleared between issues — it
    /// grows across runs so future sessions can grep it instead of
    /// re-deriving environment procedures.
    pub fn knowledge_dir(&self) -> PathBuf {
        self.ralphy_dir().join("knowledge")
    }

    /// `<repo>/.ralphy/knowledge/issue-<N>.md` — a re-run of the same issue
    /// overwrites its note (the latest close supersedes).
    pub fn knowledge_path(&self, number: u64) -> PathBuf {
        self.knowledge_dir().join(format!("issue-{number}.md"))
    }

    /// `<repo>/.ralphy/knowledge/KNOWLEDGE.md` — the curated consolidation of
    /// the per-issue notes, written by a `ralphy consolidate` session and read
    /// FIRST by planner/executor sessions (the loose `issue-<N>.md` files are
    /// the not-yet-consolidated remainder).
    pub fn knowledge_file(&self) -> PathBuf {
        self.knowledge_dir().join("KNOWLEDGE.md")
    }

    /// `<repo>/.ralphy/knowledge/raw` — raw per-issue notes already folded
    /// into `KNOWLEDGE.md`, kept for provenance. The runner moves notes here
    /// after a successful consolidation; sessions don't read it.
    pub fn knowledge_raw_dir(&self) -> PathBuf {
        self.knowledge_dir().join("raw")
    }

    /// `<repo>/.ralphy/knowledge/citations.jsonl` — the cache's hit-rate log:
    /// one JSON line per green close recording which `KNOWLEDGE.md` /
    /// `handoffs.md` bullets that session's `**Knowledge used**` cited.
    /// Append-only and never archived, so the consolidation curator can judge
    /// "never cited across the last N closes" when pruning bullets.
    pub fn citations_path(&self) -> PathBuf {
        self.knowledge_dir().join("citations.jsonl")
    }

    /// `<repo>/.ralphy/runs/<stamp>` — per-run logs and scratch.
    pub fn run_dir(&self, stamp: &str) -> PathBuf {
        self.ralphy_dir().join("runs").join(stamp)
    }

    /// `<repo>/.ralphy/plugin` — the Claude Code plugin Ralphy materializes each
    /// run (the `reviewer` / `staged-plan` skills the prompts depend on). Passed
    /// to every `claude` call via `--plugin-dir`, so a run never depends on
    /// whatever skills happen to be installed globally on the machine.
    pub fn plugin_dir(&self) -> PathBuf {
        self.ralphy_dir().join("plugin")
    }

    /// `<repo>/.ralphy/settings.json` — the per-repo operator config store
    /// (ADR-0010). Gitignored like everything else under `.ralphy/`.
    pub fn settings_path(&self) -> PathBuf {
        self.ralphy_dir().join("settings.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_add_tokens_is_additive() {
        let mut a = Usage {
            input: 10,
            output: 1,
            cache_read: 100,
            cache_creation: 5,
            model: Some("claude-opus-4-8".into()),
        };
        let b = Usage {
            input: 20,
            output: 2,
            cache_read: 200,
            cache_creation: 7,
            model: Some("claude-sonnet-4-6".into()),
        };
        a.add_tokens(&b);
        assert_eq!(a.input, 30);
        assert_eq!(a.output, 3);
        assert_eq!(a.cache_read, 300);
        assert_eq!(a.cache_creation, 12);
        // `model` is untouched by summing — it stays the receiver's value.
        assert_eq!(a.model.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(a.total(), 345);
    }

    #[test]
    fn init_state_path_is_under_ralphy_dir() {
        let ws = Workspace::new("/some/repo");
        assert!(ws.init_state_path().starts_with(ws.ralphy_dir()));
        assert!(ws.init_state_path().ends_with("init-state.json"));
    }

    #[test]
    fn references_path_is_under_ralphy_dir() {
        let ws = Workspace::new("/some/repo");
        assert!(ws.references_path().starts_with(ws.ralphy_dir()));
        assert!(ws.references_path().ends_with("references.md"));
    }
}
