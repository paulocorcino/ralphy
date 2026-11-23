//! The deny rules the guard matches against the text of a command.

use std::sync::LazyLock;

use regex::Regex;

/// The Bash deny-list: each `(regex, why)` pair blocks a destructive command.
/// Every rule is case-insensitive (`(?i)`), as PowerShell is. Recursive deletes
/// are not here: [`super::delete::first_disallowed_target`] judges them by their paths.
static BASH_DENY_RULES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (
            r"(?i)\bgit\s+push\b",
            "pushing is the orchestrator's job, not the agent's",
        ),
        (
            r"(?i)\bgit\s+reset\s+--hard\b",
            "hard reset can destroy uncommitted work",
        ),
        (r"(?i)\bgit\s+clean\b", "git clean deletes untracked files"),
        (
            r"(?i)\bgit\s+rebase\b",
            "history rewrite is not allowed in the loop",
        ),
        (
            r"(?i)\bgit\s+(checkout|switch)\b",
            "the agent must stay on the run branch the orchestrator created",
        ),
        (
            r"(?i)\bgit\s+worktree\b",
            "worktrees are the orchestrator's business, not the agent's",
        ),
        (
            // Global flags (`-R o/r`, `--repo=o/r`) may sit between `gh` and `pr`.
            concat!(
                r"(?i)\bgh(?:\s+-\S+(?:\s+[^-\s]\S*)?)*\s+pr\s+",
                r"(create|edit|ready|reopen|review|comment|merge|close)\b",
            ),
            "writing to a pull request is the operator's job",
        ),
        (
            r"(?i)\bgh\s+(release|repo|workflow|secret|auth)\b",
            "repo/release/workflow/secret/auth ops are out of scope",
        ),
        (
            r"(?i)\bcargo\s+publish\b",
            "publishing crates is out of scope",
        ),
        (
            // `format` only as a command name (start of command or after a
            // separator) — `--format json` / `--format=%H` flags are benign.
            r"(?i)(?:^|[;&|]\s*)format(?:\.com)?\s|(?i)\b(?:mkfs|diskpart)\b",
            "disk-level command is blocked",
        ),
        (
            r"(?i)\bcurl\b.*\|\s*(sh|bash|pwsh|powershell)",
            "piping a download into a shell is blocked",
        ),
        (
            r"(?i)iwr\b.*\|\s*iex|(?i)Invoke-Expression\b",
            "remote code execution is blocked",
        ),
    ]
    .into_iter()
    .map(|(rx, why)| (Regex::new(rx).expect("valid guard regex"), why))
    .collect()
});

/// The reason of the first deny rule `text` matches.
pub(super) fn denied_by_rules(text: &str) -> Option<String> {
    BASH_DENY_RULES
        .iter()
        .find(|(re, _)| re.is_match(text))
        .map(|(_, why)| (*why).to_owned())
}
