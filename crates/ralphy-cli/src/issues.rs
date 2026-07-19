//! `ralphy issues` — the read-only backlog query surface (ADR-0020).
//!
//! Lists open issues **as the runner judges them** (queue status, human-return
//! precedence, blocked-by gating, stop-before, position) by reusing the very
//! resolver the runner uses ([`resolve_queue_view`]), so the CLI and a real run
//! can never disagree. `ralphy issues show <n>` adds body, comments (with the
//! ADR-0017 consolidated-spec surfaced first-class), labels, the queue judgment,
//! and the issue's run history from the usage ledger (ADR-0008). The surface is
//! strictly read-only: it calls only read methods on the tracker and never a
//! label, comment, or state mutation.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, ValueEnum};
use serde::Serialize;
use serde_json::{Map, Value};

use ralphy_core::{
    blocked, git, github, read_project_rows, resolve_queue_view, GhTracker, IssueTracker,
    IssueView, QueueStatus, QueueView, UsageRow,
};

/// `ralphy issues` arguments.
#[derive(Args)]
pub struct IssuesArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,

    /// Show one issue in detail instead of listing the queue: `ralphy issues
    /// show <n>` (the `show` subcommand word is optional — a bare number works).
    #[arg(value_name = "NUMBER")]
    pub show: Option<u64>,

    /// Output format: the default human table, or `json`.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    pub format: Format,

    /// Comma-separated subset of fields to emit (JSON only), e.g.
    /// `--fields number,queue_status`. Unknown names are ignored.
    #[arg(long)]
    pub fields: Option<String>,

    /// Push the current queue snapshot as a `dev.ralphy.queue.snapshot` CloudEvent
    /// to the configured `events.url` (ADR-0020) instead of printing. Fails with a
    /// clear message when no `events.url` is set for this repo.
    #[arg(long)]
    pub push: bool,

    /// List only issues this login is among the assignees of (`gh --assignee`
    /// semantics; `@me` = the authenticated user), matching what `ralphy run
    /// --assignee` would work. Overrides a persisted `queue.assignee`.
    #[arg(long)]
    pub assignee: Option<String>,

    /// Disable a persisted `queue.assignee` filter for this one invocation.
    /// Mutually exclusive with `--assignee`.
    #[arg(long = "no-assignee", conflicts_with = "assignee")]
    pub no_assignee: bool,

    /// Emit the Kanban board fold instead of the flat queue array: `{issues[]
    /// (per-issue + assignees[], state_reason), labels[] ({name,color} repo
    /// vocabulary)}`. List + `--format json` only. Mutually exclusive with
    /// `--push` (both are queue-level, but only one output mode applies).
    #[arg(long, conflicts_with = "push")]
    pub board: bool,
}

/// The output format `--format` selects.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum Format {
    #[default]
    Text,
    Json,
}

/// One row of an issue's Ralphy run history, projected from a usage-ledger row
/// (ADR-0008): the phase, its terminal outcome, the model, the flat token total,
/// and the timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct HistoryRow {
    issue: u64,
    phase: String,
    outcome: String,
    model: String,
    tokens: u64,
    ts: String,
}

/// The `ralphy issues show <n>` detail view: enough to decide without opening
/// GitHub. Body, labels, the ADR-0017 consolidated-spec (when present), the queue
/// judgment (flattened from the single-issue [`IssueView`]), and the run history.
/// Carries no `position`: that is a list-relative rank with no meaning for one
/// issue viewed in isolation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ShowView {
    number: u64,
    title: String,
    body: String,
    /// The issue's full comment thread, in order (raw, unlike
    /// `consolidated_spec` which extracts just the marked comment).
    comments: Vec<String>,
    labels: Vec<String>,
    /// The authoritative consolidated-spec comment (ADR-0017), surfaced
    /// first-class when a marked comment exists; `None` otherwise.
    consolidated_spec: Option<String>,
    queue_status: QueueStatus,
    skip_reason: Option<String>,
    blocked_by: Vec<u64>,
    history: Vec<HistoryRow>,
}

