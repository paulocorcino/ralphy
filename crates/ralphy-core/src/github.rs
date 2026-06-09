//! Fetching an issue from the queue via the `gh` CLI. The queue *is* GitHub
//! issues, so this is a core (domain) concern — distinct from how any agent is
//! driven.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::Issue;

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

/// Parse `gh issue list --json` output (a JSON array) into domain [`Issue`]s.
fn parse_issue_list(json: &str) -> Result<Vec<Issue>> {
    let raw: Vec<GhIssue> =
        serde_json::from_str(json).context("parsing `gh issue list` JSON array")?;
    Ok(raw
        .into_iter()
        .map(|g| Issue {
            number: g.number,
            title: g.title,
            body: g.body,
            labels: g.labels.into_iter().map(|l| l.name).collect(),
        })
        .collect())
}

/// Flatten per-label issue batches into one queue: union all batches, dedupe by
/// `number` (an issue carrying several queue labels appears once), and sort
/// ascending by number so the queue is worked in task sequence.
pub fn build_queue(batches: Vec<Vec<Issue>>) -> Vec<Issue> {
    let mut by_number: BTreeMap<u64, Issue> = BTreeMap::new();
    for batch in batches {
        for issue in batch {
            by_number.entry(issue.number).or_insert(issue);
        }
    }
    by_number.into_values().collect()
}

