//! The typed run-event vocabulary (ADR-0039 §1): one `pub fn` per consumed
//! lifecycle event, owning its message, its field names, their `%`/`?` encoding,
//! and its level.
//!
//! Nothing outside this module may write one of these message literals — the
//! `…_MSG` constants are the single source, and the CLI decoder
//! (`crates/ralphy-cli/src/runstate/event.rs`) matches against them rather than
//! against strings.
//!
//! **The convention**: a new `RunEvent` variant without an `emit` helper AND a
//! round-trip test is an incomplete change (ADR-0039 §2). The round-trip lives in
//! `crates/ralphy-cli/src/runstate/roundtrip.rs`.
//!
//! Every helper emits at `INFO` on purpose: the decoder short-circuits
//! `WARN`/`ERROR` into a generic `Notice`, so a helper logged above `INFO` would
//! silently lose its identity.
//!
//! The message is passed as `"{}", MSG` rather than as a literal because
//! `format_args!` takes only literals — the constant, not a copy of its text, is
//! what every helper emits.

use tracing::info;

/// See [`issue_started`].
pub const ISSUE_STARTED_MSG: &str = "issue started";

/// Work began on an issue.
pub fn issue_started(number: u64, title: &str) {
    info!(number, title = %title, "{}", ISSUE_STARTED_MSG);
}

/// See [`plan_written`].
pub const PLAN_WRITTEN_MSG: &str = "plan written";

/// A plan was written; `open_steps == 0` means the planner judged it infeasible.
/// `usage` is the PLANNING phase's token split; `steps_json` the `[{text,status}]`
/// checkbox list.
pub fn plan_written(number: u64, open_steps: u64, usage: &crate::Usage, steps_json: &str) {
    info!(
        number,
        open_steps,
        up = usage.input,
        cr = usage.cache_read,
        cw = usage.cache_creation,
        out = usage.output,
        model = usage.model.as_deref().unwrap_or(""),
        steps_json = %steps_json,
        "{}",
        PLAN_WRITTEN_MSG
    );
}

/// See [`plan_opened`].
pub const PLAN_OPENED_MSG: &str = "plan opened";

/// The raw `plan.md` snapshot at the plan-write point (#96).
pub fn plan_opened(number: u64, plan_md: &str) {
    info!(number, plan_md = %plan_md, "{}", PLAN_OPENED_MSG);
}

/// See [`plan_closed`].
pub const PLAN_CLOSED_MSG: &str = "plan closed";

/// The raw `plan.md` snapshot at the issue close, before the next issue's
/// `plan()` overwrites it (#96).
pub fn plan_closed(number: u64, plan_md: &str) {
    info!(number, plan_md = %plan_md, "{}", PLAN_CLOSED_MSG);
}

/// See [`issue_closed`].
pub const ISSUE_CLOSED_MSG: &str = "green — issue closed";

/// A green issue was closed. `tokens` is the issue TOTAL (plan + execute +
/// protocol + repair) the telegram notifier reads; `usage` is the EXECUTION
/// phase's split the live UI combines with the planning usage (ADR-0008 D11).
pub fn issue_closed(number: u64, tokens: u64, usage: &crate::Usage) {
    info!(
        number,
        tokens,
        up = usage.input,
        cr = usage.cache_read,
        cw = usage.cache_creation,
        out = usage.output,
        model = usage.model.as_deref().unwrap_or(""),
        "{}",
        ISSUE_CLOSED_MSG
    );
}

/// See [`needs_split`].
pub const NEEDS_SPLIT_MSG: &str = "bundle plan — needs split";

/// The planner judged the issue a bundle: the queue parks on a human split.
pub fn needs_split(number: u64) {
    info!(number, "{}", NEEDS_SPLIT_MSG);
}

/// See [`blocked_by_open`].
pub const BLOCKED_BY_OPEN_MSG: &str = "blocked by open issue(s) — skipping";

/// The issue is gated on still-open blockers, all of them agent work.
pub fn blocked_by_open(number: u64, blockers: &[u64]) {
    info!(number, blockers = ?blockers, "{}", BLOCKED_BY_OPEN_MSG);
}

/// See [`blocked_waiting_human`].
pub const BLOCKED_WAITING_HUMAN_MSG: &str = "blocked — waiting on human";