/// `ralphy issues` entry point: resolve the repo, then either list the judged
/// queue or show one issue, in text or JSON.
pub fn issues_cmd(args: IssuesArgs) -> Result<()> {
    let repo_root = git::resolve_toplevel(&args.repo)?;
    let tracker = GhTracker::new(&repo_root);
    let human_return = github::resolve_human_return_labels(&repo_root);
    let fields = parse_fields(args.fields.as_deref());

    // Resolve the assignee filter identically to `ralphy run` (ADR-0021): flag >
    // `--no-assignee` > persisted `queue.assignee` > none, so the listing agrees
    // issue-for-issue with what the runner would build under the same filter.
    let settings =
        ralphy_core::Settings::load(&ralphy_core::Workspace::new(&repo_root)).unwrap_or_default();
    let assignee = crate::config::resolve_assignee(
        args.assignee.as_deref(),
        args.no_assignee,
        settings.queue.assignee.as_deref(),
    );

    // `--board` emits the Kanban fold (ADR-0036) instead of the flat queue
    // array; JSON-only, list-only, so it cannot combine with `show <n>` or a
    // non-JSON format.
    if args.board {
        if args.show.is_some() {
            anyhow::bail!(
                "`--board` emits the whole board fold and cannot be combined with `show <n>`"
            );
        }
        if args.format != Format::Json {
            anyhow::bail!("`--board` requires `--format json`");
        }
        // Whole-tracker fold: build the Ready subset UNFILTERED (the assignee
        // union applies later in the fold, so unassigned issues are present to
        // union over) and graph-ordered by the same resolver the runner uses
        // (`build_list_queue` already sorts via `sort_queue_in_graph`); the
        // open+closed reads feed the Backlog/Closed columns.
        let queue = build_list_queue(&repo_root, None)?;
        let open = github::list_all_open_meta(&repo_root)?;
        let closed = github::list_closed_board(&repo_root)?;
        let repo_labels = github::list_repo_labels(&repo_root)?;
        // The union login: resolve `@me` once; `None` ⇒ unassigned-only default.
        let login = match assignee.as_deref() {
            Some(a) => Some(github::resolve_login(a, &repo_root)?),
            None => None,
        };
        println!(
            "{}",
            render_board_json(&queue, &open, &closed, login.as_deref(), &repo_labels)?
        );
        return Ok(());
    }

    // `--push` emits the whole judged queue as a snapshot event rather than
    // printing it — the on-demand twin of the runner's enriched `queue.built`.
    // It is a queue-level operation, so it cannot be combined with `show <n>`.
    if args.push {
        if args.show.is_some() {
            anyhow::bail!(
                "`--push` emits the whole queue snapshot and cannot be combined with `show <n>`"
            );
        }
        let queue = build_list_queue(&repo_root, assignee.as_deref())?;
        let view = resolve_queue_view(&queue, &[], &human_return, &tracker)?;
        // Resolve the assignee scope mark for the snapshot (ADR-0021 §5). Unlike
        // `ralphy run` (best-effort telemetry), this one-shot explicit command fails
        // loud on a resolve error — propagate via `?`.
        let assignee_filter = match assignee.as_deref() {
            Some(a) => Some(github::resolve_login(a, &repo_root)?),
            None => None,
        };
        return push_snapshot(&repo_root, &view, assignee_filter.as_deref());
    }

    if let Some(number) = args.show {
        let issue = github::fetch_issue(number, &repo_root)?;
        // Best-effort: a comment-fetch failure degrades to body-only, never a stop
        // (matching the runner's own tolerance).
        let comments = tracker.issue_comments(number).unwrap_or_default();
        let slug = git::project_slug(&repo_root);
        let history = issue_history(&slug, number);
        let view = show_view(&issue, &comments, &history, &human_return, &tracker)?;
        let out = match args.format {
            Format::Json => render_show_json(&view, fields.as_deref())?,
            Format::Text => render_show_text(&view),
        };
        println!("{out}");
    } else {
        let queue = build_list_queue(&repo_root, assignee.as_deref())?;
        // The list is never a forced selection, so `stop-before` is honoured.
        let view = resolve_queue_view(&queue, &[], &human_return, &tracker)?;
        let out = match args.format {
            Format::Json => render_json(&view, fields.as_deref())?,
            Format::Text => render_text(&view, assignee.as_deref()),
        };
        println!("{out}");
    }
    Ok(())
}

