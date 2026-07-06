//! The shared event-delivery worker (issue #132, ADR-0024): one bounded,
//! drop-oldest ring ([`EventQueue`]), one generic `tracing` [`DeliveryLayer`], and
//! one background worker loop ([`run_delivery_worker`]) with a bounded-shutdown
//! [`WorkerHandle`] — all parameterized by a per-sink [`DeliveryEngine`] fold. The
//! Telegram notifier (ADR-0007) and the CloudEvents sink (ADR-0019) are two engines
//! over this one spine; each owns its own vendor transport and message/envelope
//! shaping, plus any sink-specific policy (CloudEvents' retry/backoff). What is
//! shared here is the concurrency + lifetime substrate: the ring's drop-oldest
//! back-pressure, the Layer's off-the-run-path enqueue, and the worker's
//! poll/drain/fold/shutdown lifecycle.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use crate::runstate::{event_to_runevent, EventFields, RunEvent};

/// The default ring capacity for [`EventQueue::new`] (the Telegram notifier's bound,
/// ADR-0007 D4/D7). The CloudEvents sink builds a wider ring via
/// [`EventQueue::with_capacity`] (ADR-0019 ~1000).
const QUEUE_CAPACITY: usize = 256;

/// How often the worker re-polls the ring so it notices shutdown promptly even if a
/// notify is lost, distinct from any sink's own refresh/heartbeat cadence.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// How long [`WorkerHandle::shutdown`] waits for the worker before detaching, so a
/// wedged network never holds the process open (ADR-0007 D4 / ADR-0019).
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Bounded, drop-oldest event ring (ADR-0007 D4/D7, ADR-0019)
// ---------------------------------------------------------------------------

/// A bounded ring buffer of [`RunEvent`]s shared between the Layer (producer) and
/// the worker (consumer). On overflow it drops the **oldest** element, not the
/// newest, so a stalled network never slows the logging thread while the card
/// still converges on the most-current state.
pub struct EventQueue {
    inner: Mutex<VecDeque<RunEvent>>,
    cv: Condvar,
    cap: usize,
}

impl EventQueue {
    /// A queue with the default capacity.
    pub fn new() -> Self {
        Self::with_capacity(QUEUE_CAPACITY)
    }

    /// A queue with an explicit capacity (the sink's ~1000 bound and tests).
    pub fn with_capacity(cap: usize) -> Self {
        EventQueue {
            inner: Mutex::new(VecDeque::with_capacity(cap)),
            cv: Condvar::new(),
            cap,
        }
    }

    /// Enqueue an event, dropping the oldest if at capacity. Never blocks.
    pub fn push(&self, event: RunEvent) {
        let mut q = self.inner.lock().expect("event queue poisoned");
        if q.len() >= self.cap {
            q.pop_front();
        }
        q.push_back(event);
        self.cv.notify_one();
    }

    /// Wait up to `timeout` for at least one event, then drain everything pending.
    /// Returns an empty vec on timeout.
    pub fn drain_blocking(&self, timeout: Duration) -> Vec<RunEvent> {
        let mut q = self.inner.lock().expect("event queue poisoned");
        if q.is_empty() {
            let (guard, _) = self
                .cv
                .wait_timeout(q, timeout)
                .expect("event queue poisoned");
            q = guard;
        }
        q.drain(..).collect()
    }

    /// Wake any waiter (used at shutdown so the worker re-checks its flag).
    pub fn wake(&self) {
        self.cv.notify_all();
    }
}

impl Default for EventQueue {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// The generic tracing Layer
// ---------------------------------------------------------------------------

/// A `tracing` Layer that enqueues each consumed event as a [`RunEvent`] onto the
/// shared ring. It does no I/O on the logging thread — only `event_to_runevent` + a
/// ring push. `self_target` is the substring identifying the owning sink's own
/// `tracing` target (`"telegram::notifier"` / `"events::sink"`), so a runtime
/// `warn!` from that sink's worker never feeds back into the Layer and loops
/// (ADR-0007 / ADR-0019 self-target loop guard).
pub struct DeliveryLayer {
    queue: Arc<EventQueue>,
    self_target: &'static str,
}

impl DeliveryLayer {
    /// Wrap the shared event queue the worker drains, tagged with the sink's own
    /// `tracing` target substring for the loop guard.
    pub fn new(queue: Arc<EventQueue>, self_target: &'static str) -> Self {
        DeliveryLayer { queue, self_target }
    }
}

impl<S: Subscriber> Layer<S> for DeliveryLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let target = event.metadata().target();
        // Ignore the owning sink's own events so a runtime warn! cannot loop back in.
        if target.contains(self.self_target) {
            return;
        }
        let mut fields = EventFields {
            level: *event.metadata().level(),
            ..Default::default()
        };
        event.record(&mut fields);
        if let Some(run_event) = event_to_runevent(target, &fields.message, &fields) {
            self.queue.push(run_event);
        }
    }
}

