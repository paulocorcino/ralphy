//! The board reads: the whole-tracker rows and the per-issue metadata the board fold needs.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use super::{GhAssignee, GhLabel};
use crate::github::client::{gh, gh_output};

#[derive(Deserialize)]
struct GhIssueMeta {
    number: u64,
    #[serde(default)]
    assignees: Vec<GhAssignee>,
    #[serde(default, rename = "stateReason")]
    state_reason: Option<String>,
}

/// Per-issue metadata the board fold needs (assignees, close reason) that the
/// domain [`Issue`](crate::Issue) does not carry — see [`list_issue_meta`].
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
/// closed read). `Snake_case` on the wire, matching the project's JSON convention.
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
            state: state.to_owned(),
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

/// Build the run queue's per-issue metadata (assignees, stateReason) that the
/// board fold needs but [`Issue`](crate::Issue) does not carry. Mirrors
/// [`list_queue`](super::list_queue): one batched `gh issue list` spawn per label
/// (never per-issue), union-deduped by number so an issue carrying several queue
/// labels appears once.
pub fn list_issue_meta(
    labels: &[String],
    assignee: Option<&str>,
    repo_root: &Path,
) -> Result<Vec<IssueMeta>> {
    let mut by_number: BTreeMap<u64, IssueMeta> = BTreeMap::new();
    for label in labels {
        let mut args = vec![
            "issue".to_owned(),
            "list".to_owned(),
            "--label".to_owned(),
            label.clone(),
            "--state".to_owned(),
            "open".to_owned(),
            "--json".to_owned(),
            "number,assignees,stateReason".to_owned(),
            "--limit".to_owned(),
            "100".to_owned(),
        ];
        if let Some(a) = assignee {
            args.push("--assignee".to_owned());
            args.push(a.to_owned());
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

#[cfg(test)]
mod tests;
