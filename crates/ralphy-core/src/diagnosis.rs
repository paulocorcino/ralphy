//! The repo-diagnosis schema (ADR-0012 stage 2). A read-only agent session
//! scans the target repo and returns a [`DiagnosisReport`] against this
//! Rust-defined schema; `ralphy init` then seeds its console Q&A from it. The
//! schema lives in core (next to [`crate::Issue`]/[`crate::Workspace`]) because
//! it is a domain artifact shared by the agent crate (which produces it) and the
//! cli (which consumes it).

use serde::{Deserialize, Serialize};

/// Whether the target repo already holds a project or is effectively empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoKind {
    /// No meaningful source/project content yet — a fresh or near-empty repo.
    Empty,
    /// An existing project with code, docs, or history to onboard.
    Existing,
}

/// The structured findings of one read-only repo-diagnosis session. Every field
/// is what the agent could determine by scanning the target repo as *data*; the
/// optional fields are `None` when the agent found no evidence either way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosisReport {
    /// Existing project vs empty repo.
    pub repo_kind: RepoKind,
    /// The primary language/build system (e.g. `"Rust / cargo"`), if detectable.
    pub language_build: Option<String>,
    /// Where the backlog lives (a file or dir path relative to the repo), if any.
    pub backlog_location: Option<String>,
    /// Milestone / roadmap / PRD documents found (paths relative to the repo).
    pub milestone_docs: Vec<String>,
    /// An existing agent-skills directory (e.g. `.agents`, `.cursor`, or a
    /// vendor-specific dotdir) path, if one is present.
    pub skills_dir: Option<String>,
    /// Whether the repo already carries a `CONTEXT.md` or any ADRs.
    pub has_context_or_adrs: bool,
    /// The git remote host (e.g. `"github.com"`), if an origin is configured.
    pub remote_host: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A canonical example report, mirroring what a diagnosis session emits for an
    /// existing GitHub-hosted Rust project. Used to pin the wire format.
    const SAMPLE_JSON: &str = r#"{
        "repo_kind": "existing",
        "language_build": "Rust / cargo",
        "backlog_location": "docs/backlog.md",
        "milestone_docs": ["docs/roadmap.md", "docs/prd/0001.md"],
        "skills_dir": ".agents",
        "has_context_or_adrs": true,
        "remote_host": "github.com"
    }"#;

    #[test]
    fn serde_round_trip() {
        let report = DiagnosisReport {
            repo_kind: RepoKind::Existing,
            language_build: Some("Rust / cargo".into()),
            backlog_location: Some("docs/backlog.md".into()),
            milestone_docs: vec!["docs/roadmap.md".into(), "docs/prd/0001.md".into()],
            skills_dir: Some(".agents".into()),
            has_context_or_adrs: true,
            remote_host: Some("github.com".into()),
        };
        let json = serde_json::to_string(&report).expect("serialize");
        let back: DiagnosisReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(report, back);
    }

    #[test]
    fn deserialize_sample_report() {
        let report: DiagnosisReport = serde_json::from_str(SAMPLE_JSON).expect("parse sample");
        assert_eq!(report.repo_kind, RepoKind::Existing);
        assert_eq!(report.remote_host.as_deref(), Some("github.com"));
    }
}
