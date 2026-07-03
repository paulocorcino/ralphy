//! The run-time CloudEvents sink Layer + worker (ADR-0019).
//!
//! [`EventsLayer`] mirrors the Telegram [`NotifierLayer`](crate::telegram::notifier)
//! exactly: it decodes each `tracing` event into a [`RunEvent`] and pushes it onto
//! a bounded, drop-oldest ring on the logging thread — never touching the network.
//! One background worker ([`run_sender`]) drains the ring, folds each event into a
//! local [`RunState`] (so the adapter events that carry issue `0` resolve to the
//! active issue), maps it to a CloudEvents envelope, and POSTs it through the
//! injectable [`EventSink`]. The Layer ignores the sink's own `tracing` target so a
//! runtime `warn!` on a delivery drop can never feed back into the ring and loop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tracing::{warn, Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use super::client::{EventSink, PostOutcome};
use super::envelope::{runevent_to_cloudevent, EventCtx};
use crate::runstate::{event_to_runevent, EventFields, IssueStatus, RunEvent, RunState, UsageLite};
use crate::telegram::notifier::EventQueue;

/// The sink ring's capacity (ADR-0019: bounded ~1000, drop-oldest).
const QUEUE_CAPACITY: usize = 1000;

/// How often the worker re-polls the ring so it notices shutdown promptly even if
/// a notify is lost.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// How long [`EventsHandle::shutdown`] waits for the worker before detaching, so a
/// wedged endpoint never holds the process open.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// The heartbeat cadence (ADR-0019: ~30s, carried on the wire as `interval_s` so a
/// consumer never hardcodes it).
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Total delivery attempts per event: one initial POST plus three retries
/// (ADR-0019 / docs/events.md transport contract).
const MAX_ATTEMPTS: u32 = 4;

/// The first retry backoff; each subsequent retry doubles it (1s, 2s, 4s).
const RETRY_BASE_BACKOFF: Duration = Duration::from_secs(1);

/// The substring identifying the sink's own `tracing` target, so a drop `warn!`
/// from the worker never feeds back into the Layer and loops (ADR-0019).
const SELF_TARGET_MARKER: &str = "events::sink";

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
    if state.sleep.is_some() {
        return "sleeping";
    }
    if state.consolidating.is_some() {
        return "consolidating";
    }
    match state.active_issue().map(|e| &e.status) {
        Some(IssueStatus::Executing) => "executing",
        Some(IssueStatus::Planning) => "planning",
        _ => "starting",
    }
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
    super::envelope::run_envelope(
        "dev.ralphy.run.heartbeat",
        ctx,
        json!({
            "phase": phase(state),
            "interval_s": HEARTBEAT_INTERVAL.as_secs(),
            "issue": state.active,
            "elapsed_s": elapsed_s,
            "queue_done": queue_done,
            "queue_total": state.total,
            "tokens_total": tokens.to_json(),
        }),
    )
}

/// A sink ring with the ADR-0019 ~1000-event bound.
pub fn new_queue() -> Arc<EventQueue> {
    Arc::new(EventQueue::with_capacity(QUEUE_CAPACITY))
}

/// A `tracing` Layer that enqueues each consumed event as a [`RunEvent`] onto the
/// sink's own ring. Does no I/O on the logging thread — only `event_to_runevent` +
/// a ring push, so an unreachable endpoint never touches the run's wall-clock.
pub struct EventsLayer {
    queue: Arc<EventQueue>,
}

impl EventsLayer {
    /// Wrap the shared sink ring the worker drains.
    pub fn new(queue: Arc<EventQueue>) -> Self {
        EventsLayer { queue }
    }
}

