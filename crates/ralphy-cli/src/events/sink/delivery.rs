//! The sink's delivery engine (ADR-0019, ADR-0024): a [`DeliveryEngine`] fold that
//! maps each drained event into a CloudEvents envelope and POSTs it with retry, plus
//! the ~30s heartbeat and the plan-step poll on each tick. The ring, worker loop, and
//! bounded-shutdown handle are the shared [`crate::delivery`] spine.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tracing::warn;

use super::poller::StepPoller;
use crate::delivery::{spawn_worker, DeliveryEngine, EventQueue, WorkerHandle};
use crate::events::client::{EventSink, PostOutcome};
use crate::events::envelope::{runevent_to_cloudevent, EventCtx};
use crate::runstate::{RunEvent, RunState, UsageLite};

/// The heartbeat cadence (ADR-0019: ~30s, carried on the wire as `interval_s` so a
/// consumer never hardcodes it).
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Total delivery attempts per event: one initial POST plus three retries
/// (ADR-0019 / docs/events.md transport contract).
const MAX_ATTEMPTS: u32 = 4;

/// The first retry backoff; each subsequent retry doubles it (1s, 2s, 4s).
pub(super) const RETRY_BASE_BACKOFF: Duration = Duration::from_secs(1);

/// A running four-field token total (`up`/`cr`/`cw`/`out`) the worker accumulates
/// across the run for the heartbeat and never resets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Totals {
    up: u64,
    cr: u64,
    cw: u64,
    out: u64,
}

impl Totals {
    /// Fold one phase's usage breakdown into the running total.
    fn add(&mut self, u: &UsageLite) {
        self.up += u.input;
        self.cr += u.cache_read;
        self.cw += u.cache_creation;
        self.out += u.output;
    }

    fn to_json(self) -> Value {
        json!({ "up": self.up, "cr": self.cr, "cw": self.cw, "out": self.out })
    }
}

/// The run's current phase for the heartbeat (ADR-0019): a usage-limit sleep wins,
/// then an in-progress consolidation, then the active issue's phase, else the
/// initial `starting`. A sleep reports `sleeping` even with an executing issue so a
/// long usage-limit pause is never mistaken for progress or for death.
fn phase(state: &RunState) -> &'static str {
    state.run_phase()
}

/// Build a `run.heartbeat` envelope from the folded state and running totals: the
/// [`phase`], the emitter's own `interval_s` cadence, the active issue, elapsed
/// seconds, queue progress, and the token totals (docs/events.md `run.heartbeat`).
fn heartbeat(ctx: &EventCtx, state: &RunState, tokens: Totals, elapsed_s: u64) -> Value {
    let queue_done = state
        .issues
        .iter()
        .filter(|e| e.status.is_terminal())
        .count();
    // The active issue as `{number, title}` (the normalized `issue` shape, #96) or
    // `null` when no issue is active — replacing the bare active number.
    let issue = state
        .active_issue()
        .map(|e| json!({ "number": e.number, "title": e.title }))
        .unwrap_or(Value::Null);
    crate::events::envelope::run_envelope(
        "dev.ralphy.run.heartbeat",
        ctx,
        state,
        json!({
            "phase": phase(state),
            "interval_s": HEARTBEAT_INTERVAL.as_secs(),
            "issue": issue,
            "elapsed_s": elapsed_s,
            "queue_done": queue_done,
            "queue_total": state.total,
            "tokens_total": tokens.to_json(),
        }),
    )
}

/// The CloudEvents sink fold (ADR-0019, ADR-0024): folds each drained event into a
/// local [`RunState`] (so adapter events carrying issue `0` resolve to the active
/// issue), maps it to a CloudEvents envelope, and POSTs it with retry; each tick
/// runs the plan-step poll and the ~30s heartbeat timer. Owns its own
/// [`EventSink`] transport and the run's drop-guard `warned` flag.
struct CloudEventsEngine<T: EventSink> {
    transport: T,
    ctx: EventCtx,
    state: RunState,
    tokens: Totals,
    start: Instant,
    last_beat: Instant,
    /// The plan-step poller diffs the active issue's `plan.md` mtime each tick (#96).
    poller: StepPoller,
    /// One warn per run on the first delivery drop; later drops stay silent.
    warned: AtomicBool,
    plan_path: PathBuf,
}

impl<T: EventSink> DeliveryEngine for CloudEventsEngine<T> {
    fn on_start(&mut self) {
        // Clock the run + heartbeat baselines at worker entry (not at construction),
        // matching the old `run_sender` so `elapsed_s` and the first heartbeat are not
        // skewed by thread-spawn latency.
        self.start = Instant::now();
        self.last_beat = Instant::now();
    }