/// Build the label-scoped queue exactly as `ralphy run` does (default queue
/// labels, then dependency-ordered), so the listing reflects the sequence a run
/// would work. Best-effort ordering: a `gh` failure fetching the open set falls
/// back to in-queue edges rather than aborting.
fn build_list_queue(
    repo_root: &std::path::Path,
    assignee: Option<&str>,
) -> Result<Vec<ralphy_core::Issue>> {
    let labels = github::resolve_queue_labels(&[], repo_root);
    let queue = github::list_queue(&labels, assignee, repo_root)?;
    if queue.len() > 1 {
        match github::list_open_issues(repo_root) {
            Ok(open) => Ok(blocked::sort_queue_in_graph(queue, &open)),
            Err(_) => Ok(blocked::sort_queue(queue)),
        }
    } else {
        Ok(blocked::sort_queue(queue))
    }
}

/// Emit the judged queue as a `dev.ralphy.queue.snapshot` CloudEvent to the
/// configured `events.url` (ADR-0020), reusing the ADR-0019 sink transport and the
/// SAME `data` builder the runner's `queue.built` uses (so the two shapes cannot
/// diverge). A one-shot synchronous POST — no ring, no worker. Fails clearly when
/// no `events.url` is configured for this repo, and reports the delivery outcome.
fn push_snapshot(
    repo_root: &std::path::Path,
    view: &QueueView,
    assignee_filter: Option<&str>,
) -> Result<()> {
    use crate::events::client::{EventSink, PostOutcome, UreqEventTransport};
    use crate::events::config::{effective_token, EventsStore, TOKEN_ENV};
    use crate::events::{emitter, envelope};

    let slug = git::project_slug(repo_root);
    let entry = EventsStore::load()
        .ok()
        .unwrap_or_default()
        .entry(&slug)
        .cloned();
    let Some(url) = entry.as_ref().and_then(|e| e.url.clone()) else {
        anyhow::bail!(
            "no events.url configured for {slug}; set it with \
             `ralphy config set events.url <url>` before `ralphy issues --push`"
        );
    };
    // The effective token honours RALPHY_EVENTS_TOKEN over the stored one; strip it
    // from the env once captured so nothing this process spawns inherits it (ADR-0019).
    let token = effective_token(entry.as_ref().and_then(|e| e.token.as_deref()));
    std::env::remove_var(TOKEN_ENV);

    // The per-run identity/context, minted exactly as `ralphy run` does.
    let ctx = envelope::EventCtx {
        source: emitter::source(&slug),
        runid: emitter::new_runid(),
        emitter: serde_json::to_value(emitter::detect(repo_root)).unwrap_or_default(),
        // Out-of-run snapshot: `branch` is the repo's current branch (no operating
        // run branch is cut), `repository` the slug — the same `data.git` shape a
        // real run carries (ADR-0019 amendment #96).
        git: serde_json::json!({
            "repository": slug,
            "branch": git::current_branch(repo_root).unwrap_or_default(),
        }),
    };
    let issues = serde_json::to_value(&view.issues)?;
    let data = envelope::queue_snapshot_data(
        &issues,
        view.count,
        &view.order,
        view.stop_before,
        assignee_filter,
    );
    // Out-of-run: no folded run state, so the `agent` block is all-`null` (matching
    // a `queue.built` emitted before `run.started` folds).
    let env = envelope::queue_snapshot_envelope(data, &ctx, &crate::runstate::RunState::default());

    let transport = UreqEventTransport::new(url.clone(), token);
    match transport.post(&env)? {
        PostOutcome::Delivered => {
            println!("queue.snapshot delivered to {url} ({} issues)", view.count);
            Ok(())
        }
        PostOutcome::Permanent => anyhow::bail!(
            "queue.snapshot rejected by {url} (4xx) — check events.url / events.token"
        ),
        PostOutcome::Transient => anyhow::bail!(
            "queue.snapshot delivery to {url} failed (5xx/timeout/network) — try again"
        ),
    }
}

