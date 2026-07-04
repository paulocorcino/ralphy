//! Issue CRUD: fetching, creating, editing, closing, labeling, and commenting
//! on GitHub issues via the `gh` CLI.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::github::client::{gh, gh_output, is_transient_gh_failure, GH_MAX_ATTEMPTS};
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
    bail!("`gh issue edit {number}` exhausted {GH_MAX_ATTEMPTS} attempts");
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

/// Parse the milestone number from `gh api .../milestones` JSON (the created
/// milestone object carries a `number`).
fn parse_milestone_number(json: &str) -> Result<u64> {
    #[derive(serde::Deserialize)]
    struct MilestoneJson {
        number: u64,
    }
    let m: MilestoneJson =
        serde_json::from_str(json).context("parsing `gh api .../milestones` JSON")?;
    Ok(m.number)
}

/// Create a GitHub repository from the local repo at `repo_root` via
/// `gh repo create`, wiring `origin` to the new remote and pushing the current
/// branch. `name` is the new repo's name (the bootstrap derives it from the
/// directory); `private` selects visibility. Used by `ralphy init`'s bootstrap to
/// give a freshly `git init`-ed directory the GitHub remote the environment gate
/// requires. The local repo must already have a commit (see
/// [`crate::git::initial_commit`]) so `--push` has something to send.
pub fn create_repo(repo_root: &Path, name: &str, private: bool) -> Result<()> {
    let visibility = if private { "--private" } else { "--public" };
    gh_output(&format!("gh repo create {name}"), || {
        let mut cmd = gh(repo_root);
        cmd.args([
            "repo", "create", name, "--source", ".", "--remote", "origin", "--push", visibility,
        ]);
        cmd
    })?;
    Ok(())
}

/// Create a GitHub Milestone via `gh api repos/{owner}/{repo}/milestones` (the
/// `{owner}`/`{repo}` placeholders are resolved by `gh` from the repo dir). Returns
/// the created milestone's number, which [`create_issue`] links issues to. ADR-0012
/// stage 8 (milestone path).
pub fn create_milestone(repo_root: &Path, title: &str, description: &str) -> Result<u64> {
    let out = gh_output(&format!("gh api milestones (create {title})"), || {
        let mut cmd = gh(repo_root);
        cmd.args([
            "api",
            "--method",
            "POST",
            "repos/{owner}/{repo}/milestones",
            "-f",
            &format!("title={title}"),
            "-f",
            &format!("description={description}"),
        ]);
        cmd
    })?;
    parse_milestone_number(&String::from_utf8_lossy(&out.stdout))
}

