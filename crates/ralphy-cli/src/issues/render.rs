//! The text and JSON a `ralphy issues` view prints.

use std::borrow::Cow;

use anyhow::Result;
use serde_json::{Map, Value};

use ralphy_core::{blocked, github, IssueView, QueueStatus, QueueView};

use super::ShowView;

/// Keep only `fields` keys of a JSON object, or the whole value when `fields` is
/// `None`. A requested key absent from the object is silently skipped. Key order
/// in the result is not preserved — `serde_json::Map` sorts keys — but JSON object
/// key order is not semantically meaningful.
fn select_fields(full: Value, fields: Option<&[String]>) -> Value {
    match fields {
        None => full,
        Some(keys) => {
            let obj = full.as_object().cloned().unwrap_or_default();
            let mut m = Map::new();
            for k in keys {
                if let Some(v) = obj.get(k) {
                    m.insert(k.clone(), v.clone());
                }
            }
            Value::Object(m)
        }
    }
}

/// Render the judged queue as a JSON array of per-issue objects, optionally
/// projected to `--fields`.
pub(super) fn render_json(view: &QueueView, fields: Option<&[String]>) -> Result<String> {
    let arr: Vec<Value> = view
        .issues
        .iter()
        .map(|iv| Ok(select_fields(serde_json::to_value(iv)?, fields)))
        .collect::<Result<Vec<_>>>()?;
    Ok(serde_json::to_string_pretty(&Value::Array(arr))?)
}

/// Render the `show` detail view as a JSON object, optionally projected.
pub(super) fn render_show_json(view: &ShowView, fields: Option<&[String]>) -> Result<String> {
    let full = serde_json::to_value(view)?;
    Ok(serde_json::to_string_pretty(&select_fields(full, fields))?)
}

/// A degraded board row synthesized from a queue [`ralphy_core::Issue`] when the
/// open read did not carry it (e.g. a queue-labeled issue older than the open
/// read's `--limit`). Preserves the parity invariant — a Ready row the runner
/// would execute never silently vanishes — at the cost of the richer meta
/// (assignees empty ⇒ kept by the union, dates blank) the drawer's `issue.show`
/// backfills anyway.
fn degraded_board_issue(iss: &ralphy_core::Issue) -> github::BoardIssue {
    github::BoardIssue {
        number: iss.number,
        title: iss.title.clone(),
        state: "open".to_string(),
        reason: None,
        labels: iss.labels.clone(),
        assignees: Vec::new(),
        blocked_by: blocked::parse_blocked_by(&iss.body),
        created: String::new(),
        updated: String::new(),
    }
}

/// Render the whole-tracker Kanban board fold (ADR-0036 slice 6): the Ready
/// subset (`ready`, already in the core's `sort_queue_in_graph` order) FIRST,
/// then the remaining open issues, then the recent-closed batch. Every row is
/// filtered by the assignee union ([`blocked::assignee_union_keep`]), so with no
/// configured login the default hides assigned issues. A Ready issue absent from
/// the `open` read is emitted from a synthesized [`degraded_board_issue`] rather
/// than dropped — board-order == core-queue-order stays true even when the open
/// read's limit does not cover the whole queue. Each row carries
/// `{number,title,state,reason,labels,assignees,blocked_by,created,updated}`;
/// `labels[]` is the repo's `{name,color}` vocabulary for chip rendering.
pub(super) fn render_board_json(
    ready: &[ralphy_core::Issue],
    open: &[github::BoardIssue],
    closed: &[github::BoardIssue],
    login: Option<&str>,
    repo_labels: &[(String, String)],
) -> Result<String> {
    let ready_set: std::collections::BTreeSet<u64> = ready.iter().map(|i| i.number).collect();
    // Ready issues first, preserving the core's graph order — this is what makes
    // board-order == core-queue-order true by construction. A row missing from the
    // open read is synthesized so it is never silently dropped.
    let ready_rows = ready.iter().map(|iss| {
        open.iter()
            .find(|b| b.number == iss.number)
            .map_or_else(|| Cow::Owned(degraded_board_issue(iss)), Cow::Borrowed)
    });
    // Remaining open issues, then the closed batch — natural read order.
    let rest = open
        .iter()
        .filter(|b| !ready_set.contains(&b.number))
        .chain(closed)
        .map(Cow::Borrowed);
    let rows = ready_rows
        .chain(rest)
        .filter(|b| blocked::assignee_union_keep(&b.assignees, login))
        .map(|b| serde_json::to_value(&*b))
        .collect::<Result<Vec<Value>, _>>()?;
    let labels: Vec<Value> = repo_labels
        .iter()
        .map(|(name, color)| serde_json::json!({"name": name, "color": color}))
        .collect();
    Ok(serde_json::to_string_pretty(
        &serde_json::json!({"issues": rows, "labels": labels}),
    )?)
}

