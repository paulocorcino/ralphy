//! Fetching an issue from the queue via the `gh` CLI. The queue *is* GitHub
//! issues, so this is a core (domain) concern — distinct from how any agent is
//! driven.

use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::Issue;

/// Fetch one issue as raw `gh` JSON (so the CLI can persist it byte-for-byte to
/// `.ralphy/issue.json`, which the planner reads).
pub fn fetch_issue_json(number: u64) -> Result<String> {
    let out = Command::new("gh")
        .args([
            "issue",
            "view",
            &number.to_string(),
            "--json",
            "number,title,body,labels",
        ])
        .output()
        .context("failed to spawn `gh` (is the GitHub CLI installed and on PATH?)")?;
    if !out.status.success() {
        bail!(
            "`gh issue view {number}` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

#[derive(Deserialize)]
struct GhLabel {
    name: String,
}

#[derive(Deserialize)]
struct GhIssue {
    number: u64,
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    labels: Vec<GhLabel>,
}

/// Parse `gh issue view --json` output into the domain [`Issue`].
pub fn parse_issue(json: &str) -> Result<Issue> {
    let g: GhIssue = serde_json::from_str(json).context("parsing `gh issue view` JSON")?;
    Ok(Issue {
        number: g.number,
        title: g.title,
        body: g.body,
        labels: g.labels.into_iter().map(|l| l.name).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_issue_with_labels() {
        let json =
            r#"{"number":7,"title":"t","body":"b","labels":[{"name":"AFK"},{"name":"bug"}]}"#;
        let issue = parse_issue(json).unwrap();
        assert_eq!(issue.number, 7);
        assert_eq!(issue.labels, vec!["AFK", "bug"]);
    }

    #[test]
    fn tolerates_missing_body_and_labels() {
        let issue = parse_issue(r#"{"number":1,"title":"t"}"#).unwrap();
        assert_eq!(issue.body, "");
        assert!(issue.labels.is_empty());
    }
}