/// Create a GitHub issue via `gh issue create`, piping the body on stdin
/// (`--body-file -`) like [`edit_issue_body`] so multi-line bodies survive. Each
/// label is passed with a repeated `--label`; `milestone` (a milestone *name*,
/// which `gh issue create --milestone` resolves) links the issue when `Some` — the
/// milestone must already exist (see [`create_milestone`]). Returns the created
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
    use std::io::Write;
    use std::process::Stdio;

    let mut backoff = Duration::from_secs(1);
    for attempt in 1..=GH_MAX_ATTEMPTS {
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

        let mut child = gh(repo_root)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn `gh` (is the GitHub CLI installed and on PATH?)")?;

        let mut stdin = child.stdin.take().expect("stdin was piped");
        let write_result = stdin.write_all(body.as_bytes());
        drop(stdin); // close stdin (EOF) before waiting

        let out = child
            .wait_with_output()
            .context("waiting for `gh issue create`")?;

        write_result.context("writing body to `gh` stdin")?;
        if out.status.success() {
            return parse_issue_url(&String::from_utf8_lossy(&out.stdout));
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        if attempt < GH_MAX_ATTEMPTS && is_transient_gh_failure(&stderr) {
            std::thread::sleep(backoff);
            backoff *= 2;
            continue;
        }
        bail!("`gh issue create` ({title}) failed: {}", stderr.trim());
    }
    bail!("`gh issue create` ({title}) exhausted {GH_MAX_ATTEMPTS} attempts");
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

/// Parse the REST `GET .../issues/{n}/comments` JSON array into `(id, body)`
/// pairs in thread order. Unlike [`parse_issue_comments`] this keeps the numeric
/// comment `id` — the handle a REST `PATCH .../issues/comments/{id}` needs to edit
/// a specific comment (the `gh issue view --json comments` node ids cannot drive
/// the REST edit). Comments with a null/absent body default to empty.
pub fn parse_rest_comments(json: &str) -> Result<Vec<(u64, String)>> {
    #[derive(serde::Deserialize)]
    struct RestComment {
        id: u64,
        #[serde(default)]
        body: String,
    }
    let comments: Vec<RestComment> =
        serde_json::from_str(json).context("parsing REST issue comments JSON")?;
    Ok(comments.into_iter().map(|c| (c.id, c.body)).collect())
}

/// Fetch an issue's comments WITH their numeric REST ids via
/// `gh api repos/{owner}/{repo}/issues/{n}/comments --paginate`. The `{owner}` /
/// `{repo}` placeholders resolve from the `repo_root` cwd. Used to find Ralphy's
/// own marked consolidated-spec comment for an idempotent edit (ADR-0017).
pub fn list_comments_with_ids(number: u64, repo_root: &Path) -> Result<Vec<(u64, String)>> {
    let path = format!("repos/{{owner}}/{{repo}}/issues/{number}/comments");
    let out = gh_output(&format!("gh api {path}"), || {
        let mut cmd = gh(repo_root);
        cmd.args(["api", &path, "--paginate"]);
        cmd
    })?;
    parse_rest_comments(&String::from_utf8_lossy(&out.stdout))
}

/// The numeric id of the first comment whose body carries `marker`, or `None`.
/// The seam that makes `ralphy triage`'s consolidated-spec comment idempotent:
/// found → edit that id; absent → post a fresh one.
pub fn find_marked_comment(comments: &[(u64, String)], marker: &str) -> Option<u64> {
    comments
        .iter()
        .find(|(_, body)| body.contains(marker))
        .map(|(id, _)| *id)
}

/// Edit an existing issue comment by numeric id via
/// `gh api repos/{owner}/{repo}/issues/comments/{id} -X PATCH --input -`, sending
/// `{"body": ...}` on stdin (never argv — bodies carry markdown, newlines, and
/// quotes that would break Windows quoting). Mirrors [`edit_issue_body`]'s
/// stdin-pipe + transient-retry shape.
pub fn edit_comment(id: u64, body: &str, repo_root: &Path) -> Result<()> {
    use std::io::Write;
    use std::process::Stdio;

    let path = format!("repos/{{owner}}/{{repo}}/issues/comments/{id}");
    let payload = serde_json::json!({ "body": body }).to_string();

    let mut backoff = Duration::from_secs(1);
    for attempt in 1..=GH_MAX_ATTEMPTS {
        let mut child = gh(repo_root)
            .args(["api", &path, "-X", "PATCH", "--input", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn `gh` (is the GitHub CLI installed and on PATH?)")?;

        let mut stdin = child.stdin.take().expect("stdin was piped");
        let write_result = stdin.write_all(payload.as_bytes());
        drop(stdin);

        let out = child
            .wait_with_output()
            .context("waiting for `gh api` comment edit")?;

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
        bail!("`gh api` comment edit ({id}) failed: {}", stderr.trim());
    }
    bail!("`gh api` comment edit ({id}) exhausted {GH_MAX_ATTEMPTS} attempts");
}

/// Post-or-edit the single comment carrying `marker` on an issue (ADR-0017): find
/// Ralphy's own marked comment and EDIT it, or post a fresh one when none exists.
/// Idempotent by construction — re-triage never stacks a second consolidated-spec
/// comment. The author's body and other people's comments are never touched.
pub fn upsert_marked_comment(
    number: u64,
    marker: &str,
    body: &str,
    repo_root: &Path,
) -> Result<()> {
    let existing = list_comments_with_ids(number, repo_root)?;
    match find_marked_comment(&existing, marker) {
        Some(id) => edit_comment(id, body, repo_root),
        None => comment_issue(number, body, repo_root),
    }
}

/// Parse `gh issue view --json number,title,state,body,url` into a [`Reference`].
fn parse_reference(json: &str) -> Result<crate::references::Reference> {
    #[derive(serde::Deserialize)]
    struct RefJson {
        number: u64,
        #[serde(default)]
        title: String,
        #[serde(default)]
        state: String,
        #[serde(default)]
        body: String,
        #[serde(default)]
        url: String,
    }
    let r: RefJson =
        serde_json::from_str(json).context("parsing `gh issue view` reference JSON")?;
    Ok(crate::references::Reference {
        number: r.number,
        state: r.state,
        title: r.title,
        body: r.body,
        url: r.url,
    })
}

/// Fetch a single issue's number, title, state, body, and URL via
/// `gh issue view <n> --json number,title,state,body,url` — the source a
/// structured reference (`## Blocked by` / `## Parent`) points at, reproduced for
/// the planner. The `url` is the handle the planner follows to pull the comment
/// thread on demand (this call omits comments by design). One call carries
/// everything `references.md` renders, distinct from the state-only
/// [`issue_is_closed`] and the comments-only [`issue_comments`].
pub fn fetch_reference(number: u64, repo_root: &Path) -> Result<crate::references::Reference> {
    let out = gh_output(
        &format!("gh issue view {number} --json number,title,state,body,url"),
        || {
            let mut cmd = gh(repo_root);
            cmd.args([
                "issue",
                "view",
                &number.to_string(),
                "--json",
                "number,title,state,body,url",
            ]);
            cmd
        },
    )?;
    parse_reference(&String::from_utf8_lossy(&out.stdout))
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
    use std::process::Command;

    /// End-to-end evidence against the live issue
    /// `paulocorcino/bioledger-platform#29`: drive the real ralphy chain
    /// (`structured_refs` → `parse_reference` → `render_references_file`) over
    /// real GitHub data and print the `.ralphy/references.md` it produces.
    ///
    /// Ignored by default (needs network + `gh` auth + that specific repo). Run:
    ///   cargo test -p ralphy-core e2e_references_for_bioledger_29 -- --ignored --nocapture
    ///
    /// The only departure from production is the `--repo` flag here vs.
    /// `fetch_reference`'s cwd-pinned `gh`: the parser and renderer exercised are
    /// the exact ones the runner uses.
    #[test]
    #[ignore = "live network: paulocorcino/bioledger-platform"]
    fn e2e_references_for_bioledger_29() {
        const REPO: &str = "paulocorcino/bioledger-platform";

        // #29's real body, verbatim (the two structured sections are what matter).
        let body_29 = "## Parent\n\n\
            Part of PRD-0009. Split from #15 (Spike S2c — OCR + Redaction, bundle `needs-split`).\n\n\
            ## What to build\n\n\
            A metade Redaction do S2c, medida no harness sobre o corpus.\n\n\
            ## Blocked by\n\n\
            - #13 (S2a — corpus real + ground truth)\n";

        // 1. Our pure extractor over the real body: structured refs only.
        let refs = crate::blocked::structured_refs(body_29, 29);
        println!("\nstructured_refs(#29) = {refs:?}");
        assert_eq!(
            refs,
            vec![13, 15],
            "blocked-by (#13) leads, then parent (#15)"
        );

        // 2. Fetch each ref from the live repo and run OUR parser on the output.
        let mut fetched = Vec::new();
        for n in refs {
            let out = Command::new("gh")
                .args([
                    "issue",
                    "view",
                    &n.to_string(),
                    "--repo",
                    REPO,
                    "--json",
                    "number,title,state,body,url",
                ])
                .output()
                .expect("spawn gh");
            assert!(out.status.success(), "gh failed for #{n}");
            let r = parse_reference(&String::from_utf8_lossy(&out.stdout))
                .unwrap_or_else(|e| panic!("parse_reference(#{n}): {e}"));
            println!("  fetched #{} [{}] {}", r.number, r.state, r.title);
            fetched.push(r);
        }

        // 3. Our renderer produces the file the planner reads.
        let file =
            crate::references::render_references_file(&fetched).expect("non-empty references file");
        println!("\n----- .ralphy/references.md -----\n{file}\n---------------------------------");

        // Evidence assertions: both refs present with their real state, source
        // bodies reproduced — not paraphrased.
        assert!(file.contains("## #13 (CLOSED)"));
        assert!(file.contains("## #15 (CLOSED)"));
        assert!(file.contains("ground truth") || file.to_lowercase().contains("corpus"));
        assert!(file.contains("treat it as a lead"));
        // The source URL travels with each reference (the handle for comments).
        assert!(file.contains("/bioledger-platform/issues/13"));
        assert!(file.contains("/bioledger-platform/issues/15"));
    }

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
    fn parse_milestone_number_reads_number_field() {
        let json = r#"{"number": 3, "title": "v1", "state": "open"}"#;
        assert_eq!(parse_milestone_number(json).unwrap(), 3);
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
    fn parse_reference_reads_number_state_title_body() {
        let json = r#"{"number":13,"title":"S2a corpus","state":"OPEN","body":"ground truth corpus","url":"https://github.com/o/r/issues/13"}"#;
        let r = parse_reference(json).unwrap();
        assert_eq!(r.number, 13);
        assert_eq!(r.state, "OPEN");
        assert_eq!(r.title, "S2a corpus");
        assert!(r.body.contains("ground truth"));
        assert_eq!(r.url, "https://github.com/o/r/issues/13");
    }

    #[test]
    fn parse_reference_tolerates_missing_optional_fields() {
        let r = parse_reference(r#"{"number":7,"state":"CLOSED"}"#).unwrap();
        assert_eq!(r.number, 7);
        assert_eq!(r.state, "CLOSED");
        assert!(r.title.is_empty());
        assert!(r.body.is_empty());
        assert!(r.url.is_empty());
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
    fn parse_rest_comments_extracts_ids_and_bodies() {
        let json = r#"[
            { "id": 111, "body": "first" },
            { "id": 222, "body": "<!-- ralphy:consolidated-spec -->\nspec" }
        ]"#;
        let got = parse_rest_comments(json).expect("parse");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], (111, "first".to_string()));
        assert_eq!(got[1].0, 222);
        assert!(got[1].1.contains("consolidated-spec"));
    }

    #[test]
    fn find_marked_comment_matches_marker_only() {
        let comments = vec![
            (1u64, "just chatter".to_string()),
            (
                2u64,
                "<!-- ralphy:consolidated-spec -->\nthe spec".to_string(),
            ),
            (
                3u64,
                "another <!-- ralphy:consolidated-spec --> later".to_string(),
            ),
        ];
        // First marked comment wins; unmarked comments are skipped.
        assert_eq!(
            find_marked_comment(&comments, "<!-- ralphy:consolidated-spec -->"),
            Some(2)
        );
        assert_eq!(
            find_marked_comment(&comments[..1], "<!-- ralphy:consolidated-spec -->"),
            None
        );
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