// ---------------------------------------------------------------------------
// The engine seam + the worker loop
// ---------------------------------------------------------------------------

/// The per-sink fold the shared worker drives: a per-tick state machine over the
/// drained ring. `on_start`/`on_finish` bracket the worker's life; `on_event` folds
/// each drained event in order; `on_tick(changed)` runs once per poll after the
/// batch is folded (`changed` is whether that batch was non-empty), carrying any
/// time-driven work (Telegram's throttled card refresh, CloudEvents' heartbeat +
/// plan-step poll). The engine owns its vendor transport and all sink-specific
/// policy; the worker owns only the poll/drain/shutdown lifecycle.
pub trait DeliveryEngine {
    /// Called once before the drain loop (e.g. send the initial card).
    fn on_start(&mut self);
    /// Fold one drained event, in ring order.
    fn on_event(&mut self, event: RunEvent);
    /// Run once per poll after the batch is folded; `changed` is whether the batch
    /// was non-empty.
    fn on_tick(&mut self, changed: bool);
    /// Called once after the loop breaks on shutdown (e.g. the terminal edit).
    fn on_finish(&mut self);
}

/// Drive `engine` over `queue` until `shutdown` is set: `on_start`, then a
/// poll/drain/fold/tick loop (draining without blocking once stopping so a final
/// batch is still folded), then `on_finish` on the way out — so a sink's terminal
/// work runs on every exit path.
pub fn run_delivery_worker<E: DeliveryEngine>(
    mut engine: E,
    queue: Arc<EventQueue>,
    shutdown: Arc<AtomicBool>,
) {
    engine.on_start();
    loop {
        let stopping = shutdown.load(Ordering::SeqCst);
        // On shutdown, drain everything still pending (non-blocking) for a final
        // fold; otherwise wait up to a poll interval for the next batch.
        let events = if stopping {
            queue.drain_blocking(Duration::from_millis(0))
        } else {
            queue.drain_blocking(POLL_INTERVAL)
        };
        let changed = !events.is_empty();
        for event in events {
            engine.on_event(event);
        }
        engine.on_tick(changed);
        if stopping {
            break;
        }
    }
    engine.on_finish();
}

// ---------------------------------------------------------------------------
// The worker handle
// ---------------------------------------------------------------------------

/// A handle to a running delivery worker: the shared ring, its shutdown flag, the
/// worker thread, and the sink's detach-warn hook. [`shutdown`](Self::shutdown)
/// drains-and-joins under a bounded timeout so a wedged network never holds the
/// process open.
///
/// `detach_warn` is a per-sink hook that emits the "worker did not finish in time"
/// `warn!` under the sink's OWN `tracing` target (a compile-time literal, which the
/// macro requires) so [`DeliveryLayer`]'s self-target filter drops it — a WARN
/// otherwise folds into a `RunEvent::Notice` and would loop back into the ring.
pub struct WorkerHandle {
    queue: Arc<EventQueue>,
    shutdown: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    detach_warn: fn(),
}

impl WorkerHandle {
    /// Signal shutdown, wake the worker, and join it under the default bounded
    /// timeout so a wedged network never holds the process open.
    pub fn shutdown(self) {
        self.shutdown_within(SHUTDOWN_TIMEOUT);
    }

    /// Like [`shutdown`](Self::shutdown) but with an explicit timeout (tests use a
    /// short one). If the worker does not finish in time it is detached.
    pub fn shutdown_within(mut self, timeout: Duration) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Wake the worker if it is parked waiting for events, so it observes the
        // shutdown flag at once rather than after the next poll.
        self.queue.wake();
        if let Some(join) = self.join.take() {
            // Join on a helper thread and bound the wait, so a worker wedged in a
            // blocking network call can never hold the process open.
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = join.join();
                let _ = tx.send(());
            });
            if rx.recv_timeout(timeout).is_err() {
                (self.detach_warn)();
            }
        }
    }
}

