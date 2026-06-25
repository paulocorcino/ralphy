//! Fetching an issue from the queue via the `gh` CLI. The queue *is* GitHub
//! issues, so this is a core (domain) concern — distinct from how any agent is
//! driven.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

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

impl From<GhIssue> for Issue {
    fn from(g: GhIssue) -> Self {
        Issue {
            number: g.number,
            title: g.title,
            body: g.body,
            labels: g.labels.into_iter().map(|l| l.name).collect(),
        }
    }
}

/// Parse `gh issue view --json` output into the domain [`Issue`].
pub fn parse_issue(json: &str) -> Result<Issue> {
    let g: GhIssue = serde_json::from_str(json).context("parsing `gh issue view` JSON")?;
    Ok(Issue::from(g))
}

/// Parse `gh issue list --json` output (a JSON array) into domain [`Issue`]s.
fn parse_issue_list(json: &str) -> Result<Vec<Issue>> {
    let raw: Vec<GhIssue> =
        serde_json::from_str(json).context("parsing `gh issue list` JSON array")?;
    Ok(raw.into_iter().map(Issue::from).collect())
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

/// List ALL open issues (no label filter), with bodies. Used to find the open
/// children of a retired bundle: issues whose `## Parent` section references
/// it. Label-agnostic on purpose — children often sit in `needs-triage` and
/// would be invisible to the label-filtered queue.
pub fn list_open_issues(repo_root: &Path) -> Result<Vec<Issue>> {
    let out = gh_output("gh issue list --state open", || {
        let mut cmd = gh(repo_root);
        cmd.args([
            "issue",
            "list",
            "--state",
            "open",
            "--json",
            "number,title,body,labels",
            "--limit",
            "200",
        ]);
        cmd
    })?;
    parse_issue_list(&String::from_utf8_lossy(&out.stdout))
}

/// A `gh` command rooted at `repo_root`. Ralphy is a global tool driven with
/// `--repo`, so the process cwd need not be the target repo; `gh` resolves the
/// repository from its working directory, so every GitHub call must be pinned to
/// `repo_root` or it would silently target the wrong repo (or none).
fn gh(repo_root: &Path) -> Command {
    let mut cmd = Command::new("gh");
    cmd.current_dir(repo_root);
    cmd
}

/// Total attempts for a transient-failing `gh` call (1 initial + 3 retries).
const GH_MAX_ATTEMPTS: u32 = 4;

/// Is a `gh` failure a transient GitHub edge / network blip (worth retrying)
/// rather than a real rejection (bad label, missing issue, auth — never retry)?
///
/// GitHub's gateway answers an overloaded request with a 5xx HTML page —
/// e.g. `non-200 OK status code: 504 Gateway Timeout` — which `gh` surfaces on
/// stderr. We match those markers (and the usual transport failures) so a momentary
/// blip is retried instead of aborting a run whose work has already landed.
fn is_transient_gh_failure(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    [
        "502",
        "503",
        "504",
        "bad gateway",
        "gateway timeout",
        "service unavailable",
        "timeout",
        "timed out",
        "connection reset",
        "connection refused",
        "could not resolve host",
        "tls handshake",
        "eof",
    ]
    .iter()
    .any(|m| s.contains(m))
}

/// Run a `gh` invocation (built fresh by `build` each attempt — `Command` is not
/// reusable) and return its captured output, retrying on a transient failure with
/// exponential backoff. `op` labels the call in the final error.
///
/// Every call routed through here is idempotent enough that a retried duplicate is
/// harmless next to losing the run: closing an already-closed issue, re-applying a
/// label, re-setting a body, or (worst case) a duplicate evidence comment after a
/// 504 whose write actually landed. A real rejection is not transient, so it bails
/// on the first attempt — no added latency on genuine errors.
fn gh_output(op: &str, mut build: impl FnMut() -> Command) -> Result<Output> {
    let mut backoff = Duration::from_secs(1);
    for attempt in 1..=GH_MAX_ATTEMPTS {
        let out = build()
            .output()
            .context("failed to spawn `gh` (is the GitHub CLI installed and on PATH?)")?;
        if out.status.success() {
            return Ok(out);
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        if attempt < GH_MAX_ATTEMPTS && is_transient_gh_failure(&stderr) {
            std::thread::sleep(backoff);
            backoff *= 2;
            continue;
        }
        bail!("`{op}` failed: {}", stderr.trim());
    }
    unreachable!("the final attempt returns Ok or bails");
}

/// Build the run queue from GitHub. `gh --label` is an AND filter, so query each
/// label separately and union the batches — an issue carrying ANY queue label
/// qualifies. Returns the deduped, ascending queue.
pub fn list_queue(labels: &[String], repo_root: &Path) -> Result<Vec<Issue>> {
    let mut batches = Vec::with_capacity(labels.len());
    for label in labels {
        let out = gh_output(&format!("gh issue list --label {label}"), || {
            let mut cmd = gh(repo_root);
            cmd.args([
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
            ]);
            cmd
        })?;
        batches.push(parse_issue_list(&String::from_utf8_lossy(&out.stdout))?);
    }
    Ok(build_queue(batches))
}

/// Close a green queue issue with a comment pointing at the run branch. The
/// issue's labels are left untouched — closing alone removes it from the queue
/// (the cycle); the human still merges the branch by hand.
pub fn close_issue(number: u64, comment: &str, repo_root: &Path) -> Result<()> {
    gh_output(&format!("gh issue close {number}"), || {
        let mut cmd = gh(repo_root);
        cmd.args(["issue", "close", &number.to_string(), "--comment", comment]);
        cmd
    })?;
    Ok(())
}

/// Edit a GitHub issue's body by sending the new content via stdin to
/// `gh issue edit <n> --body-file -`. Mirrors `close_issue`'s spawn/error pattern.
pub fn edit_issue_body(number: u64, body: &str, repo_root: &Path) -> Result<()> {
    use std::io::Write;
    use std::process::Stdio;

    // Re-setting the same body is idempotent, so mirror `gh_output`'s transient
    // retry here; the stdin pipe is why this can't route through that helper.
    let mut backoff = Duration::from_secs(1);
    for attempt in 1..=GH_MAX_ATTEMPTS {
        let mut child = gh(repo_root)
            .args(["issue", "edit", &number.to_string(), "--body-file", "-"])
            .stdin(Stdio::piped())
            // Capture stdout/stderr rather than inheriting them: `gh issue edit` prints
            // the issue URL to stdout on success, which would otherwise leak a loose
            // line into the console UI (and `out.stderr` below would be empty on error).
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
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
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        if attempt < GH_MAX_ATTEMPTS && is_transient_gh_failure(&stderr) {
            std::thread::sleep(backoff);
            backoff *= 2;
            continue;
        }
        bail!("`gh issue edit {number}` failed: {}", stderr.trim());
    }
    unreachable!("the final attempt returns Ok or bails");
}

/// Add a label to an issue via `gh issue edit <n> --add-label <label>`. When the
/// label does not exist in the repository yet, `gh` rejects the edit — so on
/// failure the label is created once (`gh label create`, best-effort) and the
/// edit retried, keeping first use on a fresh repo from failing.
pub fn add_label(number: u64, label: &str, repo_root: &Path) -> Result<()> {
    let edit = |root: &Path| -> Result<()> {
        gh_output(
            &format!("gh issue edit {number} --add-label {label}"),
            || {
                let mut cmd = gh(root);
                cmd.args(["issue", "edit", &number.to_string(), "--add-label", label]);
                cmd
            },
        )?;
        Ok(())
    };
    if edit(repo_root).is_ok() {
        return Ok(());
    }
    // The most common failure is a missing label; create it and retry once.
    let _ = gh(repo_root)
        .args([
            "label",
            "create",
            label,
            "--color",
            "D93F0B",
            "--description",
            "Ralphy: bundle issue awaiting split into child issues",
        ])
        .output();
    edit(repo_root)
}

/// Post a comment on a GitHub issue via `gh issue comment <n> --body <comment>`.
pub fn comment_issue(number: u64, comment: &str, repo_root: &Path) -> Result<()> {
    gh_output(&format!("gh issue comment {number}"), || {
        let mut cmd = gh(repo_root);
        cmd.args(["issue", "comment", &number.to_string(), "--body", comment]);
        cmd
    })?;
    Ok(())
}

/// Parse `{"comments":[{"body":"..."}]}` JSON from `gh issue view --json comments`
/// into the comment bodies, in thread order.
pub fn parse_issue_comments(json: &str) -> Result<Vec<String>> {
    #[derive(serde::Deserialize)]
    struct CommentJson {
        body: String,
    }
    #[derive(serde::Deserialize)]
    struct CommentsJson {
        #[serde(default)]
        comments: Vec<CommentJson>,
    }
    let c: CommentsJson =
        serde_json::from_str(json).context("parsing `gh issue view --json comments`")?;
    Ok(c.comments.into_iter().map(|c| c.body).collect())
}

/// Fetch an issue's comment bodies via `gh issue view <n> --json comments`.
pub fn issue_comments(number: u64, repo_root: &Path) -> Result<Vec<String>> {
    let out = gh_output(&format!("gh issue view {number} --json comments"), || {
        let mut cmd = gh(repo_root);
        cmd.args(["issue", "view", &number.to_string(), "--json", "comments"]);
        cmd
    })?;
    parse_issue_comments(&String::from_utf8_lossy(&out.stdout))
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
pub fn issue_is_closed(number: u64, repo_root: &Path) -> Result<bool> {
    let out = gh_output(&format!("gh issue view {number} --json state"), || {
        let mut cmd = gh(repo_root);
        cmd.args(["issue", "view", &number.to_string(), "--json", "state"]);
        cmd
    })?;
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
    fn transient_detector_matches_the_observed_504() {
        // The exact gateway response that aborted a run mid-evidence-comment.
        let stderr = r#"failed to run git: non-200 OK status code: 504 Gateway Timeout body: "<!DOCTYPE html>...""#;
        assert!(is_transient_gh_failure(stderr));
    }

    #[test]
    fn transient_detector_matches_other_edge_and_transport_blips() {
        for s in [
            "non-200 OK status code: 502 Bad Gateway",
            "503 Service Unavailable",
            "request timed out",
            "connection reset by peer",
            "could not resolve host: api.github.com",
        ] {
            assert!(is_transient_gh_failure(s), "expected transient: {s}");
        }
    }

    #[test]
    fn transient_detector_rejects_real_rejections() {
        // Real failures must bail on the first attempt — no pointless retries.
        for s in [
            "could not add label: 'needs-split' not found",
            "GraphQL: Could not resolve to an Issue with the number of 9999",
            "gh: Not Found (HTTP 404)",
            "authentication required",
        ] {
            assert!(!is_transient_gh_failure(s), "expected non-transient: {s}");
        }
    }

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
    fn from_ghissue_maps_all_fields() {
        let g = GhIssue {
            number: 42,
            title: "some title".into(),
            body: "some body".into(),
            labels: vec![
                GhLabel { name: "AFK".into() },
                GhLabel { name: "bug".into() },
            ],
        };
        let issue = Issue::from(g);
        assert_eq!(issue.number, 42);
        assert_eq!(issue.title, "some title");
        assert_eq!(issue.body, "some body");
        assert_eq!(issue.labels, vec!["AFK", "bug"]);
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
