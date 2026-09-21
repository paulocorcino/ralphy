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

/// The `gh issue list --json` shape the whole-tracker board reads fetch. All
/// fields are `#[serde(default)]` so one struct deserializes BOTH the open read
/// (carries `body`, no `stateReason`) and the closed read (carries `stateReason`,
/// no `body`) — the caller stamps `state` and the parse half fills the rest.
#[derive(Deserialize)]
struct GhBoardIssue {
    number: u64,
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    labels: Vec<GhLabel>,
    #[serde(default)]
    assignees: Vec<GhAssignee>,
    #[serde(default, rename = "stateReason")]
    state_reason: Option<String>,
    #[serde(default, rename = "createdAt")]
    created_at: String,
    #[serde(default, rename = "updatedAt")]
    updated_at: String,
}

/// One whole-tracker row the Kanban board fold emits (ADR-0036 slice 6): every
/// open issue plus a bounded recent-closed batch, each carrying the columns the
/// four board lanes and the drawer need. `state` is `"open"`/`"closed"`; `reason`
/// is the lowercased `stateReason` (populated only for closed issues); `blocked_by`
/// is parsed from the body's `## Blocked by` section (empty for the body-less
/// closed read). Snake_case on the wire, matching the project's JSON convention.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BoardIssue {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub reason: Option<String>,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub blocked_by: Vec<u64>,
    pub created: String,
    pub updated: String,
}

impl GhBoardIssue {
    /// Fold a raw `gh` row into a [`BoardIssue`], stamping the given `state` and
    /// lowercasing `stateReason` into `reason`. `blocked_by` is parsed from the
    /// body (empty when the body-less closed read is folded).
    fn into_board(self, state: &str) -> BoardIssue {
        BoardIssue {
            number: self.number,
            title: self.title,
            state: state.to_string(),
            reason: self.state_reason.map(|s| s.to_lowercase()),
            labels: self.labels.into_iter().map(|l| l.name).collect(),
            assignees: self.assignees.into_iter().map(|a| a.login).collect(),
            blocked_by: crate::blocked::parse_blocked_by(&self.body),
            created: self.created_at,
            updated: self.updated_at,
        }
    }
}

/// Parse the open-issue board read (`gh issue list --state open --json
/// number,title,labels,assignees,createdAt,updatedAt,body`) into [`BoardIssue`]s,
/// each stamped `state="open"` with `blocked_by` derived from its body.
pub fn parse_all_open_meta(json: &str) -> Result<Vec<BoardIssue>> {
    let raw: Vec<GhBoardIssue> =
        serde_json::from_str(json).context("parsing `gh issue list --state open` board JSON")?;
    Ok(raw.into_iter().map(|g| g.into_board("open")).collect())
}

/// Parse the closed-issue board read (`gh issue list --state closed --json
/// number,title,labels,assignees,stateReason,createdAt,updatedAt`) into
/// [`BoardIssue`]s, each stamped `state="closed"` with `reason` lowercased from
/// `stateReason` (`COMPLETED`→`completed`, `NOT_PLANNED`→`not_planned`).
pub fn parse_closed_board(json: &str) -> Result<Vec<BoardIssue>> {
    let raw: Vec<GhBoardIssue> =
        serde_json::from_str(json).context("parsing `gh issue list --state closed` board JSON")?;
    Ok(raw.into_iter().map(|g| g.into_board("closed")).collect())
}

/// List EVERY open issue (no label filter) with the columns the board fold needs
/// (assignees, dates, body for blocked-by). The union fold applies the assignee
/// scope later, so this read is deliberately UNFILTERED — unassigned issues must
/// be present to union over. Bounded at 200 (the whole-tracker lens, not a queue).
pub fn list_all_open_meta(repo_root: &Path) -> Result<Vec<BoardIssue>> {
    let out = gh_output("gh issue list --state open (board)", || {
        let mut cmd = gh(repo_root);
        cmd.args([
            "issue",
            "list",
            "--state",
            "open",
            "--json",
            "number,title,labels,assignees,createdAt,updatedAt,body",
            "--limit",
            "200",
        ]);
        cmd
    })?;
    parse_all_open_meta(&String::from_utf8_lossy(&out.stdout))
}

/// List a bounded batch of recently-closed issues for the board's Closed column.
/// Bounded at 50 (the read-only Closed lens; the runner's queue never includes
/// closed issues, so a very old closed issue simply not appearing is acceptable).
/// No `body` is fetched (the drawer's `issues show` fills it on demand).
pub fn list_closed_board(repo_root: &Path) -> Result<Vec<BoardIssue>> {
    let out = gh_output("gh issue list --state closed (board)", || {
        let mut cmd = gh(repo_root);
        cmd.args([
            "issue",
            "list",
            "--state",
            "closed",
            "--json",
            "number,title,labels,assignees,stateReason,createdAt,updatedAt",
            "--limit",
            "50",
        ]);
        cmd
    })?;
    parse_closed_board(&String::from_utf8_lossy(&out.stdout))
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
    let (color, description) = label_bootstrap_spec(label);
    let _ = gh(repo_root)
        .args([
            "label",
            "create",
            label,
            "--color",
            &color,
            "--description",
            &description,
        ])
        .output();
    edit(repo_root)
}

/// The colour + description [`add_label`]'s create-on-missing bootstrap gives a
/// label, looked up in the canonical roster so a label born here is identical to
/// the one `ralphy init` would have created. A name outside the roster falls
/// back to a neutral grey with a generic description — never another label's.
fn label_bootstrap_spec(label: &str) -> (String, String) {
    super::labels::ralphy_label_specs(None)
        .into_iter()
        .find(|s| s.name == label)
        .map(|s| (s.color, s.description))
        .unwrap_or_else(|| ("ededed".to_string(), format!("Ralphy: {label}")))
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
mod tests;