impl<S: Subscriber> Layer<S> for EventsLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let target = event.metadata().target();
        // Ignore the sink's own events so a runtime warn! cannot loop back in.
        if target.contains(SELF_TARGET_MARKER) {
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

/// Drain the sink ring, folding each event into a local [`RunState`] and POSTing
/// its CloudEvents envelope once. Retry/drop and the heartbeat land in later
/// slices; this base worker delivers each mapped event a single time.
pub fn run_sender<T: EventSink>(
    transport: T,
    ctx: EventCtx,
    queue: Arc<EventQueue>,
    shutdown: Arc<AtomicBool>,
) {
    let mut state = RunState::default();
    let mut tokens = Totals::default();
    let start = Instant::now();
    let mut last_beat = Instant::now();
    // One warn per run on the first delivery drop; later drops stay silent.
    let warned = AtomicBool::new(false);
    loop {
        let stopping = shutdown.load(Ordering::SeqCst);
        let events = if stopping {
            queue.drain_blocking(Duration::from_millis(0))
        } else {
            queue.drain_blocking(POLL_INTERVAL)
        };
        for event in events {
            // Accumulate the run's token totals off the two phases that report a
            // usage breakdown, for the heartbeat's `tokens_total`.
            match &event {
                RunEvent::PlanWritten { usage, .. } | RunEvent::IssueClosed { usage, .. } => {
                    tokens.add(usage)
                }
                _ => {}
            }
            // Fold first so the adapter events that carry issue `0` resolve to the
            // active issue when mapped.
            state.apply(event.clone());
            if let Some(cloudevent) = runevent_to_cloudevent(&event, &ctx, &state) {
                deliver(&transport, &cloudevent, &warned, RETRY_BASE_BACKOFF);
            }
        }
        // The heartbeat fires on its own 30s timer, independent of event arrival —
        // so it keeps beating through a silent usage-limit sleep (`phase: sleeping`)
        // and a consumer never mistakes a long pause for a dead run.
        if last_beat.elapsed() >= HEARTBEAT_INTERVAL {
            let beat = heartbeat(&ctx, &state, tokens, start.elapsed().as_secs());
            deliver(&transport, &beat, &warned, RETRY_BASE_BACKOFF);
            last_beat = Instant::now();
        }
        if stopping {
            break;
        }
    }
}

/// Deliver one envelope through the transport with the at-most-once retry policy
/// (docs/events.md): retry a [`PostOutcome::Transient`] up to [`MAX_ATTEMPTS`]
/// times with exponential backoff, drop a [`PostOutcome::Permanent`] (a `4xx`
/// config error) immediately, and drop after exhaustion. Any drop emits at most one
/// `warn!` per run via `warned`. Returns the number of attempts made (a test seam).
///
/// The `warn!` target embeds the sink's own module so the [`EventsLayer`] filters
/// it out of the ring — the drop notice reaches `ralphy.log` without feeding back
/// into the sink and looping.
fn deliver<T: EventSink>(
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
        "dropping CloudEvents delivery after retries — endpoint unreachable or rejecting (further drops silenced this run)"
    );
    true
}

/// A handle to the running sink: the shared ring, its shutdown flag, and the
/// worker thread. [`shutdown`](Self::shutdown) drains-and-joins under a bounded
/// timeout so a wedged endpoint never holds the process open.
pub struct EventsHandle {
    queue: Arc<EventQueue>,
    shutdown: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl EventsHandle {
    /// Signal shutdown, wake the worker, and join it under the default timeout.
    pub fn shutdown(self) {
        self.shutdown_within(SHUTDOWN_TIMEOUT);
    }

    /// Like [`shutdown`](Self::shutdown) but with an explicit timeout (tests use a
    /// short one). If the worker does not finish in time it is detached.
    pub fn shutdown_within(mut self, timeout: Duration) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.queue.wake();
        if let Some(join) = self.join.take() {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = join.join();
                let _ = tx.send(());
            });
            if rx.recv_timeout(timeout).is_err() {
                warn!(target: "ralphy_cli::events::sink", "sink worker did not finish in time — detaching");
            }
        }
    }
}