/// Build the run queue from GitHub. `gh --label` is an AND filter, so query each
/// label separately and union the batches — an issue carrying ANY queue label
/// qualifies. Returns the deduped, ascending queue.
pub fn list_queue(labels: &[String]) -> Result<Vec<Issue>> {
    let mut batches = Vec::with_capacity(labels.len());
    for label in labels {
        let out = Command::new("gh")
            .args([
                "issue",
                "list",
                "--label",
                label,
                "--state",
                "open",
                "--json",
                "number,title,body,labels",
                "--limit",
                "100",
            ])
            .output()
            .context("failed to spawn `gh` (is the GitHub CLI installed and on PATH?)")?;
        if !out.status.success() {
            bail!(
                "`gh issue list --label {label}` failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        batches.push(parse_issue_list(&String::from_utf8_lossy(&out.stdout))?);
    }
    Ok(build_queue(batches))
}

/// Close a green queue issue with a comment pointing at the run branch. The
/// issue's labels are left untouched — closing alone removes it from the queue
/// (the cycle); the human still merges the branch by hand.
pub fn close_issue(number: u64, comment: &str) -> Result<()> {
    let out = Command::new("gh")
        .args(["issue", "close", &number.to_string(), "--comment", comment])
        .output()
        .context("failed to spawn `gh` (is the GitHub CLI installed and on PATH?)")?;
    if !out.status.success() {
        bail!(
            "`gh issue close {number}` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Edit a GitHub issue's body by sending the new content via stdin to
/// `gh issue edit <n> --body-file -`. Mirrors `close_issue`'s spawn/error pattern.
pub fn edit_issue_body(number: u64, body: &str) -> Result<()> {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = Command::new("gh")
        .args(["issue", "edit", &number.to_string(), "--body-file", "-"])
        .stdin(Stdio::piped())
        .spawn()
        .context("failed to spawn `gh` (is the GitHub CLI installed and on PATH?)")?;

    // Store the write result rather than short-circuiting with `?`: dropping
    // `child` without calling `wait` would leave a zombie process on write failure.
    let mut stdin = child.stdin.take().expect("stdin was piped");
    let write_result = stdin.write_all(body.as_bytes());
    drop(stdin); // close stdin (EOF) before waiting

    let out = child
        .wait_with_output()
        .context("waiting for `gh issue edit`")?;

    write_result.context("writing body to `gh` stdin")?;
    if !out.status.success() {
        bail!(
            "`gh issue edit {number}` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Post a comment on a GitHub issue via `gh issue comment <n> --body <comment>`.
pub fn comment_issue(number: u64, comment: &str) -> Result<()> {
    let out = Command::new("gh")
        .args(["issue", "comment", &number.to_string(), "--body", comment])
        .output()
        .context("failed to spawn `gh` (is the GitHub CLI installed and on PATH?)")?;
    if !out.status.success() {
        bail!(
            "`gh issue comment {number}` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Parse `{"state":"CLOSED"}` / `{"state":"OPEN"}` JSON from `gh issue view --json state`.
fn parse_issue_state(json: &str) -> Result<bool> {
    #[derive(serde::Deserialize)]
    struct StateJson {
        state: String,
    }
    let s: StateJson =
        serde_json::from_str(json).context("parsing `gh issue view --json state`")?;
    Ok(s.state == "CLOSED")
}

/// Return `true` when the given issue number is closed, by running
/// `gh issue view <n> --json state`.
pub fn issue_is_closed(number: u64) -> Result<bool> {
    let out = Command::new("gh")
        .args(["issue", "view", &number.to_string(), "--json", "state"])
        .output()
        .context("failed to spawn `gh` (is the GitHub CLI installed and on PATH?)")?;
    if !out.status.success() {
        bail!(
            "`gh issue view {number} --json state` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    parse_issue_state(&String::from_utf8_lossy(&out.stdout))
}

/// Parse a `docs/agents/triage-labels.md` table row. Scans `doc` for
/// `|`-delimited rows, strips backticks, trims each cell, and returns cell[2]
/// when cell[1] == `canonical`. Ports `Resolve-TriageLabels`'s row parsing.
pub fn parse_triage_mapping(doc: &str, canonical: &str) -> Option<String> {
    for line in doc.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line
            .split('|')
            .map(|c| c.trim().trim_matches('`').trim())
            .collect();
        // After split on '|', a row like `| a | b |` yields
        // ["", "a", "b", ""] — cell[1] and cell[2] are the first and
        // second data columns. Skip separator rows (|---|---|).
        let is_separator = |s: &str| s.trim_matches(['-', ':', ' ']).is_empty() && !s.is_empty();
        if cells.len() >= 4 && cells[1] == canonical && !is_separator(cells[2]) {
            let mapped = cells[2].to_string();
            if !mapped.is_empty() {
                return Some(mapped);
            }
        }
    }
    None
}

/// Build the effective queue label set. If `explicit` is non-empty, return it
/// verbatim (explicit overrides everything). Otherwise start from the defaults
/// `["ready-for-agent", "AFK"]`, read `docs/agents/triage-labels.md` under
/// `repo_root` (absent is fine), and append the `parse_triage_mapping` result
/// for `"ready-for-agent"`, deduped. Ports `Resolve-TriageLabels`.
pub fn resolve_queue_labels(explicit: &[String], repo_root: &Path) -> Vec<String> {
    if !explicit.is_empty() {
        return explicit.to_vec();
    }
    let mut labels: Vec<String> = vec!["ready-for-agent".into(), "AFK".into()];
    let triage_path = repo_root
        .join("docs")
        .join("agents")
        .join("triage-labels.md");
    if let Ok(doc) = std::fs::read_to_string(&triage_path) {
        if let Some(mapped) = parse_triage_mapping(&doc, "ready-for-agent") {
            if !labels.contains(&mapped) {
                labels.push(mapped);
            }
        }
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_issue_state_closed() {
        assert!(parse_issue_state(r#"{"state":"CLOSED"}"#).unwrap());
    }

    #[test]
    fn parse_issue_state_open() {
        assert!(!parse_issue_state(r#"{"state":"OPEN"}"#).unwrap());
    }

    #[test]
    fn parse_triage_mapping_finds_mapped_label() {
        // Two-column format: | canonical | mapped |
        let doc = "# Triage Labels\n\
                   | Canonical | Mapped |\n\
                   |-----------|--------|\n\
                   | `ready-for-agent` | `afk-ready` |\n\
                   | `other` | `other-mapped` |\n";
        assert_eq!(
            parse_triage_mapping(doc, "ready-for-agent"),
            Some("afk-ready".into())
        );
    }

    #[test]
    fn parse_triage_mapping_returns_none_when_absent() {
        let doc = "| `other` | `other-mapped` |\n";
        assert_eq!(parse_triage_mapping(doc, "ready-for-agent"), None);
    }

    #[test]
    fn parse_triage_mapping_returns_none_on_empty_doc() {
        assert_eq!(parse_triage_mapping("", "ready-for-agent"), None);
    }

    #[test]
    fn resolve_queue_labels_explicit_set_returned_verbatim() {
        let explicit = vec!["my-label".to_string(), "other-label".to_string()];
        let result = resolve_queue_labels(&explicit, Path::new("/nonexistent"));
        assert_eq!(result, explicit, "explicit set should be returned verbatim");
    }

    #[test]
    fn resolve_queue_labels_defaults_without_triage_file() {
        let result = resolve_queue_labels(&[], Path::new("/nonexistent/repo"));
        assert_eq!(result, vec!["ready-for-agent", "AFK"]);
    }

    #[test]
    fn resolve_queue_labels_appends_mapped_label_from_triage_file() {
        let dir = std::env::temp_dir().join(format!("ralphy-triage-{}", std::process::id()));
        let docs_dir = dir.join("docs").join("agents");
        std::fs::create_dir_all(&docs_dir).unwrap();
        let triage_content = "| Canonical | Mapped |\n\
                              |-----------|--------|\n\
                              | `ready-for-agent` | `afk-extended` |\n";
        std::fs::write(docs_dir.join("triage-labels.md"), triage_content).unwrap();

        let result = resolve_queue_labels(&[], &dir);
        assert_eq!(result, vec!["ready-for-agent", "AFK", "afk-extended"]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_queue_labels_dedupes_mapped_label() {
        let dir = std::env::temp_dir().join(format!("ralphy-triage-dedup-{}", std::process::id()));
        let docs_dir = dir.join("docs").join("agents");
        std::fs::create_dir_all(&docs_dir).unwrap();
        // Mapping resolves to "AFK" which is already in defaults.
        let triage_content = "| `ready-for-agent` | `AFK` |\n";
        std::fs::write(docs_dir.join("triage-labels.md"), triage_content).unwrap();

        let result = resolve_queue_labels(&[], &dir);
        // "AFK" should appear only once.
        assert_eq!(result, vec!["ready-for-agent", "AFK"]);

        std::fs::remove_dir_all(&dir).ok();
    }

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

    fn issue(number: u64) -> Issue {
        Issue {
            number,
            title: format!("issue {number}"),
            body: String::new(),
            labels: vec![],
        }
    }

    #[test]
    fn build_queue_unions_dedupes_and_sorts() {
        // Two labels, one issue (#5) shared across both, out of order within each.
        let ready = vec![issue(9), issue(5)];
        let afk = vec![issue(5), issue(2)];
        let queue = build_queue(vec![ready, afk]);
        let numbers: Vec<u64> = queue.iter().map(|i| i.number).collect();
        assert_eq!(
            numbers,
            vec![2, 5, 9],
            "union, deduped by number, ascending"
        );
    }

    #[test]
    fn build_queue_keeps_first_seen_for_duplicates() {
        // The shared issue's first occurrence wins, but identity (number) is what
        // matters — assert it appears exactly once regardless of batch order.
        let queue = build_queue(vec![vec![issue(3)], vec![issue(3)], vec![issue(1)]]);
        let numbers: Vec<u64> = queue.iter().map(|i| i.number).collect();
        assert_eq!(numbers, vec![1, 3]);
    }

    #[test]
    fn parse_issue_list_reads_array() {
        let json =
            r#"[{"number":2,"title":"b","labels":[{"name":"AFK"}]},{"number":1,"title":"a"}]"#;
        let list = parse_issue_list(json).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].number, 2);
        assert_eq!(list[0].labels, vec!["AFK"]);
        assert_eq!(list[1].number, 1);
    }
}