/// Read this project's ledger (ADR-0008) and keep only issue `n`'s rows — the
/// issue's Ralphy run history for `show`.
fn issue_history(slug: &str, n: u64) -> Vec<UsageRow> {
    read_project_rows(slug)
        .into_iter()
        .filter(|r| r.issue == n)
        .collect()
}

/// Build the single-issue detail view: reuse [`resolve_queue_view`] over a
/// one-element queue for the judgment, surface the consolidated-spec comment, and
/// project the ledger rows into the history list.
fn show_view(
    issue: &ralphy_core::Issue,
    comments: &[String],
    history: &[UsageRow],
    human_return: &[String],
    tracker: &dyn IssueTracker,
) -> Result<ShowView> {
    let view = resolve_queue_view(std::slice::from_ref(issue), &[], human_return, tracker)?;
    let iv = view
        .issues
        .into_iter()
        .next()
        .expect("one issue in, one issue out");
    let consolidated_spec = blocked::find_consolidated_spec(comments).map(str::to_string);
    let history = history
        .iter()
        .map(|r| HistoryRow {
            issue: r.issue,
            phase: r.phase.clone(),
            outcome: r.outcome.clone(),
            model: r.model.clone(),
            tokens: r.tokens.total(),
            ts: r.ts.clone(),
        })
        .collect();
    Ok(ShowView {
        number: iv.number,
        title: iv.title,
        body: issue.body.clone(),
        comments: comments.to_vec(),
        labels: iv.labels,
        consolidated_spec,
        queue_status: iv.queue_status,
        skip_reason: iv.skip_reason,
        blocked_by: iv.blocked_by,
        history,
    })
}

/// Parse a `--fields a,b,c` value into the selection list, dropping empty tokens.
/// `None` (flag absent) means "all fields".
fn parse_fields(raw: Option<&str>) -> Option<Vec<String>> {
    raw.map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect()
    })
}

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
fn render_json(view: &QueueView, fields: Option<&[String]>) -> Result<String> {
    let arr: Vec<Value> = view
        .issues
        .iter()
        .map(|iv| Ok(select_fields(serde_json::to_value(iv)?, fields)))
        .collect::<Result<Vec<_>>>()?;
    Ok(serde_json::to_string_pretty(&Value::Array(arr))?)
}

/// Render the `show` detail view as a JSON object, optionally projected.
fn render_show_json(view: &ShowView, fields: Option<&[String]>) -> Result<String> {
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
fn render_board_json(
    ready: &[ralphy_core::Issue],
    open: &[github::BoardIssue],
    closed: &[github::BoardIssue],
    login: Option<&str>,
    repo_labels: &[(String, String)],
) -> Result<String> {
    let ready_set: std::collections::BTreeSet<u64> = ready.iter().map(|i| i.number).collect();
    let keep = |b: &github::BoardIssue| blocked::assignee_union_keep(&b.assignees, login);
    let mut rows: Vec<Value> = Vec::new();
    // Ready issues first, preserving the core's graph order — this is what makes
    // board-order == core-queue-order true by construction. A row missing from the
    // open read is synthesized so it is never silently dropped.
    for iss in ready {
        let b = match open.iter().find(|b| b.number == iss.number) {
            Some(b) => b.clone(),
            None => degraded_board_issue(iss),
        };
        if keep(&b) {
            rows.push(serde_json::to_value(&b)?);
        }
    }
    // Remaining open issues, then the closed batch — natural read order.
    for b in open {
        if !ready_set.contains(&b.number) && keep(b) {
            rows.push(serde_json::to_value(b)?);
        }
    }
    for b in closed {
        if keep(b) {
            rows.push(serde_json::to_value(b)?);
        }
    }
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
fn render_text(view: &QueueView, assignee: Option<&str>) -> String {
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
fn render_show_text(view: &ShowView) -> String {
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
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests;
