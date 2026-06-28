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
    bail!("`{op}` exhausted {GH_MAX_ATTEMPTS} attempts");
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

/// A label to maintain on the GitHub repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelSpec {
    pub name: String,
    pub color: String,
    pub description: String,
}

/// Strip a leading `#`, trim whitespace, and lowercase — produces the
/// 6-hex lowercase form `gh label list --json color` returns.
fn normalize_color(c: &str) -> String {
    c.trim().trim_start_matches('#').to_ascii_lowercase()
}

/// The 8 canonical Ralphy labels, with triage-role names resolved through
/// `triage_doc` when provided.  Each canonical triage role is looked up via
/// `parse_triage_mapping`; if absent in the doc the canonical name is kept.
/// Fixed-name specs (`AFK`, `HITL`, `stop-before`) are appended after the five
/// triage roles.  The result is deduped by `name` preserving first occurrence.
pub fn ralphy_label_specs(triage_doc: Option<&str>) -> Vec<LabelSpec> {
    let doc = triage_doc.unwrap_or("");
    let resolve = |canonical: &str| -> String {
        parse_triage_mapping(doc, canonical).unwrap_or_else(|| canonical.to_string())
    };

    let mut specs = vec![
        LabelSpec {
            name: resolve("needs-triage"),
            color: "e4e669".into(),
            description: "Needs a human triage pass before it can be worked".into(),
        },
        LabelSpec {
            name: resolve("needs-info"),
            color: "0075ca".into(),
            description: "Blocked — waiting for more information from the author".into(),
        },
        LabelSpec {
            name: resolve("ready-for-agent"),
            color: "0e8a16".into(),
            description: "Ready for an agent to pick up and implement".into(),
        },
        LabelSpec {
            name: resolve("ready-for-human"),
            color: "5319e7".into(),
            description: "Agent finished — waiting for human review and merge".into(),
        },
        LabelSpec {
            name: resolve("wontfix"),
            color: "e6e6e6".into(),
            description: "This issue will not be worked".into(),
        },
        LabelSpec {
            name: "AFK".into(),
            color: "f9d0c4".into(),
            description: "Agent away — run paused, will resume".into(),
        },
        LabelSpec {
            name: "HITL".into(),
            color: "b60205".into(),
            description: "Human-in-the-loop required before the agent can continue".into(),
        },
        LabelSpec {
            name: "stop-before".into(),
            color: "d93f0b".into(),
            description: "Fixed flow-control: agent must stop before acting on this issue".into(),
        },
    ];

    // Dedup by name, preserving first occurrence.
    let mut seen = std::collections::HashSet::new();
    specs.retain(|s| seen.insert(s.name.clone()));
    specs
}

/// What to do with one desired label given the current repository state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelAction {
    Create(LabelSpec),
    UpdateColor {
        name: String,
        from: String,
        to: String,
    },
    Skip(String),
}

/// Compare `desired` against `existing` (a `(name, color)` slice from the repo)
/// and return one [`LabelAction`] per desired spec.
pub fn plan_label_actions(
    desired: &[LabelSpec],
    existing: &[(String, String)],
) -> Vec<LabelAction> {
    desired
        .iter()
        .map(|spec| {
            match existing
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(&spec.name))
            {
                None => LabelAction::Create(spec.clone()),
                Some((_, existing_color)) => {
                    let norm_existing = normalize_color(existing_color);
                    let norm_desired = normalize_color(&spec.color);
                    if norm_existing != norm_desired {
                        LabelAction::UpdateColor {
                            name: spec.name.clone(),
                            from: norm_existing,
                            to: norm_desired,
                        }
                    } else {
                        LabelAction::Skip(spec.name.clone())
                    }
                }
            }
        })
        .collect()
}

/// Build the `gh label create` argv for a spec (no `--force`; only absent labels
/// are created).
fn label_create_argv(spec: &LabelSpec) -> Vec<String> {
    vec![
        "label".into(),
        "create".into(),
        spec.name.clone(),
        "--color".into(),
        spec.color.clone(),
        "--description".into(),
        spec.description.clone(),
    ]
}

/// Build the `gh label edit` argv to update a label's color.
fn label_edit_argv(name: &str, color: &str) -> Vec<String> {
    vec![
        "label".into(),
        "edit".into(),
        name.to_string(),
        "--color".into(),
        color.to_string(),
    ]
}

#[derive(serde::Deserialize)]
struct GhLabelColor {
    name: String,
    color: String,
}