/// The wire word for a queue status, matching the ADR-0020 columns.
fn status_word(status: QueueStatus) -> &'static str {
    match status {
        QueueStatus::Eligible => "eligible",
        QueueStatus::Skipped => "skipped",
        QueueStatus::Blocked => "blocked",
        QueueStatus::StopBefore => "stop_before",
    }
}

/// The trailing reason cell for a listed issue: `pos N` for eligibles, the
/// parking label for skips, `by #a, #b` for blocked, and the boundary note for
/// stop-before.
fn status_extra(iv: &IssueView) -> String {
    match iv.queue_status {
        QueueStatus::Eligible => iv.position.map(|p| format!("pos {p}")).unwrap_or_default(),
        QueueStatus::Skipped => iv.skip_reason.clone().unwrap_or_default(),
        QueueStatus::Blocked => format!(
            "by {}",
            iv.blocked_by
                .iter()
                .map(|n| format!("#{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        QueueStatus::StopBefore => "run halts here".to_string(),
    }
}

/// The `[a,b]` labels cell for a listed issue.
fn labels_cell(labels: &[String]) -> String {
    format!("[{}]", labels.join(","))
}

/// Truncate a title to `max` chars for the text table, appending `…` when cut.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Render the judged queue as an aligned text table matching the ADR-0020 sample.
pub(super) fn render_text(view: &QueueView, assignee: Option<&str>) -> String {
    if view.issues.is_empty() {
        return match assignee {
            Some(a) => format!("No open issues in the queue assigned to {a}."),
            None => "No open issues in the queue.".to_string(),
        };
    }
    const TITLE_MAX: usize = 40;
    let rows: Vec<(String, String, String, &'static str, String)> = view
        .issues
        .iter()
        .map(|iv| {
            (
                format!("#{}", iv.number),
                truncate(&iv.title, TITLE_MAX),
                labels_cell(&iv.labels),
                status_word(iv.queue_status),
                status_extra(iv),
            )
        })
        .collect();
    let num_w = rows.iter().map(|r| r.0.chars().count()).max().unwrap_or(0);
    let title_w = rows.iter().map(|r| r.1.chars().count()).max().unwrap_or(0);
    let labels_w = rows.iter().map(|r| r.2.chars().count()).max().unwrap_or(0);
    let mut out = String::new();
    for (num, title, labels, status, extra) in &rows {
        out.push_str(&format!(
            "{num:<num_w$}  {title:<title_w$}  {labels:<labels_w$}  {status:<11}  {extra}\n"
        ));
    }
    out.trim_end().to_string()
}

/// Render the `show` detail view as a readable text block.
pub(super) fn render_show_text(view: &ShowView) -> String {
    let mut out = String::new();
    out.push_str(&format!("#{}  {}\n", view.number, view.title));
    out.push_str(&format!("labels: {}\n", labels_cell(&view.labels)));
    let judgment = match view.queue_status {
        // A detail view has no list to rank against, so `eligible` carries no
        // position (position is a list-relative concept — see `IssueView`).
        QueueStatus::Eligible => "eligible".to_string(),
        QueueStatus::Skipped => format!(
            "skipped ({})",
            view.skip_reason.as_deref().unwrap_or("human-return")
        ),
        QueueStatus::Blocked => format!(
            "blocked by {}",
            view.blocked_by
                .iter()
                .map(|n| format!("#{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        QueueStatus::StopBefore => "stop_before (run halts here)".to_string(),
    };
    out.push_str(&format!("queue: {judgment}\n"));
    if let Some(spec) = &view.consolidated_spec {
        out.push_str("\n--- consolidated spec ---\n");
        out.push_str(spec);
        out.push('\n');
    } else if !view.body.is_empty() {
        out.push_str("\n--- body ---\n");
        out.push_str(&view.body);
        out.push('\n');
    }
    if !view.history.is_empty() {
        out.push_str("\n--- history ---\n");
        for h in &view.history {
            out.push_str(&format!(
                "{}  {}  {}  {}  {} tok\n",
                h.ts, h.phase, h.outcome, h.model, h.tokens
            ));
        }
    }
    if !view.comments.is_empty() {
        out.push_str(&format!("\n--- comments ({}) ---\n", view.comments.len()));
        for c in &view.comments {
            out.push_str(&format!("{}  {}\n", c.author, c.at));
            out.push_str(&c.body);
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}