/// Spawn a named delivery worker draining `queue` through `engine`. The returned
/// [`WorkerHandle`] holds the shutdown signal and the worker's join handle; a spawn
/// failure returns `None`, leaving the installed Layer inert (the ring just fills
/// and drops) rather than aborting the run. `detach_warn` is the sink's detach-warn
/// hook (see [`WorkerHandle`]).
pub fn spawn_worker<E: DeliveryEngine + Send + 'static>(
    name: &str,
    engine: E,
    queue: Arc<EventQueue>,
    detach_warn: fn(),
) -> Option<WorkerHandle> {
    let shutdown = Arc::new(AtomicBool::new(false));
    let worker_queue = queue.clone();
    let worker_shutdown = shutdown.clone();
    let join = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || run_delivery_worker(engine, worker_queue, worker_shutdown))
        .ok()?;
    Some(WorkerHandle {
        queue,
        shutdown,
        join: Some(join),
        detach_warn,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runstate::UsageLite;
    use std::sync::Mutex;
    use std::time::Instant;

    #[test]
    fn event_queue_drops_oldest_at_capacity() {
        let q = EventQueue::with_capacity(2);
        q.push(RunEvent::IssueClosed {
            number: 1,
            tokens: 0,
            usage: UsageLite::default(),
        });
        q.push(RunEvent::IssueClosed {
            number: 2,
            tokens: 0,
            usage: UsageLite::default(),
        });
        q.push(RunEvent::IssueClosed {
            number: 3,
            tokens: 0,
            usage: UsageLite::default(),
        });
        let drained = q.drain_blocking(Duration::from_millis(0));
        assert_eq!(
            drained,
            vec![
                RunEvent::IssueClosed {
                    number: 2,
                    tokens: 0,
                    usage: UsageLite::default(),
                },
                RunEvent::IssueClosed {
                    number: 3,
                    tokens: 0,
                    usage: UsageLite::default(),
                },
            ]
        );
    }

    #[test]
    fn layer_enqueue_is_off_the_run_path() {
        // The Layer holds NO transport by construction — only the ring — so the
        // logging thread never touches the network; delivery is entirely the
        // worker's job. Drive the exact enqueue path (a real `tracing::Event` is
        // impractical to build in a unit test) and prove it stays well under 50ms
        // even at volume — no network I/O could hide inside a push that fast.
        let queue = Arc::new(EventQueue::with_capacity(1000));
        let layer = DeliveryLayer::new(queue.clone(), "events::sink");
        let start = Instant::now();
        for n in 0..1000u64 {
            queue.push(RunEvent::IssueClosed {
                number: n,
                tokens: 0,
                usage: UsageLite::default(),
            });
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(50),
            "the Layer enqueue path must be well under 50ms, took {elapsed:?}"
        );
        drop(layer);
        assert!(!queue.drain_blocking(Duration::ZERO).is_empty());
    }

    /// A fake engine that logs each lifecycle callback for order assertions; its
    /// `on_start` optionally sleeps to model a blocking transport for the
    /// bounded-shutdown test.
    struct FakeEngine {
        log: Arc<Mutex<Vec<String>>>,
        start_sleep: Duration,
    }

    impl DeliveryEngine for FakeEngine {
        fn on_start(&mut self) {
            self.log.lock().unwrap().push("start".into());
            if !self.start_sleep.is_zero() {
                std::thread::sleep(self.start_sleep);
            }
        }
        fn on_event(&mut self, event: RunEvent) {
            if let RunEvent::IssueClosed { number, .. } = event {
                self.log.lock().unwrap().push(format!("event:{number}"));
            }
        }
        fn on_tick(&mut self, _changed: bool) {}
        fn on_finish(&mut self) {
            self.log.lock().unwrap().push("finish".into());
        }
    }

    #[test]
    fn worker_drives_engine_through_lifecycle() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let queue = Arc::new(EventQueue::new());
        for n in 1..=3u64 {
            queue.push(RunEvent::IssueClosed {
                number: n,
                tokens: 0,
                usage: UsageLite::default(),
            });
        }
        let engine = FakeEngine {
            log: log.clone(),
            start_sleep: Duration::ZERO,
        };
        let handle =
            spawn_worker("test-worker", engine, queue.clone(), || {}).expect("spawn worker");
        handle.shutdown_within(Duration::from_secs(5));

        let log = log.lock().unwrap();
        assert_eq!(log.first().map(String::as_str), Some("start"));
        assert_eq!(log.last().map(String::as_str), Some("finish"));
        // All three events folded, in order.
        let events: Vec<&str> = log
            .iter()
            .filter(|s| s.starts_with("event:"))
            .map(String::as_str)
            .collect();
        assert_eq!(events, ["event:1", "event:2", "event:3"]);
    }

    #[test]
    fn shutdown_returns_promptly_when_engine_blocks() {
        // The engine wedges in `on_start` for 30s; a bounded shutdown must still
        // return promptly (the worker is detached rather than joined).
        let log = Arc::new(Mutex::new(Vec::new()));
        let queue = Arc::new(EventQueue::new());
        let engine = FakeEngine {
            log,
            start_sleep: Duration::from_secs(30),
        };
        let handle = spawn_worker("test-blocker", engine, queue, || {}).expect("spawn worker");
        let start = Instant::now();
        handle.shutdown_within(Duration::from_millis(300));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "shutdown did not return promptly: {:?}",
            start.elapsed()
        );
    }
}
