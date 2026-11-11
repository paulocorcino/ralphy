//! The run-snapshot publisher: the third destination on the ADR-0024 delivery
//! seam (ADR-0047 §2).
//!
//! It is not a new mechanism — the ring, the `DeliveryLayer` and the bounded
//! worker are the shared [`crate::delivery`] spine; this is one
//! [`DeliveryEngine`] fold that projects its own `RunState` into the versioned
//! document and writes it atomically under the repo's `.ralphy/`. The write is
//! therefore off the run path, coalesced by the 250 ms poll, and best-effort:
//! a failure warns ONCE per run and never reaches the run.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ralphy_run_snapshot::RunSnapshot;
use tracing::warn;

use crate::delivery::{spawn_worker, DeliveryEngine, EventQueue, WorkerHandle};
use crate::plan_progress::{self, PlanFileWatch, PlanProgress, PlanStep, StepStatus};
use crate::runstate::snapshot::{project, SnapshotCtx};
use crate::runstate::{RunEvent, RunState};

/// The snapshot fold (ADR-0024, ADR-0047 §2): folds each drained event into its
/// own [`RunState`] exactly as the Telegram engine does, and rewrites the
/// document whenever the projection differs from the one last written.
///
/// It also carries the run's plan progress (#330): the fold cannot, since it is
/// built from `tracing` events only and the checkboxes move on disk.
struct SnapshotEngine {
    ctx: SnapshotCtx,
    state: RunState,
    last: Option<RunSnapshot>,
    repo_root: PathBuf,
    warned: AtomicBool,
    /// Armed by `PlanWritten`/`Executing`, disarmed by `IssueStarted`: between
    /// issues `plan.md` still holds the PREVIOUS issue's plan.
    plan: PlanProgress,
    watch: PlanFileWatch,
    /// ABSOLUTE path of the run's plan (the document ships the relative one).
    plan_abs: PathBuf,
    /// The run's cooperative-stop sentinel (docs/adr/0054). This worker is the
    /// SETTER of `ralphy_core::stop`: it already ticks at 250 ms and it is the
    /// one component holding both the runid and the repo root, so the stop needs
    /// no thread and no cadence of its own.
    stop_path: PathBuf,
    /// Latch. Once the flag is set nothing can un-set it, so the `exists()` call
    /// is pure cost from then on — this drops it to zero for the rest of the run.
    stop_seen: bool,
}

impl SnapshotEngine {
    /// Project and write, but only when the document would differ from the last
    /// one written — an unchanged fold costs no write (ADR-0047 §8).
    fn publish(&mut self, force: bool) {
        let mut next = project(&self.ctx, &self.state, &self.plan);
        carry_phase_since(self.last.as_ref(), &mut next, || {
            chrono::Local::now().to_rfc3339()
        });
        if !force && self.last.as_ref() == Some(&next) {
            return;
        }
        match ralphy_run_snapshot::write_atomic(&self.repo_root, &next) {
            Ok(()) => self.last = Some(next),
            // Best-effort by decision: a full disk delays a panel update, never
            // the run. `last` is left alone so the next tick retries.
            Err(e) => warn_once(&self.warned, &e),
        }
    }
}

impl DeliveryEngine for SnapshotEngine {
    fn on_start(&mut self) {
        // Force the first write so the run appears in the panel before its first
        // event, even though the projection has not "changed" yet.
        self.publish(true);
    }

    fn on_event(&mut self, event: RunEvent) {
        // Disarm BEFORE the apply: the plan still on disk is the previous issue's.
        if matches!(event, RunEvent::IssueStarted { .. }) {
            self.plan = PlanProgress::default();
            self.watch = PlanFileWatch::default();
        }
        // The arming key is the EVENT's issue number, resolving `0` (the
        // adapter never learns it) through the fold's active issue: the ring
        // drops its oldest entry when full, and an `IssueStarted` lost that way
        // would leave `state.active` on the PREVIOUS issue — publishing this
        // plan under its number. Keyed this way the projection's own
        // `plan.issue == state.active` check becomes an independent guard, and
        // the mismatch publishes NO plan rather than the wrong one.
        let seed = match &event {
            RunEvent::PlanWritten { number, steps, .. } => Some((*number, Some(steps.clone()))),
            RunEvent::Executing { number, .. } => Some((*number, None)),
            RunEvent::QueueBuilt { .. }
            | RunEvent::IssueStarted { .. }
            | RunEvent::Planning { .. }
            | RunEvent::IssueClosed { .. }
            | RunEvent::NonGreen { .. }
            | RunEvent::Skipped { .. }
            | RunEvent::HumanBlocked { .. }
            | RunEvent::NeedsSplit { .. }
            | RunEvent::Notice { .. }
            | RunEvent::DeadlinePassed { .. }
            | RunEvent::RunStopped { .. }
            | RunEvent::SleepStarted { .. }
            | RunEvent::SleepEnded
            | RunEvent::ApiDegraded
            | RunEvent::ApiRecovered
            | RunEvent::IdleReaped { .. }
            | RunEvent::HooksUnprovenAllowed { .. }
            | RunEvent::AgentState { .. }
            | RunEvent::KnowledgeConsolidating { .. }
            | RunEvent::KnowledgeConsolidated { .. }
            | RunEvent::RunStarted { .. }
            | RunEvent::RunFinished { .. }
            | RunEvent::RunSkipped { .. }
            | RunEvent::PlanOpened { .. }
            | RunEvent::PlanClosed { .. } => None,
        };
        self.state.apply(event);
        let seed = seed.map(|(number, steps)| {
            let issue = (number != 0).then_some(number).or(self.state.active);
            (issue, steps)
        });
        match seed {
            // A written plan arms the poll and seeds its baseline.
            Some((issue, Some(steps))) => {
                self.plan.issue = issue;
                self.plan.steps = steps
                    .iter()
                    .map(|(text, status)| PlanStep {
                        text: plan_progress::normalize_step(text),
                        status: StepStatus::from_wire(status),
                    })
                    .collect();
            }
            // An executing issue's plan on disk is certainly its own — arm the
            // poll even when this session resumed without a `PlanWritten`.
            Some((issue, None)) if self.plan.issue.is_none() => {
                self.plan.issue = issue;
            }
            Some((_, None)) | None => {}
        }
    }

