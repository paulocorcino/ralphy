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
