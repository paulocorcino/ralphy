//! The runner's configuration and report types (ADR-0022 split of `runner.rs`):
//! the model-free [`QueueConfig`] the CLI fills, the per-issue [`IssueResult`]
//! and [`StopReason`], the [`QueueReport`] the run returns, and the internal
//! [`RunLedger`] accumulator threaded through the phases.

use std::collections::BTreeMap;
use std::time::Duration;

use tracing::warn;

use super::BranchMode;
use crate::ledger::{LedgerRecord, LedgerSink};
use crate::{Outcome, Usage};

/// Everything the core needs to work a whole queue — model-free by construction
/// (model and effort are adapter concerns, set when the adapter is built). The
/// issues come from the caller (built via [`crate::github::list_queue`]) so the
/// loop itself stays `gh`-free and testable.
pub struct QueueConfig {
    pub repo_root: std::path::PathBuf,
    pub base_branch: String,
    pub dry_run: bool,
    pub stamp: String,
    /// Where commits land: a fresh `afk/run-*` branch (`New`) or the branch the
    /// repo is already on (`Current`, which ignores `base_branch`).
    pub branch_mode: BranchMode,
    /// Issues the operator named explicitly (`--only-issue N` → one entry;
    /// `--issues 5,3,9` → the whole list, in order). The `stop-before` label on
    /// any listed issue is ignored and it runs normally — the queue was already
    /// filtered to this selection, so the operator clearly wants it. Empty means
    /// no explicit selection (the ordinary label-built queue). Mirrors ps1's
    /// `$OnlyIssue -le 0` guard, generalized to a set.
    pub forced_issues: Vec<u64>,
    /// The human-return label set (ADR-0016): any of these on a queued issue
    /// outranks its queue label, so the issue is skipped with a recorded reason
    /// and the queue continues. Resolved by the CLI (via
    /// [`crate::github::resolve_human_return_labels`]) so the core stays
    /// `gh`-free. Unlike `stop-before`, `forced_issues` does NOT override these —
    /// a human-return label may record someone else's state (ADR-0016).
    pub human_return_labels: Vec<String>,
    /// When true, a usage limit during the *plan* phase stops the run and reports
    /// the reset (the old behaviour). The default (`false`) waits for the reset
    /// and auto-resumes the same issue. Derived from the planner agent so a split
    /// run can resume through a plan-time reset while still stopping on an
    /// execute-time limit. See docs/adr/0003 and docs/adr/0009.
    pub stop_on_limit_plan: bool,
    /// When true, a usage limit during the *execute* phase stops the run and
    /// reports the reset. The default (`false`) waits and auto-resumes. Derived
    /// from the executor agent. See docs/adr/0003 and docs/adr/0009.
    pub stop_on_limit_exec: bool,
    /// The per-repo fallback verify command(s) resolved from `settings.json`
    /// `verify.command` (ADR-0011). Used only when a plan's `## Verify` section
    /// is *absent or empty* (`VerifySpec::Unspecified`); a plan's own commands
    /// take precedence and `## Verify: none` skips this fallback. `None` here
    /// means no per-repo default — an unspecified plan then closes on the agent's
    /// self-report with a loud warning.
    pub verify_fallback: Option<Vec<Vec<String>>>,
    /// The bounded time budget for one issue's verify gate (ADR-0011). A gate
    /// that runs past it is killed and counts as a failure. Derived from
    /// `--max-minutes-per-issue`.
    pub verify_timeout: Duration,
    /// When true, an issue whose verify resolution lands on `VerifyPlan::NoGate`
    /// (no `## Verify` in the plan and no settings fallback) is NOT closed on the
    /// agent's self-report: it is labeled `ready-for-human`, a comment explains
    /// why, and the run continues to the next issue (ADR-0015). `false` keeps the
    /// ADR-0011 warn-and-close behavior. From `settings.json`
    /// `verify.require_verify_gate`.
    pub require_verify_gate: bool,
    /// The literal completion token the active adapter's charter tells the
    /// agent to emit. The runner never DETECTS it — completion detection lives
    /// in the adapters (ADR-0002) — it only quotes it in the verify/protocol
    /// repair briefs so the hand-back speaks the agent's own protocol. Supplied
    /// by the caller (the CLI passes the adapter layer's constant).
    pub done_signal: String,
}

/// What happened to one issue in the queue.
#[derive(Debug)]
pub struct IssueResult {
    pub number: u64,
    /// The execution outcome, or `None` when the issue was skipped (infeasible
    /// plan, blocked, or dry run).
    pub outcome: Option<Outcome>,
    /// Whether the runner closed the issue (the cycle). Only ever true for a
    /// green, non-dry-run issue.
    pub closed: bool,
    /// Open blocker issue numbers that caused this issue to be skipped. Empty
    /// when the issue was not blocked.
    pub blocked_by: Vec<u64>,
    /// The subset of `blocked_by` that are human gates (`ready-for-human`/`HITL`,
    /// ADR-0014): blockers parked until a person acts, not agent work the queue
    /// will clear. Empty when no blocker is a human gate. The run still continues
    /// past the issue — only this chain stalls; this field is for visibility.
    pub human_blockers: Vec<u64>,
}