    fn on_tick(&mut self, _changed: bool) {
        // The operator's stop (docs/adr/0054). Existence-only: `ralphy stop`
        // writes a JSON body for forensics, but a torn or truncated write must
        // never be the difference between stopping and not.
        //
        // This raises a PROCESS-WIDE flag from a worker thread; the run thread
        // reads it at its own poll sites and does the stopping. Nothing here
        // touches the run, kills a child, or writes repo state.
        if !self.stop_seen && self.stop_path.exists() {
            self.stop_seen = true;
            ralphy_core::stop::request();
        }
        // The poll only ever mutates `plan.steps`; the single write site stays
        // `publish`, which returns early when the projection is unchanged — so an
        // unchanged plan file still costs no write.
        if self.plan.issue.is_some() {
            if let Some(md) = self.watch.changed_text(&self.plan_abs) {
                self.plan.steps = plan_progress::parse_steps(&md);
            }
        }
        self.publish(false);
    }

    fn on_finish(&mut self) {
        self.publish(false);
    }
}

/// Stamp `next`'s phase anchor: carry the previous document's `since` while the
/// phase is the same one, and mint a fresh instant when it is not.
///
/// This is the whole of the panel's clock. It lives here rather than in the
/// projection because the projection is pure over the fold and the fold has no
/// clock (`runstate::snapshot`'s module doc) — and because the anchor is a fact
/// about a TRANSITION, which only a comparison of consecutive documents can see.
///
/// A phase is `(state, active)`: the same `"executing"` on a different issue is a
/// different activity and restamps, which is what makes the clock answer "how long
/// has THIS been running" rather than "how long since the phase word last changed".
///
/// Load-bearing property, and the reason `now` is a closure: when the phase has
/// not changed the anchor is byte-identical, so an otherwise-unchanged projection
/// stays unchanged and [`SnapshotEngine::publish`] still writes nothing. A clock
/// read straight into the field would rewrite the document four times a second.
fn carry_phase_since(
    prev: Option<&RunSnapshot>,
    next: &mut RunSnapshot,
    now: impl FnOnce() -> String,
) {
    let carried = prev
        .filter(|p| p.phase.state == next.phase.state && p.phase.active == next.phase.active)
        .and_then(|p| p.phase.since.clone());
    next.phase.since = Some(carried.unwrap_or_else(now));
}

/// Emit the single non-spamming write-failure warning for the run, under the
/// engine's OWN `tracing` target so [`crate::delivery::DeliveryLayer`]'s
/// self-target filter drops it instead of folding it back into the ring.
fn warn_once(warned: &AtomicBool, error: &std::io::Error) {
    if warned.swap(true, Ordering::SeqCst) {
        return;
    }
    warn!(
        target: "ralphy_cli::run::snapshot",
        error = %error,
        "could not write the run snapshot — the Runs panel will not see this run \
         (further failures silenced this run)"
    );
}

/// The engine's detach-warn hook (ADR-0024), under the same self-target.
fn detach_warn() {
    warn!(
        target: "ralphy_cli::run::snapshot",
        "snapshot worker did not finish in time — detaching"
    );
}

/// The run's removal guard (ADR-0047 §8), which also keeps the last document
/// in the run folder as its final snapshot (docs/adr/0085 D11). A failure to
/// keep it is logged and never fails the run.
pub(super) fn snapshot_guard(
    repo_root: &std::path::Path,
    runid: &str,
    run_dir: &std::path::Path,
) -> ralphy_run_snapshot::SnapshotGuard {
    ralphy_run_snapshot::SnapshotGuard::new(repo_root, runid).keep_final_in(run_dir, |e| {
        warn!(error = %e, "could not keep the final run snapshot in the run folder");
    })
}

/// Spawn the `"ralphy-snapshot"` worker draining `queue` through a
/// [`SnapshotEngine`]. A spawn failure leaves the installed Layer inert (the ring
/// just fills and drops) rather than aborting the run.
pub(super) fn try_start_snapshot(
    ctx: SnapshotCtx,
    state: RunState,
    repo_root: PathBuf,
    queue: Arc<EventQueue>,
    plan_abs: PathBuf,
) -> Option<WorkerHandle> {
    let stop_path = ralphy_run_snapshot::stop_path(&repo_root, &ctx.runid);
    let engine = SnapshotEngine {
        ctx,
        state,
        last: None,
        repo_root,
        warned: AtomicBool::new(false),
        plan: PlanProgress::default(),
        watch: PlanFileWatch::default(),
        plan_abs,
        stop_path,
        stop_seen: false,
    };
    spawn_worker("ralphy-snapshot", engine, queue, detach_warn)
}

#[cfg(test)]
mod tests;
