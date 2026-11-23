//! The deny rules the guard matches against the text of a command.

use std::sync::LazyLock;

use regex::Regex;

/// The group of a deny rule. A workflow rule exists because a run's
/// orchestrator owns the branch, the push and the pull request.
// ADR-0093 D1: a new rule is a safety rule unless the ADR is amended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Group {
    Workflow,
    Safety,
}

/// Whether the guard applies its workflow rules to a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowRules {
    Apply,
    /// A console whose operator lets the agent run them (ADR-0093 D3).
    Skip,
}

/// The Bash deny-list: each `(group, regex, why)` blocks a command.
/// Every rule is case-insensitive (`(?i)`), as PowerShell is. Recursive deletes
/// are not here: [`super::delete::first_disallowed_target`] judges them by their paths.
static BASH_DENY_RULES: LazyLock<Vec<(Group, Regex, &'static str)>> = LazyLock::new(|| {
    use Group::{Safety, Workflow};
    [
        (
            Workflow,
            r"(?i)\bgit\s+push\b",
            "pushing is the orchestrator's job, not the agent's",
        ),
        (
            Safety,
            r"(?i)\bgit\s+reset\s+--hard\b",
            "hard reset can destroy uncommitted work",
        ),
        (
            Safety,
            r"(?i)\bgit\s+clean\b",
            "git clean deletes untracked files",
        ),
        (
            Safety,
            r"(?i)\bgit\s+rebase\b",
            "history rewrite is not allowed in the loop",
        ),
        (
            Workflow,
            r"(?i)\bgit\s+(checkout|switch)\b",
            "the agent must stay on the run branch the orchestrator created",
        ),
        (
            Workflow,
            r"(?i)\bgit\s+worktree\b",
            "worktrees are the orchestrator's business, not the agent's",
        ),
        (
            Workflow,
            // Global flags (`-R o/r`, `--repo=o/r`) may sit between `gh` and `pr`.
            concat!(
                r"(?i)\bgh(?:\s+-\S+(?:\s+[^-\s]\S*)?)*\s+pr\s+",
                r"(create|edit|ready|reopen|review|comment|merge|close)\b",
            ),
            "writing to a pull request is the operator's job",
        ),
        (
            Safety,
            r"(?i)\bgh\s+(release|repo|workflow|secret|auth)\b",
            "repo/release/workflow/secret/auth ops are out of scope",
        ),
        (
            Safety,
            r"(?i)\bcargo\s+publish\b",
            "publishing crates is out of scope",
        ),
        (
            Safety,
            // `format` only as a command name (start of command or after a
            // separator) — `--format json` / `--format=%H` flags are benign.
            r"(?i)(?:^|[;&|]\s*)format(?:\.com)?\s|(?i)\b(?:mkfs|diskpart)\b",
            "disk-level command is blocked",
        ),
        (
            Safety,
            r"(?i)\bcurl\b.*\|\s*(sh|bash|pwsh|powershell)",
            "piping a download into a shell is blocked",
        ),
        (
            Safety,
            r"(?i)iwr\b.*\|\s*iex|(?i)Invoke-Expression\b",
            "remote code execution is blocked",
        ),
    ]
    .into_iter()
    .map(|(group, rx, why)| (group, Regex::new(rx).expect("valid guard regex"), why))
    .collect()
});

/// The reason of the first deny rule `text` matches, of the groups that apply.
pub(super) fn denied_by_rules(text: &str, workflow: WorkflowRules) -> Option<String> {
    BASH_DENY_RULES
        .iter()
        .filter(|(group, ..)| *group == Group::Safety || workflow == WorkflowRules::Apply)
        .find(|(_, re, _)| re.is_match(text))
        .map(|(.., why)| (*why).to_owned())
}

#[cfg(test)]
mod tests {
    use super::{Group, BASH_DENY_RULES};

    /// ADR-0093 D1 names four workflow rules; a fifth needs the ADR first.
    #[test]
    fn the_workflow_rules_are_the_four_the_adr_names() {
        let workflow = BASH_DENY_RULES
            .iter()
            .filter(|(g, ..)| *g == Group::Workflow);
        let reasons: Vec<&str> = workflow.map(|(.., why)| *why).collect();
        assert_eq!(
            reasons,
            [
                "pushing is the orchestrator's job, not the agent's",
                "the agent must stay on the run branch the orchestrator created",
                "worktrees are the orchestrator's business, not the agent's",
                "writing to a pull request is the operator's job",
            ]
        );
    }
}