/// Parse `[{"name":..,"color":..}]` JSON from `gh label list --json name,color`.
fn parse_label_list(json: &str) -> Result<Vec<(String, String)>> {
    let raw: Vec<GhLabelColor> =
        serde_json::from_str(json).context("parsing `gh label list` JSON array")?;
    Ok(raw.into_iter().map(|l| (l.name, l.color)).collect())
}

/// Fetch the current repository labels via `gh label list --json name,color --limit 200`.
pub fn list_repo_labels(repo_root: &Path) -> Result<Vec<(String, String)>> {
    let out = gh_output("gh label list --json name,color", || {
        let mut cmd = gh(repo_root);
        cmd.args(["label", "list", "--json", "name,color", "--limit", "200"]);
        cmd
    })?;
    parse_label_list(&String::from_utf8_lossy(&out.stdout))
}

/// Render a human-readable plan of label actions: one tagged line per action
/// plus a summary.
pub fn format_label_plan(actions: &[LabelAction]) -> String {
    let mut out = String::new();
    let mut n_create = 0usize;
    let mut n_update = 0usize;
    let mut n_skip = 0usize;
    for action in actions {
        match action {
            LabelAction::Create(spec) => {
                n_create += 1;
                out.push_str(&format!("  create  {}\n", spec.name));
            }
            LabelAction::UpdateColor { name, from, to } => {
                n_update += 1;
                out.push_str(&format!("  update  {} ({} → {})\n", name, from, to));
            }
            LabelAction::Skip(name) => {
                n_skip += 1;
                out.push_str(&format!("  skip    {}\n", name));
            }
        }
    }
    out.push_str(&format!(
        "labels: {} to create, {} to update, {} unchanged\n",
        n_create, n_update, n_skip
    ));
    out
}