/// The issue is gated on blockers, at least one of which needs a person
/// (ADR-0014) — `human_blockers` names the ones the operator must clear.
pub fn blocked_waiting_human(number: u64, blockers: &[u64], human_blockers: &[u64]) {
    info!(
        number,
        blockers = ?blockers,
        human_blockers = ?human_blockers,
        "{}",
        BLOCKED_WAITING_HUMAN_MSG
    );
}

/// See [`non_green`].
pub const NON_GREEN_MSG: &str = "non-green — stopping run";

/// An issue finished non-green and halts the queue.
pub fn non_green(number: u64, outcome: &crate::Outcome) {
    info!(number, ?outcome, "{}", NON_GREEN_MSG);
}

/// See [`deadline_passed`].
pub const DEADLINE_PASSED_MSG: &str = "deadline passed — not starting issue";

/// The global budget ran out before this issue could be started.
pub fn deadline_passed(number: u64) {
    info!(number, "{}", DEADLINE_PASSED_MSG);
}

/// See [`stop_before_label`].
pub const STOP_BEFORE_LABEL_MSG: &str = "stop-before label — halting run before this issue";

/// A `stop-before` flow-control label halts the run ahead of this issue.
pub fn stop_before_label(number: u64) {
    info!(number, "{}", STOP_BEFORE_LABEL_MSG);
}

/// See [`human_return_label`].
pub const HUMAN_RETURN_LABEL_MSG: &str = "human-return label — skipping issue";

/// A human-return label outranks the queue label (ADR-0016): the issue is
/// skipped with the parking `label` named and the queue continues.
pub fn human_return_label(number: u64, label: &str) {
    info!(number, label = %label, "{}", HUMAN_RETURN_LABEL_MSG);
}

/// See [`verify_gate_failed`].
pub const VERIFY_GATE_FAILED_MSG: &str = "verify gate failed — skipping issue";

/// The verify gate stayed red after the repair budget (ADR-0011): the issue is
/// left open and the queue marches on.
pub fn verify_gate_failed(number: u64, summary: &str) {
    info!(number, %summary, "{}", VERIFY_GATE_FAILED_MSG);
}

/// See [`usage_limit_waiting`].
pub const USAGE_LIMIT_WAITING_MSG: &str = "usage limit — waiting for reset";

/// The run hit a vendor usage limit and is sleeping. `reset` is the display wake
/// time-of-day (`HH:MM`, buffer included), `hint` the raw vendor string (logged
/// only — the decoder ignores it), `target_epoch` the countdown anchor.
pub fn usage_limit_waiting(reset: &str, hint: &str, target_epoch: i64) {
    info!(
        reset = %reset,
        hint = %hint,
        target_epoch,
        "{}",
        USAGE_LIMIT_WAITING_MSG
    );
}

/// See [`reset_reached`].
pub const RESET_REACHED_MSG: &str = "reset reached — resuming";

/// The usage-limit reset arrived and the run resumed.
pub fn reset_reached() {
    info!("{}", RESET_REACHED_MSG);
}

// ── Emitted by the adapters (ADR-0038, #149/#217) ────────────────────────────
// Both execution paths (PTY and headless) emit these, which is exactly why the
// vocabulary lives here: one constant, one shape, one operator experience
// regardless of which child shape happened to be driving.

/// See [`idle_reaped`].
pub const IDLE_REAPED_MSG: &str = "idle watchdog — no progress, reaping the child";

/// The idle watchdog reaped the active issue's child after `idle_minutes` with
/// no progress (docs/adr/0038).
pub fn idle_reaped(idle_minutes: u64) {
    info!(idle_minutes, "{}", IDLE_REAPED_MSG);
}

/// See [`api_degraded`].
pub const API_DEGRADED_MSG: &str = "api degraded — child retrying";

/// A degraded stretch persisted past the ≥3-min gate: the child is retrying.
pub fn api_degraded() {
    info!("{}", API_DEGRADED_MSG);
}

/// See [`api_recovered`].
pub const API_RECOVERED_MSG: &str = "api recovered — child resuming";

/// The degraded state cleared after an [`api_degraded`] — always a matched pair.
pub fn api_recovered() {
    info!("{}", API_RECOVERED_MSG);
}

// ── Emitted by the vendor adapters (ADR-0039 Decision 3) ────────────────────
// One message per phase for every adapter: the readable command rides in `cmd`,
// so an adapter never adds a field the decoder must learn.

