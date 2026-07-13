//! Issue CRUD: fetching, creating, editing, closing, labeling, and commenting
//! on GitHub issues via the `gh` CLI.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::github::client::{gh, gh_output, gh_stdin};
use crate::Issue;

#[derive(Deserialize)]
struct GhLabel {
    name: String,
}

#[derive(Deserialize)]
struct GhAssignee {
    login: String,
}

#[derive(Deserialize)]
struct GhIssueMeta {
    number: u64,
    #[serde(default)]
    assignees: Vec<GhAssignee>,
    #[serde(default, rename = "stateReason")]
    state_reason: Option<String>,
}

/// Per-issue metadata the board fold needs (assignees, close reason) that the
/// domain [`Issue`] does not carry — see [`list_issue_meta`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct IssueMeta {
    pub number: u64,
    pub assignees: Vec<String>,
    pub state_reason: Option<String>,
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
            // `gh issue list`/`view` here fetch number,title,body,labels only —
            // comments are filled later, per selected issue, by the runner.
            comments: Vec::new(),
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

/// Parse `gh issue list --json number,assignees,stateReason` output (a JSON
/// array) into [`IssueMeta`]. `stateReason` is `null` for OPEN issues (gh only
/// populates it on CLOSED); kept as `Option<String>` rather than assumed absent.
pub fn parse_issue_meta_list(json: &str) -> Result<Vec<IssueMeta>> {
    let raw: Vec<GhIssueMeta> =
        serde_json::from_str(json).context("parsing `gh issue list` meta JSON array")?;
    Ok(raw
        .into_iter()
        .map(|g| IssueMeta {
            number: g.number,
            assignees: g.assignees.into_iter().map(|a| a.login).collect(),
            state_reason: g.state_reason,
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

/// Build the full `gh issue list` argv for one queue label. When `assignee` is
/// `Some`, appends `--assignee <value>` so gh restricts the batch to issues the
/// login is among the assignees of; `None` leaves the query unfiltered. Kept pure
/// (no `Command`, no network) so the `--assignee` append is unit-testable,
/// mirroring the `parse_issue_list` seam.
pub fn queue_list_args(label: &str, assignee: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "issue".to_string(),
        "list".to_string(),
        "--label".to_string(),
        label.to_string(),
        "--state".to_string(),
        "open".to_string(),
        "--json".to_string(),
        "number,title,body,labels".to_string(),
        "--limit".to_string(),
        "100".to_string(),
    ];
    if let Some(a) = assignee {
        args.push("--assignee".to_string());
        args.push(a.to_string());
    }
    args
}

/// Build the run queue from GitHub. `gh --label` is an AND filter, so query each
/// label separately and union the batches — an issue carrying ANY queue label
/// qualifies. When `assignee` is `Some`, each batch is additionally scoped to
/// issues the login is among the assignees of. Returns the deduped, ascending
/// queue.
pub fn list_queue(
    labels: &[String],
    assignee: Option<&str>,
    repo_root: &Path,
) -> Result<Vec<Issue>> {
    let mut batches = Vec::with_capacity(labels.len());
    for label in labels {
        let out = gh_output(&format!("gh issue list --label {label}"), || {
            let mut cmd = gh(repo_root);
            cmd.args(queue_list_args(label, assignee));
            cmd
        })?;
        batches.push(parse_issue_list(&String::from_utf8_lossy(&out.stdout))?);
    }
    Ok(build_queue(batches))
}

/// Build the run queue's per-issue metadata (assignees, stateReason) that the
/// board fold needs but [`Issue`] does not carry. Mirrors [`list_queue`]: one
/// batched `gh issue list` spawn per label (never per-issue), union-deduped by
/// number so an issue carrying several queue labels appears once.
pub fn list_issue_meta(
    labels: &[String],
    assignee: Option<&str>,
    repo_root: &Path,
) -> Result<Vec<IssueMeta>> {
    let mut by_number: BTreeMap<u64, IssueMeta> = BTreeMap::new();
    for label in labels {
        let mut args = vec![
            "issue".to_string(),
            "list".to_string(),
            "--label".to_string(),
            label.to_string(),
            "--state".to_string(),
            "open".to_string(),
            "--json".to_string(),
            "number,assignees,stateReason".to_string(),
            "--limit".to_string(),
            "100".to_string(),
        ];
        if let Some(a) = assignee {
            args.push("--assignee".to_string());
            args.push(a.to_string());
        }
        let out = gh_output(&format!("gh issue list --label {label} (meta)"), || {
            let mut cmd = gh(repo_root);
            cmd.args(&args);
            cmd
        })?;
        let batch = parse_issue_meta_list(&String::from_utf8_lossy(&out.stdout))?;
        for meta in batch {
            by_number.entry(meta.number).or_insert(meta);
        }
    }
    Ok(by_number.into_values().collect())
}

/// Resolve an assignee filter value to the concrete GitHub login it scopes the
/// queue to (ADR-0021 §5's `assignee_filter` resolver). A non-`@me` string is
/// already the wire login, so it is returned verbatim with NO `gh` call; only the
/// literal `@me` is resolved via `gh api user --jq .login` through the shared
/// [`gh_output`] transient-retry wrapper.
///
/// Contract: at most ONE `gh` invocation per call, and only on the `@me` path.
/// `bail!`s when the resolved login is empty (a `gh api user` that returned no
/// `.login`).
pub fn resolve_login(assignee: &str, repo_root: &Path) -> Result<String> {
    if assignee != "@me" {
        return Ok(assignee.to_string());
    }
    let out = gh_output("gh api user", || {
        let mut cmd = gh(repo_root);
        cmd.args(["api", "user", "--jq", ".login"]);
        cmd
    })?;
    let login = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if login.is_empty() {
        bail!("`gh api user --jq .login` returned an empty login for @me");
    }
    Ok(login)
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
    gh_stdin(&format!("gh issue edit {number}"), body.as_bytes(), || {
        let mut c = gh(repo_root);
        c.args(["issue", "edit", &number.to_string(), "--body-file", "-"]);
        c
    })?;
    Ok(())
}

/// Parse the issue number from a `gh issue create` success line. `gh` prints the
/// new issue's URL (e.g. `https://github.com/owner/repo/issues/42`) on stdout; the
/// number is the trailing path segment. Tolerant of surrounding whitespace and a
/// trailing slash.
fn parse_issue_url(stdout: &str) -> Result<u64> {
    for line in stdout.lines().rev() {
        let line = line.trim().trim_end_matches('/');
        if let Some(last) = line.rsplit('/').next() {
            if let Ok(n) = last.parse::<u64>() {
                return Ok(n);
            }
        }
    }
    bail!("could not parse an issue number from `gh issue create` output: {stdout:?}");
}

/// Create a GitHub issue via `gh issue create`, piping the body on stdin
/// (`--body-file -`) like [`edit_issue_body`] so multi-line bodies survive. Each
/// label is passed with a repeated `--label`; `milestone` (a milestone *name*,
/// which `gh issue create --milestone` resolves) links the issue when `Some` — the
/// milestone must already exist (see [`crate::github::create_milestone`]). Returns the created
/// issue's number, parsed from the printed URL. ADR-0012 stage 8.
///
/// Mirrors [`edit_issue_body`]'s spawn/stdin/transient-retry shape. A retried
/// duplicate after a 504-whose-write-landed is the one non-idempotent edge here;
/// it is accepted for the same reason the rest of this module retries — losing the
/// run is worse than a rare duplicate the dev can delete.
pub fn create_issue(
    repo_root: &Path,
    title: &str,
    body: &str,
    labels: &[String],
    milestone: Option<&str>,
) -> Result<u64> {
    let out = gh_stdin(
        &format!("gh issue create ({title})"),
        body.as_bytes(),
        || {
            let mut args: Vec<String> = vec![
                "issue".into(),
                "create".into(),
                "--title".into(),
                title.into(),
                "--body-file".into(),
                "-".into(),
            ];
            for label in labels {
                args.push("--label".into());
                args.push(label.clone());
            }
            if let Some(ms) = milestone {
                args.push("--milestone".into());
                args.push(ms.to_string());
            }
            let mut c = gh(repo_root);
            c.args(&args);
            c
        },
    )?;
    parse_issue_url(&String::from_utf8_lossy(&out.stdout))
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

/// Remove a label from an issue via `gh issue edit <n> --remove-label <label>`.
/// Unlike [`add_label`] there is no create-on-missing retry: removing a label
/// that is not present is a plain no-op-or-error, never a first-use bootstrap.
pub fn remove_label(number: u64, label: &str, repo_root: &Path) -> Result<()> {
    gh_output(
        &format!("gh issue edit {number} --remove-label {label}"),
        || {
            let mut cmd = gh(repo_root);
            cmd.args([
                "issue",
                "edit",
                &number.to_string(),
                "--remove-label",
                label,
            ]);
            cmd
        },
    )?;
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
pub fn issue_is_closed(number: u64, repo_root: &Path) -> Result<bool> {
    let out = gh_output(&format!("gh issue view {number} --json state"), || {
        let mut cmd = gh(repo_root);
        cmd.args(["issue", "view", &number.to_string(), "--json", "state"]);
        cmd
    })?;
    parse_issue_state(&String::from_utf8_lossy(&out.stdout))
}

/// Parse the `{"labels":[{"name":"..."}]}` JSON from `gh issue view --json labels`
/// into the bare label names. A dedicated parser (not [`parse_issue`]) because the
/// `labels`-only projection carries no `number`/`title` for [`Issue`] to require.
fn parse_issue_labels(json: &str) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct LabelsJson {
        #[serde(default)]
        labels: Vec<GhLabel>,
    }
    let j: LabelsJson =
        serde_json::from_str(json).context("parsing `gh issue view --json labels`")?;
    Ok(j.labels.into_iter().map(|l| l.name).collect())
}

/// The label names on an issue, via `gh issue view <n> --json labels`. The
/// blocked-by gate uses it to classify an open blocker as a human gate
/// (`ready-for-human`/`HITL`) versus ordinary agent work the queue will clear
/// (ADR-0014).
pub fn issue_labels(number: u64, repo_root: &Path) -> Result<Vec<String>> {
    let out = gh_output(&format!("gh issue view {number} --json labels"), || {
        let mut cmd = gh(repo_root);
        cmd.args(["issue", "view", &number.to_string(), "--json", "labels"]);
        cmd
    })?;
    parse_issue_labels(&String::from_utf8_lossy(&out.stdout))
}

/// Fetch a single issue by number with its labels, via
/// `gh issue view <n> --json number,title,body,labels`. Label-agnostic on
/// purpose: the caller (`--issues`) names an explicit, already-ordered selection
/// and must not require the issue to carry a queue label. Reuses [`parse_issue`];
/// comments are filled later per-issue by the runner, exactly as for a
/// label-built queue.
pub fn fetch_issue(number: u64, repo_root: &Path) -> Result<Issue> {
    let out = gh_output(
        &format!("gh issue view {number} --json number,title,body,labels"),
        || {
            let mut cmd = gh(repo_root);
            cmd.args([
                "issue",
                "view",
                &number.to_string(),
                "--json",
                "number,title,body,labels",
            ]);
            cmd
        },
    )?;
    parse_issue(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The non-`@me` arm returns the value verbatim and spawns NO process — so it
    /// resolves against a nonexistent `repo_root` without error (proof of no `gh`).
    #[test]
    fn resolve_login_passes_through_concrete_login() {
        assert_eq!(
            resolve_login("ralphy-bot", Path::new("/nonexistent")).unwrap(),
            "ralphy-bot"
        );
    }

    /// The `@me` arm hits the live `gh api user`. Ignored by default (needs network
    /// + `gh` auth); mirrors the `e2e_references_for_bioledger_29` ignore pattern.
    ///   cargo test -p ralphy-core resolve_login_at_me_hits_gh_api_user -- --ignored --nocapture
    #[test]
    #[ignore = "live network: gh api user"]
    fn resolve_login_at_me_hits_gh_api_user() {
        let login = resolve_login("@me", Path::new(".")).expect("gh api user");
        println!("resolve_login(@me) = {login:?}");
        assert!(!login.is_empty(), "@me must resolve to a non-empty login");
    }

    #[test]
    fn parse_issue_url_reads_trailing_number() {
        assert_eq!(
            parse_issue_url("https://github.com/owner/repo/issues/42").unwrap(),
            42
        );
        // Tolerates whitespace, a trailing slash, and a preamble line.
        assert_eq!(
            parse_issue_url("Creating issue...\nhttps://github.com/o/r/issues/7/\n").unwrap(),
            7
        );
    }

    #[test]
    fn parse_issue_url_errors_without_a_number() {
        assert!(parse_issue_url("no url here").is_err());
    }

    #[test]
    fn parse_issue_labels_reads_names_and_tolerates_empty() {
        // `gh issue view --json labels` shape: a `labels` array of `{name,...}`.
        let json = r#"{"labels": [{"name": "ready-for-human"}, {"name": "needs-triage"}]}"#;
        assert_eq!(
            parse_issue_labels(json).unwrap(),
            vec!["ready-for-human".to_string(), "needs-triage".to_string()]
        );
        // No labels → empty, not an error.
        assert!(parse_issue_labels(r#"{"labels": []}"#).unwrap().is_empty());
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
            comments: vec![],
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

    #[test]
    fn parse_issue_meta_list_reads_assignees_and_state_reason() {
        let json = r#"[{"number":7,"assignees":[{"login":"alice"},{"login":"bob"}],"stateReason":null},{"number":8,"assignees":[],"stateReason":"COMPLETED"}]"#;
        let meta = parse_issue_meta_list(json).unwrap();
        assert_eq!(
            meta[0].assignees,
            vec!["alice".to_string(), "bob".to_string()]
        );
        assert_eq!(meta[0].state_reason, None);
        assert_eq!(meta[1].state_reason.as_deref(), Some("COMPLETED"));
    }

    #[test]
    fn queue_list_args_appends_assignee_only_when_present() {
        let with = queue_list_args("ready-for-agent", Some("@me"));
        let idx = with
            .iter()
            .position(|a| a == "--assignee")
            .expect("--assignee must be present when assignee is Some");
        assert_eq!(with.get(idx + 1).map(String::as_str), Some("@me"));

        let without = queue_list_args("ready-for-agent", None);
        assert!(
            !without.iter().any(|a| a == "--assignee"),
            "no --assignee token when assignee is None"
        );
    }
}
