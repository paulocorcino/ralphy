//! `ralphy issues` — the read-only backlog query surface (ADR-0020).
//!
//! Lists open issues **as the runner judges them** (queue status, human-return
//! precedence, blocked-by gating, stop-before, position) by reusing the very
//! resolver the runner uses ([`resolve_queue_view`]), so the CLI and a real run
//! can never disagree. `ralphy issues show <n>` adds body, comments (with the
//! ADR-0017 consolidated-spec surfaced first-class), labels, the queue judgment,
//! and the issue's run history from the usage ledger (ADR-0008). The surface is
//! strictly read-only: it calls only read methods on the tracker — plus, for the
//! detail thread, the free `github::issue_comments_detailed` read outside the
//! port (#302: the port's `issue_comments` drops author and timestamp) — and
//! never a label, comment, or state mutation.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use serde::Serialize;

use ralphy_core::{
    blocked, git, github, read_project_rows, resolve_queue_view, GhTracker, IssueTracker,
    QueueStatus, QueueView, UsageRow,
};

mod render;
use render::{render_board_json, render_json, render_show_json, render_show_text, render_text};

/// `ralphy issues` arguments.
#[derive(Args)]
pub struct IssuesArgs {
    /// Any folder inside the repo.
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,

    /// Show one issue in detail instead of the list: `show <n>`, or only `<n>`.
    // ADR-0020.
    #[arg(value_name = "SPEC", num_args = 0..=2)]
    pub spec: Vec<String>,

    /// Output: a table (the default) or `json`.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    pub format: Format,

    /// With `--format json`: print only these fields, separated by commas, for
    /// example `--fields number,queue_status`. Unknown names are ignored.
    #[arg(long)]
    pub fields: Option<String>,

    /// Send the current queue to the configured `events.url` (as a
    /// `dev.ralphy.queue.snapshot` CloudEvent) instead of printing it. Fails when
    /// this repo has no `events.url`.
    // ADR-0020.
    #[arg(long)]
    pub push: bool,

    /// List only the issues assigned to this GitHub login (`@me` is you): the
    /// issues that `ralphy run --assignee` would work. Overrides
    /// `queue.assignee` in the settings.
    #[arg(long)]
    pub assignee: Option<String>,

    /// Ignore the `queue.assignee` setting this time. Cannot be used with
    /// `--assignee`.
    #[arg(long = "no-assignee", conflicts_with = "assignee")]
    pub no_assignee: bool,

    /// Emit the Kanban board fold instead of the flat queue array: `{issues[]
    /// (per-issue + assignees[], state_reason), labels[] ({name,color} repo
    /// vocabulary)}`. List + `--format json` only. Mutually exclusive with
    /// `--push` (both are queue-level, but only one output mode applies).
    // Only the workbench's Kanban reads this, so `--help` does not list it.
    #[arg(long, conflicts_with = "push", hide = true)]
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

/// One comment on the wire (ADR-0020 amendment, #302): the core record folded to
/// the keys the workbench drawer reads, `at` being the short name for the
/// creation timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CommentView {
    author: String,
    at: String,
    body: String,
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
    comments: Vec<CommentView>,
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
    let show = parse_show_spec(&args.spec)?;
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

    refuse_queue_mode(args.board, args.push, show, args.format)?;
    if args.board {
        return print_board(&repo_root, assignee.as_deref());
    }
    if args.push {
        return push_queue(&repo_root, &tracker, &human_return, assignee.as_deref());
    }
    match show {
        Some(number) => print_show(
            number,
            &repo_root,
            &tracker,
            &human_return,
            fields.as_deref(),
            args.format,
        ),
        None => print_list(
            &repo_root,
            &tracker,
            &human_return,
            assignee.as_deref(),
            fields.as_deref(),
            args.format,
        ),
    }
}

/// `--board` and `--push` are queue-level modes: neither combines with
/// `show <n>`, and `--board` prints JSON only.
fn refuse_queue_mode(board: bool, push: bool, show: Option<u64>, format: Format) -> Result<()> {
    if board {
        if show.is_some() {
            anyhow::bail!(
                "`--board` emits the whole board fold and cannot be combined with `show <n>`"
            );
        }
        if format != Format::Json {
            anyhow::bail!("`--board` requires `--format json`");
        }
    }
    if push && show.is_some() {
        anyhow::bail!(
            "`--push` emits the whole queue snapshot and cannot be combined with `show <n>`"
        );
    }
    Ok(())
}

/// The login `assignee` stands for (`@me` is resolved through `gh`), or
/// `None` when there is no assignee filter.
fn resolve_login_opt(
    assignee: Option<&str>,
    repo_root: &std::path::Path,
) -> Result<Option<String>> {
    match assignee {
        Some(a) => Ok(Some(github::resolve_login(a, repo_root)?)),
        None => Ok(None),
    }
}

/// `--board` emits the Kanban fold (ADR-0036) instead of the flat queue
/// array.
fn print_board(repo_root: &std::path::Path, assignee: Option<&str>) -> Result<()> {
    // Whole-tracker fold: build the Ready subset UNFILTERED (the assignee
    // union applies later in the fold, so unassigned issues are present to
    // union over) and graph-ordered by the same resolver the runner uses
    // (`build_list_queue` already sorts via `sort_queue_in_graph`); the
    // open+closed reads feed the Backlog/Closed columns.
    let queue = build_list_queue(repo_root, None)?;
    let open = github::list_all_open_meta(repo_root)?;
    let closed = github::list_closed_board(repo_root)?;
    let repo_labels = github::list_repo_labels(repo_root)?;
    // The union login: resolve `@me` once; `None` ⇒ unassigned-only default.
    let login = resolve_login_opt(assignee, repo_root)?;
    println!(
        "{}",
        render_board_json(&queue, &open, &closed, login.as_deref(), &repo_labels)?
    );
    Ok(())
}