/// Why the queue loop stopped before reaching the end.
#[derive(Debug)]
pub enum StopReason {
    /// The deadline passed before the next issue could be started.
    Deadline,
    /// An issue finished non-green; the run hands back the branch as it stands.
    NonGreen { number: u64, outcome: Outcome },
    /// A `stop-before` label halted the run before the tagged issue.
    StopBefore { number: u64 },
    /// The agent hit a usage/rate limit; includes the parsed reset time when
    /// present in the transcript.
    Limit { number: u64, reset: Option<String> },
}

/// The result of working a queue: the branch the commits landed on, where the
/// repo started, the per-issue results, and why the loop stopped (if it did).
#[derive(Debug)]
pub struct QueueReport {
    pub branch: String,
    pub orig_branch: String,
    pub worked: Vec<IssueResult>,
    pub stop: Option<StopReason>,
    /// Number of commits the run added over the compare ref (the base in `New`
    /// mode, the pre-run HEAD in `Current` mode).
    pub commits: usize,
    /// The local `ralphy/pre-run-<stamp>` tag marking where the run started —
    /// the undo handle (`git reset --hard <tag>` in `Current` mode). `None` when
    /// tagging failed or the run added no commits (the tag is then deleted:
    /// nothing to undo).
    pub undo_tag: Option<String>,
    /// One `git log --oneline` entry per counted commit.
    pub oneline: Vec<String>,
    /// The token usage this run consumed across every phase it worked — the sum
    /// of each plan and execute [`Usage`] (ADR-0008). The console footer's run
    /// total (D11) reads off it.
    pub run_usage: Usage,
    /// The run's token usage split **per model** (keyed by the phase's `model`, or
    /// `unknown` when the adapter captured none). The footer's read-time USD (D8)
    /// needs this split because price resolves per model — `run_usage` alone cannot
    /// be priced once a run mixes models.
    pub run_usage_by_model: BTreeMap<String, Usage>,
}

/// Fold one phase's [`Usage`] into a per-model accumulator, keyed by its `model`
/// (or `unknown` when the adapter captured none). The read-time USD footer (D8)
/// needs this split because price resolves per model.
fn accumulate_by_model(by_model: &mut BTreeMap<String, Usage>, usage: &Usage) {
    by_model
        .entry(usage.model.clone().unwrap_or_else(|| "unknown".into()))
        .or_default()
        .add_tokens(usage);
}

/// One run's ledger identity (ADR-0008 D6/D7, read once from git) plus its
/// token accumulators, threaded through the phases so every phase line is
/// built and folded in one place instead of four hand-rolled copies.
pub(crate) struct RunLedger<'a> {
    pub(crate) sink: &'a dyn LedgerSink,
    pub(crate) project: String,
    pub(crate) actor_email: String,
    pub(crate) actor_name: String,
    /// The adapter label, `agent.name()`.
    pub(crate) agent: &'static str,
    pub(crate) run_usage: Usage,
    pub(crate) run_usage_by_model: BTreeMap<String, Usage>,
}

impl RunLedger<'_> {
    /// Append one phase line (best-effort — a write failure warns, never stops
    /// the run, D9) and fold the usage into the run totals.
    pub(crate) fn record_phase(
        &mut self,
        issue: u64,
        phase: &str,
        outcome: &str,
        usage: &Usage,
        session_id: Option<&str>,
    ) {
        let rec = LedgerRecord {
            project: self.project.clone(),
            actor_email: self.actor_email.clone(),
            actor_name: self.actor_name.clone(),
            ralphy_version: env!("CARGO_PKG_VERSION").into(),
            issue,
            phase: phase.into(),
            agent: self.agent.into(),
            model: usage.model.clone().unwrap_or_else(|| "unknown".into()),
            session_id: session_id.map(str::to_string),
            outcome: outcome.into(),
            tokens: usage.clone(),
            ts: chrono::Utc::now().to_rfc3339(),
        };
        if let Err(e) = self.sink.append(&rec) {
            warn!(number = issue, error = %e, "writing {} usage ledger line failed", phase);
        }
        self.run_usage.add_tokens(usage);
        accumulate_by_model(&mut self.run_usage_by_model, usage);
    }

    /// [`record_phase`](Self::record_phase) for the conditional repair phases:
    /// a phase that consumed nothing writes no line AND folds nothing — an
    /// unconditional fold would plant a zero-usage `unknown` key in the
    /// per-model split the report exposes.
    pub(crate) fn record_phase_if_used(
        &mut self,
        issue: u64,
        phase: &str,
        outcome: &str,
        usage: &Usage,
        session_id: Option<&str>,
    ) {
        if usage.total() > 0 {
            self.record_phase(issue, phase, outcome, usage, session_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulate_by_model_splits_and_sums_per_model_with_unknown_fallback() {
        let mut by_model: BTreeMap<String, Usage> = BTreeMap::new();
        let usage_a = |i| Usage {
            input: i,
            output: 0,
            cache_read: 0,
            cache_creation: 0,
            model: Some("model-a".into()),
        };
        accumulate_by_model(&mut by_model, &usage_a(100));
        accumulate_by_model(&mut by_model, &usage_a(200));
        // A phase with no captured model is keyed under `unknown`, not dropped.
        accumulate_by_model(
            &mut by_model,
            &Usage {
                input: 7,
                model: None,
                ..Usage::default()
            },
        );

        assert_eq!(by_model["model-a"].input, 300, "same-model rows summed");
        assert_eq!(
            by_model["unknown"].input, 7,
            "model-less rows fall to unknown"
        );
        assert_eq!(by_model.len(), 2);
    }
}
