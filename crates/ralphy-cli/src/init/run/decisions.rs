//! The pure decisions the wizard takes from an operator's answer: what a
//! yes/no or a typed choice means for each prompt, which agent runs the
//! diagnosis and which model it pins.

use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::init::gate::Agent;

/// The git-safety decision for a (clean?, answer) pair. Pure: the impure shell in
/// [`super::run`] probes the tree and reads the answer, then acts on this verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CommitDecision {
    NothingToCommit,
    Commit,
    Abort(String),
}

/// Map (is_clean, answer) to a [`CommitDecision`]. A clean tree never commits; a
/// dirty tree commits on the recommended default (empty/yes/y — the prompt shows
/// `[Y/n]`, so accepting it commits the snapshot) and aborts only on an explicit
/// decline, which stops init before any branch or scaffold write.
pub(super) fn commit_decision(is_clean: bool, answer: &str) -> CommitDecision {
    if is_clean {
        return CommitDecision::NothingToCommit;
    }

    match answer.trim().to_ascii_lowercase().as_str() {
        "" | "y" | "yes" => CommitDecision::Commit,
        _ => CommitDecision::Abort(
            "ralphy init aborted: a snapshot commit is required to isolate init's changes".into(),
        ),
    }
}

/// The branch decision for a (current, answer) pair. Pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BranchDecision {
    Create(String),
    Stay,
}

/// Map an answer to a [`BranchDecision`]. Empty/yes/y (the recommended default) →
/// create `ralphy/init`; no/n → stay on the current branch.
pub(super) fn branch_decision(_current: &str, answer: &str) -> BranchDecision {
    match answer.trim().to_ascii_lowercase().as_str() {
        "" | "y" | "yes" => BranchDecision::Create("ralphy/init".into()),
        _ => BranchDecision::Stay,
    }
}

/// The bootstrap decision when the target directory is not yet a git repository.
/// The prompt shows `[Y/n]`, so the recommended default (empty/`y`/`yes`) creates
/// the repo (`git init` + `gh repo create`); any other answer declines and init
/// keeps the original "not a git repository" error. Pure, mirrors [`labels_decision`].
pub fn create_repo_decision(answer: &str) -> bool {
    matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    )
}

/// Resolve the repo-visibility answer to whether the new GitHub repo is private.
/// The prompt shows `[Y/n]`, so the default (empty/`y`/`yes`) is private — the
/// safer default for a freshly created repo; an explicit `n`/`no` makes it public.
/// Pure.
pub fn private_visibility_decision(answer: &str) -> bool {
    !matches!(answer.trim().to_ascii_lowercase().as_str(), "n" | "no")
}

/// Derive the GitHub repo name from the (absolute) target directory: its final
/// path segment, falling back to `repo` when the path has no usable base name
/// (e.g. a drive/filesystem root). Pure over its input.
pub fn repo_name_from_path(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("repo")
        .to_string()
}

/// The label-creation decision: empty / `y` / `yes` → proceed (the default is
/// recommended since stage 7 is idempotent); `n` / anything else → skip.
pub fn labels_decision(answer: &str) -> bool {
    matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    )
}

/// Choose which agent drives the AI judgment steps. An explicit `--agent` must be
/// logged in (else a hard error names the logged-in set); with no flag, the first
/// logged-in agent in gate order (claude → codex → opencode) is used. The gate has
/// already guaranteed `logged_in` is non-empty before this is called.
pub(super) fn select_agent(requested: Option<Agent>, logged_in: &[Agent]) -> Result<Agent> {
    match requested {
        Some(a) if logged_in.contains(&a) => Ok(a),
        Some(a) => bail!(
            "ralphy init: --agent {} is not logged in (logged in: {})",
            a.cli_name(),
            if logged_in.is_empty() {
                "none".to_string()
            } else {
                logged_in
                    .iter()
                    .map(|x| x.cli_name())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ),
        None => logged_in
            .first()
            .copied()
            .context("no logged-in agent available (the environment gate should have caught this)"),
    }
}

/// The model init pins for the AI judgment steps (diagnosis + issue drafting).
/// Claude gets `sonnet` (these steps don't warrant opus, and pinning keeps init
/// off the dev's personal `claude` default); other agents keep their CLI default
/// (`None`). Pure so the mapping unit-tests.
pub(super) fn init_model_for(agent: Agent) -> Option<&'static str> {
    match agent {
        Agent::Claude => Some("sonnet"),
        Agent::Codex
        | Agent::Copilot
        | Agent::Cursor
        | Agent::Gemini
        | Agent::Opencode
        | Agent::Kimi => None,
    }
}

#[cfg(test)]
mod tests;
