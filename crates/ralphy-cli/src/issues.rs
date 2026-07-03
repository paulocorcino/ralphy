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

    // `--push` emits the whole judged queue as a snapshot event rather than
    // printing it — the on-demand twin of the runner's enriched `queue.built`.
    // It is a queue-level operation, so it cannot be combined with `show <n>`.
    if args.push {
        if args.show.is_some() {
            anyhow::bail!(
                "`--push` emits the whole queue snapshot and cannot be combined with `show <n>`"
            );
        }
        let queue = build_list_queue(&repo_root)?;
        let view = resolve_queue_view(&queue, &[], &human_return, &tracker)?;
        return push_snapshot(&repo_root, &view);
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
        let queue = build_list_queue(&repo_root)?;
        // The list is never a forced selection, so `stop-before` is honoured.
        let view = resolve_queue_view(&queue, &[], &human_return, &tracker)?;
        let out = match args.format {
            Format::Json => render_json(&view, fields.as_deref())?,
            Format::Text => render_text(&view),
        };
        println!("{out}");
    }
    Ok(())
}

/// Build the label-scoped queue exactly as `ralphy run` does (default queue
/// labels, then dependency-ordered), so the listing reflects the sequence a run
/// would work. Best-effort ordering: a `gh` failure fetching the open set falls
/// back to in-queue edges rather than aborting.
fn build_list_queue(repo_root: &std::path::Path) -> Result<Vec<ralphy_core::Issue>> {
    let labels = github::resolve_queue_labels(&[], repo_root);
    let queue = github::list_queue(&labels, repo_root)?;
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
fn push_snapshot(repo_root: &std::path::Path, view: &QueueView) -> Result<()> {
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
    };
    let issues = serde_json::to_value(&view.issues)?;
    let data = envelope::queue_snapshot_data(&issues, view.count, &view.order, view.stop_before);
    let env = envelope::queue_snapshot_envelope(data, &ctx);

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
fn render_text(view: &QueueView) -> String {
    if view.issues.is_empty() {
        return "No open issues in the queue.".to_string();
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
mod tests {
    use super::*;
    use ralphy_core::{Issue, Usage, CONSOLIDATED_SPEC_MARKER, STOP_BEFORE_LABEL};
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;

    /// Serializes the tests that set the process-global `RALPHY_USAGE_DIR`.
    static USAGE_LOCK: Mutex<()> = Mutex::new(());

    /// A read-only tracker: read methods answer from scripted state; every
    /// mutating method bumps `mutations` (asserted to stay `0` — criterion #6).
    #[derive(Default)]
    struct FakeTracker {
        open: HashSet<u64>,
        comments: HashMap<u64, Vec<String>>,
        mutations: RefCell<usize>,
    }

    impl IssueTracker for FakeTracker {
        fn close(&self, _n: u64, _c: &str) -> Result<()> {
            *self.mutations.borrow_mut() += 1;
            Ok(())
        }
        fn write_evidence(&self, _n: u64, _b: &str, _v: &[ralphy_core::Verdict]) -> Result<()> {
            *self.mutations.borrow_mut() += 1;
            Ok(())
        }
        fn comment(&self, _n: u64, _b: &str) -> Result<()> {
            *self.mutations.borrow_mut() += 1;
            Ok(())
        }
        fn add_label(&self, _n: u64, _l: &str) -> Result<()> {
            *self.mutations.borrow_mut() += 1;
            Ok(())
        }
        fn remove_label(&self, _n: u64, _l: &str) -> Result<()> {
            *self.mutations.borrow_mut() += 1;
            Ok(())
        }
        fn create_issue(&self, _t: &str, _b: &str, _l: &[String]) -> Result<u64> {
            *self.mutations.borrow_mut() += 1;
            Ok(0)
        }
        fn upsert_marked_comment(&self, _n: u64, _m: &str, _b: &str) -> Result<()> {
            *self.mutations.borrow_mut() += 1;
            Ok(())
        }
        fn is_closed(&self, number: u64) -> Result<bool> {
            Ok(!self.open.contains(&number))
        }
        fn issue_comments(&self, number: u64) -> Result<Vec<String>> {
            Ok(self.comments.get(&number).cloned().unwrap_or_default())
        }
    }

    fn issue(number: u64, labels: &[&str], body: &str) -> Issue {
        Issue {
            number,
            title: format!("issue {number}"),
            body: body.to_string(),
            labels: labels.iter().map(|s| s.to_string()).collect(),
            comments: Vec::new(),
        }
    }

    fn human() -> Vec<String> {
        ["needs-info", "wontfix"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn render_json_emits_full_key_set_and_fields_selects_subset() {
        let queue = vec![issue(7, &["queue"], "")];
        let tr = FakeTracker::default();
        let view = resolve_queue_view(&queue, &[], &human(), &tr).unwrap();

        // Full JSON: every issue carries exactly the seven contract keys.
        let json = render_json(&view, None).unwrap();
        let val: Value = serde_json::from_str(&json).unwrap();
        let obj = val[0].as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut expected = vec![
            "number",
            "title",
            "labels",
            "queue_status",
            "skip_reason",
            "blocked_by",
            "position",
        ];
        expected.sort_unstable();
        assert_eq!(keys, expected);
        assert_eq!(obj["queue_status"], "eligible");

        // `--fields number,queue_status` yields ONLY those two keys.
        let fields = vec!["number".to_string(), "queue_status".to_string()];
        let json = render_json(&view, Some(&fields)).unwrap();
        let val: Value = serde_json::from_str(&json).unwrap();
        let obj = val[0].as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["number", "queue_status"]);
    }

    #[test]
    fn render_text_shows_a_row_for_each_status() {
        let queue = vec![
            issue(1, &[STOP_BEFORE_LABEL], ""),
            issue(2, &["needs-info"], ""),
            issue(3, &[], "## Blocked by\n- #99\n"),
            issue(4, &[], ""),
        ];
        let mut tr = FakeTracker::default();
        tr.open.insert(99);
        let view = resolve_queue_view(&queue, &[], &human(), &tr).unwrap();
        let text = render_text(&view);
        // One line per issue, each carrying its status word and reason cell.
        assert!(
            text.contains("#1") && text.contains("stop_before"),
            "{text}"
        );
        assert!(
            text.contains("#2") && text.contains("skipped") && text.contains("needs-info"),
            "{text}"
        );
        assert!(
            text.contains("#3") && text.contains("blocked") && text.contains("by #99"),
            "{text}"
        );
        assert!(
            text.contains("#4") && text.contains("eligible") && text.contains("pos 1"),
            "{text}"
        );
        assert_eq!(text.lines().count(), 4, "one row per issue: {text}");
    }

    #[test]
    fn show_view_json_carries_body_spec_labels_judgment_and_history() {
        let _g = USAGE_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("ralphy-issues-show-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("RALPHY_USAGE_DIR", &dir);

        // Seed the ledger: two rows for #7 (plan + execute) and an unrelated #8 row.
        let slug = "o/r";
        let rec = |issue: u64, phase: &str, out: u64| ralphy_core::ledger::LedgerRecord {
            project: slug.to_string(),
            actor_email: "t@example.com".into(),
            actor_name: "T".into(),
            ralphy_version: "0.0.0".into(),
            issue,
            phase: phase.to_string(),
            agent: "scripted".into(),
            model: "claude-opus-4".into(),
            outcome: "ok".into(),
            tokens: Usage {
                input: 100,
                output: out,
                cache_read: 0,
                cache_creation: 0,
                model: None,
            },
            ts: "2026-07-03T10:00:00Z".into(),
        };
        ralphy_core::ledger::append(&rec(7, "plan", 10)).unwrap();
        ralphy_core::ledger::append(&rec(7, "execute", 20)).unwrap();
        ralphy_core::ledger::append(&rec(8, "plan", 5)).unwrap();

        let history = issue_history(slug, 7);
        assert_eq!(history.len(), 2, "only #7's two rows");

        let issue = issue(7, &["queue"], "the issue body");
        let comments = vec![format!(
            "{CONSOLIDATED_SPEC_MARKER}\n## Consolidated spec\nthe real spec\n"
        )];
        let tr = FakeTracker::default();
        let view = show_view(&issue, &comments, &history, &human(), &tr).unwrap();
        let json = render_show_json(&view, None).unwrap();
        let val: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(val["number"], 7);
        assert_eq!(val["body"], "the issue body");
        assert_eq!(val["labels"], serde_json::json!(["queue"]));
        assert_eq!(val["queue_status"], "eligible");
        // The detail view carries no list-relative `position`.
        assert!(
            val.get("position").is_none(),
            "show detail must not carry a position: {val}"
        );
        assert!(
            val["consolidated_spec"]
                .as_str()
                .unwrap()
                .contains("the real spec"),
            "consolidated_spec surfaced: {val}"
        );
        let hist = val["history"].as_array().unwrap();
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[0]["phase"], "plan");
        assert_eq!(hist[0]["tokens"], 110); // 100 input + 10 output
        assert_eq!(hist[1]["phase"], "execute");
        assert_eq!(hist[1]["tokens"], 120);

        std::env::remove_var("RALPHY_USAGE_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn push_without_events_url_errors_naming_events_url() {
        // Criterion: `--push` with no `events.url` configured fails with a clear
        // message naming `events.url`. Point the events store at an empty temp dir
        // and drive `push_snapshot` directly (no `gh` needed).
        let _g = crate::events::config::ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("ralphy-issues-push-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("RALPHY_EVENTS_DIR", &dir);

        let queue = vec![issue(7, &["queue"], "")];
        let tr = FakeTracker::default();
        let view = resolve_queue_view(&queue, &[], &human(), &tr).unwrap();

        let err = push_snapshot(std::path::Path::new("."), &view).unwrap_err();
        assert!(
            err.to_string().contains("events.url"),
            "error must name events.url: {err}"
        );

        std::env::remove_var("RALPHY_EVENTS_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_and_show_never_mutate_the_tracker() {
        // Criterion #6: the surface is read-only. Drive both the list resolution
        // (over a blocked issue, exercising is_closed/open_children/issue_labels)
        // and the show view (exercising issue_comments), then assert zero mutations.
        let mut tr = FakeTracker::default();
        tr.open.insert(99);
        tr.comments.insert(7, vec!["a comment".into()]);

        let queue = vec![
            issue(3, &[], "## Blocked by\n- #99\n"),
            issue(7, &["queue"], ""),
        ];
        let _ = resolve_queue_view(&queue, &[], &human(), &tr).unwrap();

        let issue = issue(7, &["queue"], "body");
        let comments = tr.issue_comments(7).unwrap();
        let _ = show_view(&issue, &comments, &[], &human(), &tr).unwrap();

        assert_eq!(
            *tr.mutations.borrow(),
            0,
            "the query surface must never mutate the tracker"
        );
    }
}