/// See [`planning`].
pub const PLANNING_MSG: &str = "planning";

/// The adapter started the planning pass for the active issue.
///
/// `cmd` is the readable child command (log-only — no decoder arm reads it).
/// An empty `model`/`effort` decodes to `None`: the CLI's `clean_opt` folds an
/// empty string into an absent field.
pub fn planning(cmd: &str, model: &str, effort: &str) {
    info!(cmd = %cmd, model = %model, effort = %effort, "{}", PLANNING_MSG);
}

/// See [`executing`].
pub const EXECUTING_MSG: &str = "executing";

/// The adapter started the execution pass for the active issue.
///
/// `budget_min = 0` is the "no per-issue budget reported" sentinel the decoder's
/// `unwrap_or(0)` already assumes; empty `model`/`effort` decode to `None`.
pub fn executing(cmd: &str, budget_min: u64, model: &str, effort: &str) {
    info!(cmd = %cmd, budget_min, model = %model, effort = %effort, "{}", EXECUTING_MSG);
}

// ── Emitted by the CLI (ADR-0019/-0020/-0021), not by the core runner ────────
// The vocabulary is owned here regardless of who emits it: one module, one set
// of constants, one decoder to match against.

/// See [`queue_built`].
pub const QUEUE_BUILT_MSG: &str = "queue built";

/// The queue was resolved (ADR-0020/-0021). `order` is the `#30 -> #31` render,
/// `issues_json` the enriched per-issue snapshot, `assignee_filter` the resolved
/// login the queue was scoped to (empty = unfiltered). `stop_before` is `0` when
/// the queue carries no stop-before (issue numbers are ≥ 1).
pub fn queue_built(
    count: u64,
    order: &str,
    stop_before: u64,
    issues_json: &str,
    assignee_filter: &str,
) {
    info!(
        count,
        order = %order,
        stop_before,
        issues_json = %issues_json,
        assignee_filter = %assignee_filter,
        "{}",
        QUEUE_BUILT_MSG
    );
}

/// See [`run_started`].
pub const RUN_STARTED_MSG: &str = "run started";

/// The run began working a queue (ADR-0019 boundary event). `queue_labels` is
/// the comma-joined label list; `deadline_hours` is `0.0` for "no deadline" (the
/// sentinel the decoder folds back to `None`).
#[allow(clippy::too_many_arguments)]
pub fn run_started(
    repo: &str,
    queue_labels: &str,
    agent: &str,
    plan_agent: &str,
    branch_mode: &str,
    base: &str,
    deadline_hours: f64,
) {
    info!(
        repo = %repo,
        queue_labels = %queue_labels,
        agent,
        plan_agent,
        branch_mode,
        base = %base,
        deadline_hours,
        "{}",
        RUN_STARTED_MSG
    );
}

/// See [`run_finished`].
pub const RUN_FINISHED_MSG: &str = "run finished";

/// The run ended cleanly (ADR-0019 boundary event). `usage` is the RUN total —
/// note it emits `up/cr/cw/out` but deliberately NO `model`: a run spans models.
pub fn run_finished(
    outcome: &str,
    issues_done: u64,
    issues_skipped: u64,
    issues_total: u64,
    usage: &crate::Usage,
    duration_s: u64,
) {
    info!(
        outcome,
        issues_done,
        issues_skipped,
        issues_total,
        up = usage.input,
        cr = usage.cache_read,
        cw = usage.cache_creation,
        out = usage.output,
        duration_s,
        "{}",
        RUN_FINISHED_MSG
    );
}

/// See [`knowledge_consolidating`].
pub const KNOWLEDGE_CONSOLIDATING_MSG: &str = "consolidating knowledge";

/// The end-of-run knowledge consolidation started over `count` loose notes.
pub fn knowledge_consolidating(count: u64) {
    info!(count, "{}", KNOWLEDGE_CONSOLIDATING_MSG);
}

/// See [`knowledge_consolidated`].
pub const KNOWLEDGE_CONSOLIDATED_MSG: &str = "knowledge consolidated";

/// Consolidation finished, archiving `count` notes into `knowledge/raw/`.
pub fn knowledge_consolidated(count: u64) {
    info!(count, "{}", KNOWLEDGE_CONSOLIDATED_MSG);
}