/// `--push` emits the whole judged queue as a snapshot event rather than
/// printing it — the on-demand twin of the runner's enriched `queue.built`.
fn push_queue(
    repo_root: &std::path::Path,
    tracker: &GhTracker,
    human_return: &[String],
    assignee: Option<&str>,
) -> Result<()> {
    let queue = build_list_queue(repo_root, assignee)?;
    let view = resolve_queue_view(&queue, &[], human_return, tracker)?;
    // Resolve the assignee scope mark for the snapshot (ADR-0021 §5). Unlike
    // `ralphy run` (best-effort telemetry), this one-shot explicit command fails
    // loud on a resolve error — propagate via `?`.
    let assignee_filter = resolve_login_opt(assignee, repo_root)?;
    push_snapshot(repo_root, &view, assignee_filter.as_deref())
}

/// `ralphy issues show <number>`: the detail view of one issue.
fn print_show(
    number: u64,
    repo_root: &std::path::Path,
    tracker: &GhTracker,
    human_return: &[String],
    fields: Option<&[String]>,
    format: Format,
) -> Result<()> {
    let issue = github::fetch_issue(number, repo_root)?;
    // Best-effort: a comment-fetch failure degrades to body-only, never a stop
    // (matching the runner's own tolerance).
    let comments = github::issue_comments_detailed(number, repo_root).unwrap_or_default();
    let slug = git::project_slug(repo_root);
    let history = issue_history(&slug, number);
    let view = show_view(&issue, &comments, &history, human_return, tracker)?;
    let out = match format {
        Format::Json => render_show_json(&view, fields)?,
        Format::Text => render_show_text(&view),
    };
    println!("{out}");
    Ok(())
}

/// `ralphy issues`: the judged queue, as a table or as JSON.
fn print_list(
    repo_root: &std::path::Path,
    tracker: &GhTracker,
    human_return: &[String],
    assignee: Option<&str>,
    fields: Option<&[String]>,
    format: Format,
) -> Result<()> {
    let queue = build_list_queue(repo_root, assignee)?;
    // The list is never a forced selection, so `stop-before` is honoured.
    let view = resolve_queue_view(&queue, &[], human_return, tracker)?;
    let out = match format {
        Format::Json => render_json(&view, fields)?,
        Format::Text => render_text(&view, assignee),
    };
    println!("{out}");
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
    #[expect(
        clippy::disallowed_methods,
        reason = "this command runs on the main thread and has started no thread, so none \
                  reads the environment while it changes"
    )]
    std::env::remove_var(TOKEN_ENV);
    #[expect(
        clippy::disallowed_methods,
        reason = "this command runs on the main thread and has started no thread, so none \
                  reads the environment while it changes"
    )]
    std::env::remove_var(crate::telegram::config::TOKEN_ENV);

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

/// Resolve the positional selector into "list the queue" (`None`) or "show issue
/// n" (`Some(n)`). Accepts the ADR-0020 documented form `show <n>` and the bare
/// `<n>` shorthand; anything else is an explicit error rather than a silent
/// fallback to listing the queue. Clap cannot express "an optional literal word
/// then a number" on a typed positional, hence the raw tokens. The 3+ arm is
/// unreachable through the CLI (`num_args = 0..=2` caps it) and guards the pure
/// function against a future widening.
fn parse_show_spec(spec: &[String]) -> Result<Option<u64>> {
    let number = |t: &str| -> Result<u64> {
        t.parse::<u64>().with_context(|| {
            format!("unrecognized issue selector `{t}` — use `issues show <n>` or `issues <n>`")
        })
    };
    match spec {
        [] => Ok(None),
        [one] if one == "show" => {
            anyhow::bail!("`issues show` needs an issue number — use `issues show <n>`")
        }
        [one] => Ok(Some(number(one)?)),
        [word, n] if word == "show" => Ok(Some(number(n)?)),
        // Blame the token that is actually wrong: in `issues 302 303` the first
        // one is valid and naming it sends the reader hunting the wrong end.
        [_, extra] => anyhow::bail!(
            "unrecognized issue selector `{extra}` — use `issues show <n>` or `issues <n>`"
        ),
        _ => anyhow::bail!("too many arguments — use `issues show <n>` or `issues <n>`"),
    }
}

/// Build the single-issue detail view: reuse [`resolve_queue_view`] over a
/// one-element queue for the judgment, surface the consolidated-spec comment, and
/// project the ledger rows into the history list.
fn show_view(
    issue: &ralphy_core::Issue,
    comments: &[github::IssueComment],
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
    let bodies: Vec<String> = comments.iter().map(|c| c.body.clone()).collect();
    let consolidated_spec = blocked::find_consolidated_spec(&bodies).map(str::to_string);
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
        comments: comments
            .iter()
            .map(|c| CommentView {
                author: c.author.clone(),
                at: c.created_at.clone(),
                body: c.body.clone(),
            })
            .collect(),
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

#[cfg(test)]
use serde_json::Value;

#[cfg(test)]
mod tests;