/// Execute the label actions against the repository, routing each to the
/// appropriate `gh` call.  `Skip` actions are a no-op.
pub fn apply_label_actions(actions: &[LabelAction], repo_root: &Path) -> Result<()> {
    for action in actions {
        match action {
            LabelAction::Create(spec) => {
                let argv = label_create_argv(spec);
                let args: Vec<&str> = argv.iter().map(String::as_str).collect();
                gh_output(&format!("gh label create {}", spec.name), || {
                    let mut cmd = gh(repo_root);
                    cmd.args(&args);
                    cmd
                })?;
            }
            LabelAction::UpdateColor { name, to, .. } => {
                let argv = label_edit_argv(name, to);
                let args: Vec<&str> = argv.iter().map(String::as_str).collect();
                gh_output(&format!("gh label edit {}", name), || {
                    let mut cmd = gh(repo_root);
                    cmd.args(&args);
                    cmd
                })?;
            }
            LabelAction::Skip(_) => {}
        }
    }
    Ok(())
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

    // ── label vocabulary (stage 7) ────────────────────────────────────────────

    #[test]
    fn normalize_color_strips_hash_and_lowercases() {
        assert_eq!(normalize_color("#0E8A16"), "0e8a16");
        assert_eq!(normalize_color("0e8a16"), "0e8a16");
        assert_eq!(normalize_color("  #FFFFFF  "), "ffffff");
    }

    #[test]
    fn ralphy_label_specs_returns_8_names_including_fixed() {
        let specs = ralphy_label_specs(None);
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names.len(), 8, "expected 8 specs, got: {names:?}");
        for expected in &[
            "needs-triage",
            "needs-info",
            "ready-for-agent",
            "ready-for-human",
            "wontfix",
            "AFK",
            "HITL",
            "stop-before",
        ] {
            assert!(names.contains(expected), "missing {expected} in {names:?}");
        }
    }

    #[test]
    fn ralphy_label_specs_resolves_triage_remap() {
        let doc = "| Canonical | Mapped |\n\
                   |-----------|--------|\n\
                   | `ready-for-agent` | `afk-ready` |\n";
        let specs = ralphy_label_specs(Some(doc));
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"afk-ready"),
            "expected afk-ready in {names:?}"
        );
        assert!(
            !names.contains(&"ready-for-agent"),
            "ready-for-agent should be remapped: {names:?}"
        );
    }

    #[test]
    fn plan_label_actions_empty_existing_yields_all_create() {
        let desired = ralphy_label_specs(None);
        let actions = plan_label_actions(&desired, &[]);
        assert_eq!(actions.len(), 8);
        assert!(
            actions.iter().all(|a| matches!(a, LabelAction::Create(_))),
            "expected all Create, got: {actions:?}"
        );
    }

    #[test]
    fn plan_label_actions_full_matching_existing_yields_all_skip() {
        let desired = ralphy_label_specs(None);
        // Use hash-prefixed uppercase colors to exercise normalize_color on the
        // existing side — a raw-comparison bug would produce UpdateColor here.
        let existing: Vec<(String, String)> = desired
            .iter()
            .map(|s| (s.name.clone(), format!("#{}", s.color.to_ascii_uppercase())))
            .collect();
        let actions = plan_label_actions(&desired, &existing);
        let n_create = actions
            .iter()
            .filter(|a| matches!(a, LabelAction::Create(_)))
            .count();
        let n_update = actions
            .iter()
            .filter(|a| matches!(a, LabelAction::UpdateColor { .. }))
            .count();
        let n_skip = actions
            .iter()
            .filter(|a| matches!(a, LabelAction::Skip(_)))
            .count();
        assert_eq!(n_create, 0, "expected 0 Create");
        assert_eq!(n_update, 0, "expected 0 UpdateColor");
        assert_eq!(n_skip, 8, "expected 8 Skip");
    }

    #[test]
    fn plan_label_actions_differing_color_yields_update_no_create_for_present() {
        let desired = ralphy_label_specs(None);
        // Provide all 8 labels as existing, but one with a wrong color.
        let mut existing: Vec<(String, String)> = desired
            .iter()
            .map(|s| (s.name.clone(), normalize_color(&s.color)))
            .collect();
        // Change AFK's color to something different.
        let afk_idx = existing.iter().position(|(n, _)| n == "AFK").unwrap();
        existing[afk_idx].1 = "aabbcc".into();

        let actions = plan_label_actions(&desired, &existing);
        let n_create = actions
            .iter()
            .filter(|a| matches!(a, LabelAction::Create(_)))
            .count();
        let n_update = actions
            .iter()
            .filter(|a| matches!(a, LabelAction::UpdateColor { .. }))
            .count();
        let n_skip = actions
            .iter()
            .filter(|a| matches!(a, LabelAction::Skip(_)))
            .count();
        assert_eq!(n_create, 0, "no Create expected for any present name");
        assert_eq!(n_update, 1, "expected exactly 1 UpdateColor");
        assert_eq!(n_skip, 7, "expected 7 Skip");
        // Verify `to` carries the desired color and `from` the stale one.
        let afk_spec = desired.iter().find(|s| s.name == "AFK").unwrap();
        assert!(
            actions.iter().any(|a| matches!(
                a,
                LabelAction::UpdateColor { name, from, to }
                    if name == "AFK"
                    && from == "aabbcc"
                    && to == &normalize_color(&afk_spec.color)
            )),
            "expected UpdateColor for AFK with correct to/from"
        );
    }

    #[test]
    fn label_create_argv_produces_7_element_vec() {
        let spec = LabelSpec {
            name: "my-label".into(),
            color: "0e8a16".into(),
            description: "A test label".into(),
        };
        let argv = label_create_argv(&spec);
        assert_eq!(
            argv,
            vec![
                "label",
                "create",
                "my-label",
                "--color",
                "0e8a16",
                "--description",
                "A test label"
            ],
            "unexpected argv: {argv:?}"
        );
    }

    #[test]
    fn parse_label_list_reads_name_and_color_pairs() {
        let json = r#"[{"name":"AFK","color":"f9d0c4"},{"name":"stop-before","color":"d93f0b"}]"#;
        let pairs = parse_label_list(json).unwrap();
        assert_eq!(
            pairs,
            vec![
                ("AFK".to_string(), "f9d0c4".to_string()),
                ("stop-before".to_string(), "d93f0b".to_string()),
            ]
        );
    }

    #[test]
    fn format_label_plan_contains_names_and_summary() {
        let actions = vec![
            LabelAction::Create(LabelSpec {
                name: "new-label".into(),
                color: "ff0000".into(),
                description: "new".into(),
            }),
            LabelAction::UpdateColor {
                name: "old-label".into(),
                from: "aabbcc".into(),
                to: "112233".into(),
            },
            LabelAction::Skip("kept-label".into()),
        ];
        let output = format_label_plan(&actions);
        assert!(
            output.contains("new-label"),
            "create name missing:\n{output}"
        );
        assert!(
            output.contains("old-label"),
            "update name missing:\n{output}"
        );
        assert!(
            output.contains("1 to create"),
            "create count missing:\n{output}"
        );
        assert!(
            output.contains("1 to update"),
            "update count missing:\n{output}"
        );
        assert!(
            output.contains("1 unchanged"),
            "skip count missing:\n{output}"
        );
        assert!(
            output.contains("kept-label"),
            "skip name missing:\n{output}"
        );
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
}