    fn on_event(&mut self, event: RunEvent) {
        // Accumulate the run's token totals off the two phases that report a
        // usage breakdown, for the heartbeat's `tokens_total`.
        if let RunEvent::PlanWritten { usage, .. } | RunEvent::IssueClosed { usage, .. } = &event {
            self.tokens.add(usage);
        }
        // A freshly written plan reseeds the poller baseline so its initial state
        // is not mistaken for a burst of transitions (#96).
        if let RunEvent::PlanWritten { steps, .. } = &event {
            self.poller.reset_from_written(steps);
        }
        // Fold first so the adapter events that carry issue `0` resolve to the
        // active issue when mapped.
        self.state.apply(event.clone());
        if let Some(cloudevent) = runevent_to_cloudevent(&event, &self.ctx, &self.state) {
            deliver(
                &self.transport,
                &cloudevent,
                &self.warned,
                RETRY_BASE_BACKOFF,
            );
        }
    }

    fn on_tick(&mut self, _changed: bool) {
        // The plan-step poll rides the same tick as the heartbeat: cheap when the
        // plan is unchanged (an mtime stat), emitting only on a checkbox transition.
        self.poller.poll(
            &self.transport,
            &self.ctx,
            &self.state,
            &self.plan_path,
            &self.warned,
        );
        // The heartbeat fires on its own 30s timer, independent of event arrival —
        // so it keeps beating through a silent usage-limit sleep (`phase: sleeping`)
        // and a consumer never mistakes a long pause for a dead run.
        if self.last_beat.elapsed() >= HEARTBEAT_INTERVAL {
            let beat = heartbeat(
                &self.ctx,
                &self.state,
                self.tokens,
                self.start.elapsed().as_secs(),
            );
            deliver(&self.transport, &beat, &self.warned, RETRY_BASE_BACKOFF);
            self.last_beat = Instant::now();
        }
    }

    fn on_finish(&mut self) {}
}

/// Deliver one envelope through the transport with the at-most-once retry policy
/// (docs/events.md): retry a [`PostOutcome::Transient`] up to [`MAX_ATTEMPTS`]
/// times with exponential backoff, drop a [`PostOutcome::Permanent`] (a `4xx`
/// config error) immediately, and drop after exhaustion. Any drop emits at most one
/// `warn!` per run via `warned`. Returns the number of attempts made (a test seam).
///
/// The `warn!` target embeds the sink's own module so the sink's
/// [`crate::delivery::DeliveryLayer`] filters it out of the ring — the drop notice
/// reaches `ralphy.log` without feeding back into the sink and looping.
pub(super) fn deliver<T: EventSink>(
    transport: &T,
    cloudevent: &Value,
    warned: &AtomicBool,
    base_backoff: Duration,
) -> u32 {
    let mut backoff = base_backoff;
    for attempt in 1..=MAX_ATTEMPTS {
        match transport.post(cloudevent) {
            Ok(PostOutcome::Delivered) => return attempt,
            // A 4xx is a configuration error: drop without retry.
            Ok(PostOutcome::Permanent) => {
                warn_dropped(warned);
                return attempt;
            }
            Ok(PostOutcome::Transient) => {
                if attempt == MAX_ATTEMPTS {
                    warn_dropped(warned);
                    return attempt;
                }
                std::thread::sleep(backoff);
                backoff = backoff.saturating_mul(2);
            }
            // A transport-level error (e.g. a body that failed to serialize) will
            // not fix on retry: drop it once, like a permanent failure.
            Err(_) => {
                warn_dropped(warned);
                return attempt;
            }
        }
    }
    MAX_ATTEMPTS
}

/// Emit the single non-spamming drop warning for the run: the first drop warns,
/// every later drop is silent. Returns whether this call emitted the warning (a
/// test seam proving "exactly one warn per run"). The `swap` makes the flip atomic
/// so two concurrent drops still warn only once.
fn warn_dropped(warned: &AtomicBool) -> bool {
    if warned.swap(true, Ordering::SeqCst) {
        return false;
    }
    warn!(
        target: "ralphy_cli::events::sink",
        "dropping CloudEvents delivery after retries — endpoint unreachable or rejecting \r
         (further drops silenced this run)"
    );
    true
}

/// The sink's detach-warn hook (ADR-0024): emits the "worker did not finish"
/// `warn!` under the sink's OWN `tracing` target so [`crate::delivery::DeliveryLayer`]'s
/// self-target filter drops it instead of folding it into a `RunEvent::Notice` and
/// looping it back into the ring.
fn detach_warn() {
    warn!(target: "ralphy_cli::events::sink", "sink worker did not finish in time — detaching");
}

/// Spawn the `"ralphy-events"` worker draining `queue` through a
/// [`CloudEventsEngine`] over `transport`. The returned [`WorkerHandle`] holds the
/// shutdown signal and the worker's join handle; a spawn failure leaves the
/// installed Layer inert (the ring just fills and drops) rather than aborting the
/// run.
pub fn try_start_sink<T: EventSink + Send + 'static>(
    transport: T,
    ctx: EventCtx,
    queue: Arc<EventQueue>,
    plan_path: PathBuf,
) -> Option<WorkerHandle> {
    let engine = CloudEventsEngine {
        transport,
        ctx,
        state: RunState::default(),
        tokens: Totals::default(),
        start: Instant::now(),
        last_beat: Instant::now(),
        poller: StepPoller::default(),
        warned: AtomicBool::new(false),
        plan_path,
    };
    spawn_worker("ralphy-events", engine, queue, detach_warn)
}

#[cfg(test)]
mod tests;