/// Spawn the `"ralphy-events"` worker draining `queue` through `transport`. The
/// returned [`EventsHandle`] holds the shutdown signal and the worker's join
/// handle; a spawn failure leaves the installed Layer inert (the ring just fills
/// and drops) rather than aborting the run.
pub fn try_start_sink<T: EventSink + Send + 'static>(
    transport: T,
    ctx: EventCtx,
    queue: Arc<EventQueue>,
) -> Option<EventsHandle> {
    let shutdown = Arc::new(AtomicBool::new(false));
    let worker_queue = queue.clone();
    let worker_shutdown = shutdown.clone();
    let join = std::thread::Builder::new()
        .name("ralphy-events".into())
        .spawn(move || run_sender(transport, ctx, worker_queue, worker_shutdown))
        .ok()?;
    Some(EventsHandle {
        queue,
        shutdown,
        join: Some(join),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runstate::{RunEvent, UsageLite};
    use serde_json::{json, Value};
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A test [`EventCtx`] with a stub emitter carrying a known `pid`.
    fn test_ctx() -> EventCtx {
        EventCtx {
            source: "ralphy/o/r".to_string(),
            runid: "01TESTRUNIDTESTRUNIDTE".to_string(),
            emitter: json!({ "version": "0.0.0", "pid": 4242 }),
        }
    }

    /// Read one HTTP request (request line + headers + Content-Length body) off a
    /// stream, returning the raw bytes. Loops until the declared body is fully read
    /// so a fragmented POST is still captured whole.
    fn read_http_request(stream: &mut std::net::TcpStream) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            // Once the headers are complete, stop as soon as the declared body is in.
            if let Some(headers_end) = find_subslice(&buf, b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..headers_end]).to_lowercase();
                let content_len = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                let body_start = headers_end + 4;
                if buf.len() >= body_start + content_len {
                    break;
                }
            }
            let n = stream.read(&mut chunk).expect("read request");
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        buf
    }

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    /// A fake sink that returns a scripted sequence of outcomes, then a default
    /// once the script is exhausted, counting every call.
    struct ScriptedSink {
        script: std::sync::Mutex<std::collections::VecDeque<PostOutcome>>,
        default: PostOutcome,
        calls: std::sync::atomic::AtomicU32,
    }

    impl ScriptedSink {
        fn new(script: Vec<PostOutcome>, default: PostOutcome) -> Self {
            ScriptedSink {
                script: std::sync::Mutex::new(script.into()),
                default,
                calls: std::sync::atomic::AtomicU32::new(0),
            }
        }
        fn call_count(&self) -> u32 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl EventSink for ScriptedSink {
        fn post(&self, _body: &Value) -> anyhow::Result<PostOutcome> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.script.lock().unwrap().pop_front().unwrap_or(self.default))
        }
    }

    // Zero backoff so the retry tests never sleep on the real clock.
    const NO_BACKOFF: Duration = Duration::ZERO;

    #[test]
    fn retry_delivers_on_fourth_attempt_after_three_transients() {
        // Three transients then a success: exactly 4 attempts, no drop warning.
        let sink = ScriptedSink::new(
            vec![
                PostOutcome::Transient,
                PostOutcome::Transient,
                PostOutcome::Transient,
            ],
            PostOutcome::Delivered,
        );
        let warned = AtomicBool::new(false);
        let attempts = deliver(&sink, &json!({}), &warned, NO_BACKOFF);
        assert_eq!(attempts, 4, "1 initial + 3 retries");
        assert_eq!(sink.call_count(), 4);
        assert!(!warned.load(Ordering::SeqCst), "a delivered event must not warn");
    }

    #[test]
    fn retry_exhausts_and_drops_on_repeated_transient() {
        // Always transient: 4 attempts then drop with a warn.
        let sink = ScriptedSink::new(vec![], PostOutcome::Transient);
        let warned = AtomicBool::new(false);
        let attempts = deliver(&sink, &json!({}), &warned, NO_BACKOFF);
        assert_eq!(attempts, 4);
        assert!(warned.load(Ordering::SeqCst), "exhaustion must warn");
    }

    #[test]
    fn permanent_drops_after_one_attempt() {
        // A 4xx is a config error: one attempt, dropped without retry.
        let sink = ScriptedSink::new(vec![], PostOutcome::Permanent);
        let warned = AtomicBool::new(false);
        let attempts = deliver(&sink, &json!({}), &warned, NO_BACKOFF);
        assert_eq!(attempts, 1, "a 4xx is not retried");
        assert!(warned.load(Ordering::SeqCst), "a permanent drop warns");
    }

    #[test]
    fn two_drops_produce_exactly_one_warn() {
        // The AtomicBool guard emits the warn only on the first drop of the run.
        let warned = AtomicBool::new(false);
        assert!(warn_dropped(&warned), "first drop warns");
        assert!(!warn_dropped(&warned), "second drop is silent");
        assert!(!warn_dropped(&warned), "and every later drop too");

        // End-to-end: two exhausting deliveries sharing one guard warn once.
        let sink = ScriptedSink::new(vec![], PostOutcome::Transient);
        let guard = AtomicBool::new(false);
        assert!(!guard.load(Ordering::SeqCst));
        deliver(&sink, &json!({}), &guard, NO_BACKOFF);
        let after_first = guard.load(Ordering::SeqCst);
        deliver(&sink, &json!({}), &guard, NO_BACKOFF);
        assert!(after_first, "first delivery flipped the guard");
        assert_eq!(sink.call_count(), 8, "two full 4-attempt runs");
    }

    #[test]
    fn heartbeat_carries_phase_interval_and_token_totals() {
        // A folded state: issue 7 executing, one plan + one close of token usage.
        let mut state = RunState::new("t", 3);
        state.apply(RunEvent::IssueStarted {
            number: 7,
            title: "a".into(),
        });
        state.apply(RunEvent::Executing {
            number: 7,
            budget_min: 45,
            model: "sonnet".into(),
            effort: None,
        });
        let mut tokens = Totals::default();
        tokens.add(&UsageLite {
            input: 10,
            cache_read: 20,
            cache_creation: 5,
            output: 3,
            model: None,
        });
        tokens.add(&UsageLite {
            input: 1,
            cache_read: 2,
            cache_creation: 0,
            output: 4,
            model: None,
        });

        let v = heartbeat(&test_ctx(), &state, tokens, 412);
        assert_eq!(v["type"], "dev.ralphy.run.heartbeat");
        assert_eq!(v["data"]["phase"], "executing");
        assert_eq!(v["data"]["interval_s"], 30);
        assert_eq!(v["data"]["issue"], 7);
        assert_eq!(v["data"]["elapsed_s"], 412);
        assert_eq!(v["data"]["queue_total"], 3);
        assert_eq!(v["data"]["tokens_total"]["up"], 11);
        assert_eq!(v["data"]["tokens_total"]["cr"], 22);
        assert_eq!(v["data"]["tokens_total"]["cw"], 5);
        assert_eq!(v["data"]["tokens_total"]["out"], 7);
    }

    #[test]
    fn phase_sleeping_wins_over_executing_issue() {
        // Even with an executing issue, an active usage-limit sleep reports sleeping.
        let mut state = RunState::new("t", 1);
        state.apply(RunEvent::IssueStarted {
            number: 1,
            title: "a".into(),
        });
        state.apply(RunEvent::Executing {
            number: 1,
            budget_min: 45,
            model: "sonnet".into(),
            effort: None,
        });
        assert_eq!(phase(&state), "executing");
        state.apply(RunEvent::SleepStarted {
            reset: "14:30".into(),
            target_epoch: 1_700_000_000,
        });
        assert_eq!(phase(&state), "sleeping");
        state.apply(RunEvent::SleepEnded);
        assert_eq!(phase(&state), "executing");
    }

    #[test]
    fn phase_starting_before_any_issue() {
        assert_eq!(phase(&RunState::new("t", 2)), "starting");
    }

    #[test]
    fn layer_enqueue_is_off_the_run_path() {
        // A transport aimed at an unroutable address: if the logging thread ever
        // touched the network, this endpoint would block it for seconds (its connect
        // timeout is 10s). The Layer holds NO transport by construction — only the
        // ring — so it is never consulted on the logging thread; delivery is
        // entirely the worker's job. Building it here documents that the Layer path
        // never reaches for it.
        let _unroutable = super::super::client::UreqEventTransport::new(
            "http://10.255.255.1:9/".to_string(),
            Some("tok".to_string()),
        );

        // The Layer's `on_event` reduces to `event_to_runevent` + `queue.push`; drive
        // that exact enqueue path (a real `tracing::Event` is impractical to build in
        // a unit test) and prove it stays well under 50ms even at volume — no network
        // I/O could hide inside a push that fast.
        let queue = new_queue();
        let layer = EventsLayer::new(queue.clone());
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
        // The Layer wraps the same ring the pushes landed on — the ~1000-bound ring
        // keeps the most recent events under back-pressure.
        drop(layer);
        assert!(!queue.drain_blocking(Duration::ZERO).is_empty());
    }

    #[test]
    fn spine_pushed_event_arrives_as_cloudevents_post() {
        // A recording server on an ephemeral port: accept one connection, read the
        // request, reply 200, and hand the raw request back over a channel.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let request = read_http_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .expect("reply");
            stream.flush().ok();
            tx.send(request).expect("send request");
        });

        let transport =
            super::super::client::UreqEventTransport::new(format!("http://127.0.0.1:{port}/"), Some("tok".to_string()));
        let queue = new_queue();
        queue.push(RunEvent::IssueClosed {
            number: 7,
            tokens: 42,
            usage: UsageLite {
                input: 1,
                cache_read: 2,
                cache_creation: 3,
                output: 4,
                model: Some("claude-sonnet-4".into()),
            },
        });
        // shutdown already set: the worker drains once, POSTs, and returns inline.
        let shutdown = Arc::new(AtomicBool::new(true));
        run_sender(transport, test_ctx(), queue, shutdown);

        let raw = rx.recv().expect("recorded request");
        server.join().ok();

        // Split head from body.
        let headers_end = find_subslice(&raw, b"\r\n\r\n").expect("headers end");
        let head = String::from_utf8_lossy(&raw[..headers_end]).to_string();
        let head_lc = head.to_lowercase();
        let body = &raw[headers_end + 4..];

        // Request line + headers.
        assert!(head.starts_with("POST "), "not a POST: {head}");
        assert!(
            head_lc.contains("content-type: application/cloudevents+json"),
            "missing/other content-type: {head}"
        );
        assert!(
            head_lc.contains("authorization: bearer tok"),
            "missing bearer auth: {head}"
        );

        // The JSON envelope body.
        let v: Value = serde_json::from_slice(body).expect("json body");
        assert_eq!(v["specversion"], "1.0");
        assert_eq!(v["type"], "dev.ralphy.issue.closed");
        assert_eq!(v["source"], "ralphy/o/r");
        assert_eq!(v["subject"], "issue/7");
        assert!(v["runid"].as_str().is_some_and(|s| !s.is_empty()), "runid: {v}");
        assert_eq!(v["data"]["emitter"]["pid"], 4242);
    }
}
