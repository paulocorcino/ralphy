//! The run-time Telegram notifier (ADR-0007 D1, D3, D4, D6, D7).
//!
//! A [`NotifierLayer`] installed in `main.rs` translates each `tracing` event into
//! a [`RunEvent`] and pushes it onto a bounded, drop-oldest [`EventQueue`]. A
//! single background worker ([`run_worker`]) owns the card's `message_id`, folds
//! the drained events into a [`RunState`], and edits the one card in place through
//! the lifecycle — sending a push at run start and at the final outcome. All HTTP
//! goes through the injectable [`BotClient`]/[`Transport`] of `client.rs`, so every
//! mechanical claim here is unit-testable behind a fake transport; only the live
//! network round-trip is review-only.
//!
//! The Layer never blocks the logging thread on the network: it only enqueues. The
//! worker swallows per-call transport errors (a stalled network must never abort or
//! block the run), and the queue drops the oldest event under back-pressure.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;
use tracing::field::{Field, Visit};
use tracing::{warn, Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use super::client::{BotClient, Transport};
use crate::runstate::{IssueEntry, IssueStatus, RunEvent, RunState};

/// Telegram's hard per-message character limit.
const TELEGRAM_LIMIT: usize = 4096;

/// Above this many issues the card collapses to counters + active + last-finished
/// rather than one line per issue (ADR-0007 D6).
const FULL_LIST_MAX: usize = 30;

/// The bounded ring's capacity (ADR-0007 D4/D7).
const QUEUE_CAPACITY: usize = 256;

/// How often the worker re-polls the queue (so it notices shutdown promptly even
/// if a notify is lost), distinct from the card refresh cadence below.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The throttled card-refresh cadence during long silent phases (ADR-0007 D4).
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// How long [`NotifierHandle::shutdown`] waits for the worker before detaching, so
/// a wedged network never holds the process open (ADR-0007 D4).
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Rendering (pure over &RunState)
// ---------------------------------------------------------------------------

/// The status emoji for an issue (ADR-0007 D3 icon table).
fn status_emoji(status: &IssueStatus) -> &'static str {
    match status {
        IssueStatus::Planning => "🧠",
        IssueStatus::Executing { .. } => "⚙️",
        IssueStatus::Done => "✅",
        IssueStatus::Skipped => "⏭️",
        IssueStatus::Blocked => "⛔",
        IssueStatus::Infeasible => "🤷",
        IssueStatus::NonGreen => "❌",
    }
}

/// `MM:SS` clock form (minutes may exceed 59), e.g. `45:00`.
fn fmt_clock(total_secs: u64) -> String {
    format!("{:02}:{:02}", total_secs / 60, total_secs % 60)
}

/// One rendered issue line: `⚙️ #5 title · 45:00` while executing (the budget in
/// the `12:43 / 45:00` form's terminal half), `emoji #n title` otherwise.
fn issue_line(entry: &IssueEntry) -> String {
    let emoji = status_emoji(&entry.status);
    match &entry.status {
        IssueStatus::Executing { budget_min } => format!(
            "{emoji} #{} {} · {}",
            entry.number,
            entry.title,
            fmt_clock(budget_min * 60)
        ),
        _ => format!("{emoji} #{} {}", entry.number, entry.title),
    }
}

/// The card's counter line, e.g. `4 issues · ✅ 2 · ⏭️ 1 · ⛔ 0 · 🤷 0 · ❌ 0`.
fn counters_line(state: &RunState) -> String {
    let c = state.counts();
    format!(
        "{} issues · ✅ {} · ⏭️ {} · ⛔ {} · 🤷 {} · ❌ {}",
        state.total, c.done, c.skipped, c.blocked, c.infeasible, c.non_green
    )
}

/// Render the live card from a [`RunState`], guaranteed within Telegram's
/// 4096-char limit. A small queue renders one line per issue; a large one (over
/// [`FULL_LIST_MAX`]) collapses to the counters plus the active issue and the
/// most-recently-finished one (ADR-0007 D6).
pub fn render_card(state: &RunState) -> String {
    let mut out = String::new();
    out.push_str(&state.title);
    out.push('\n');
    out.push_str(&counters_line(state));
    out.push('\n');

    if state.issues.len() <= FULL_LIST_MAX {
        for entry in &state.issues {
            out.push_str(&issue_line(entry));
            out.push('\n');
        }
    } else {
        if let Some(active) = state.active_issue() {
            out.push_str("active · ");
            out.push_str(&issue_line(active));
            out.push('\n');
        }
        if let Some(last) = state.most_recent_finished() {
            out.push_str("last  · ");
            out.push_str(&issue_line(last));
            out.push('\n');
        }
    }

    if let Some(summary) = &state.final_summary {
        out.push_str(summary);
        out.push('\n');
    }

    truncate_chars(out, TELEGRAM_LIMIT)
}

/// The push sent at run start (a new message, so the phone buzzes — an edit does
/// not, ADR-0007 D3).
pub fn render_start_push(state: &RunState) -> String {
    format!("▶️ {} — {} issues queued", state.title, state.total)
}

/// The push sent at the final outcome.
pub fn render_final_push(state: &RunState) -> String {
    let c = state.counts();
    let head = state
        .final_summary
        .clone()
        .unwrap_or_else(|| "run finished".to_string());
    format!(
        "🏁 {} — {} · ✅ {} done, ⏭️ {} skipped",
        state.title, head, c.done, c.skipped
    )
}

/// Truncate `s` to at most `max` characters on a char boundary.
fn truncate_chars(mut s: String, max: usize) -> String {
    if s.chars().count() <= max {
        return s;
    }
    let idx = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
    s.truncate(idx);
    s
}

/// Derive the card title (ADR-0007 D3): `--title` wins; else the single issue's
/// title with `--only-issue`; else `<repo> · N issues [labels]`.
pub fn derive_title(
    repo_name: &str,
    issue_count: usize,
    labels: &[String],
    only_issue_title: Option<&str>,
    title_override: Option<&str>,
) -> String {
    if let Some(t) = title_override {
        if !t.trim().is_empty() {
            return t.to_string();
        }
    }
    if let Some(t) = only_issue_title {
        return t.to_string();
    }
    let label_part = if labels.is_empty() {
        String::new()
    } else {
        format!(" [{}]", labels.join(", "))
    };
    format!("{repo_name} · {issue_count} issues{label_part}")
}

// ---------------------------------------------------------------------------
// Bounded, drop-oldest event ring (ADR-0007 D4/D7)
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

    /// A queue with an explicit capacity (used by tests).
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
// The tracing Layer + the pure event mapping
// ---------------------------------------------------------------------------

/// The typed fields extracted off one `tracing` event, populated by the [`Visit`]
/// impl below and consumed by the pure [`event_to_runevent`].
#[derive(Debug, Default)]
pub struct NotifierFields {
    pub message: String,
    pub number: Option<u64>,
    pub title: Option<String>,
    pub open_steps: Option<u64>,
    pub count: Option<u64>,
    pub budget_min: Option<u64>,
    pub order: Option<String>,
    pub outcome: Option<String>,
}

impl Visit for NotifierFields {
    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            "number" => self.number = Some(value),
            "open_steps" => self.open_steps = Some(value),
            "count" => self.count = Some(value),
            "budget_min" => self.budget_min = Some(value),
            _ => {}
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.message = value.to_string(),
            "title" => self.title = Some(value.to_string()),
            "order" => self.order = Some(value.to_string()),
            "outcome" => self.outcome = Some(value.to_string()),
            _ => {}
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // `%field` (Display), `?field` (Debug), and the message literal all arrive
        // here as `format_args!` debug values.
        let rendered = format!("{value:?}");
        match field.name() {
            "message" => self.message = rendered,
            "title" => self.title = Some(rendered),
            "order" => self.order = Some(rendered),
            "outcome" => self.outcome = Some(rendered),
            _ => {}
        }
    }
}

/// Map an event's `(target, message, fields)` to a [`RunEvent`], or `None` for an
/// event the notifier ignores. Pure over its inputs and unit-tested per consumed
/// event so an event/model drift fails a test (ADR-0007 D6).
///
/// `target` is currently informational — the message + fields uniquely identify
/// every consumed event — but kept in the signature for a future disambiguation.
pub fn event_to_runevent(target: &str, message: &str, fields: &NotifierFields) -> Option<RunEvent> {
    let _ = target;
    let number = fields.number.unwrap_or(0);
    match message {
        "queue built" => Some(RunEvent::QueueBuilt {
            count: fields.count.unwrap_or(0),
            order: parse_order(fields.order.as_deref()),
        }),
        "issue started" => Some(RunEvent::IssueStarted {
            number,
            title: fields.title.clone().unwrap_or_default(),
        }),
        "plan written" => Some(RunEvent::PlanWritten {
            number,
            open_steps: fields.open_steps.unwrap_or(0),
        }),
        // The adapter's execution events carry no issue number; the fold applies
        // this to the active issue.
        "executing with interactive claude over the PTY"
        | "executing with headless claude -p loop" => Some(RunEvent::Executing {
            number,
            budget_min: fields.budget_min.unwrap_or(0),
        }),
        "green — issue closed" => Some(RunEvent::IssueClosed { number }),
        "non-green — stopping run" => Some(RunEvent::NonGreen {
            number,
            outcome: fields.outcome.clone().unwrap_or_default(),
        }),
        // Both a blocked-by skip and a stop-before halt render as ⏭️ skipped.
        "blocked by open issue(s) — skipping"
        | "stop-before label — halting run before this issue" => {
            Some(RunEvent::Skipped { number })
        }
        "deadline passed — not starting issue" => Some(RunEvent::DeadlinePassed { number }),
        _ => None,
    }
}

/// Parse the `queue built` `order` field (`#30 -> #31 -> #32`) into issue numbers.
fn parse_order(order: Option<&str>) -> Vec<u64> {
    let Some(s) = order else {
        return Vec::new();
    };
    s.split("->")
        .filter_map(|tok| {
            tok.trim()
                .trim_start_matches('#')
                .trim()
                .parse::<u64>()
                .ok()
        })
        .collect()
}

/// The substring identifying the notifier's own `tracing` target, so the worker's
/// runtime `warn!`s never feed back into the Layer and loop (ADR-0007 decision).
const SELF_TARGET_MARKER: &str = "telegram::notifier";

/// A `tracing` Layer that enqueues each consumed event as a [`RunEvent`]. It does
/// no I/O on the logging thread — only `event_to_runevent` + a ring push.
pub struct NotifierLayer {
    queue: Arc<EventQueue>,
}

impl NotifierLayer {
    /// Wrap the shared event queue the worker drains.
    pub fn new(queue: Arc<EventQueue>) -> Self {
        NotifierLayer { queue }
    }
}

impl<S: Subscriber> Layer<S> for NotifierLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let target = event.metadata().target();
        // Ignore the notifier's own events so a runtime warn! cannot loop back in.
        if target.contains(SELF_TARGET_MARKER) {
            return;
        }
        let mut fields = NotifierFields::default();
        event.record(&mut fields);
        if let Some(run_event) = event_to_runevent(target, &fields.message, &fields) {
            self.queue.push(run_event);
        }
    }
}

// ---------------------------------------------------------------------------
// The worker
// ---------------------------------------------------------------------------

/// The background worker (ADR-0007 D4): send the card + start push, fold each
/// drained event, edit the one owned `message_id` on change (with a throttled
/// ~60s refresh), and on shutdown render the terminal card and send the final
/// push. Every per-call transport error is swallowed (`warn!`ed) so a stalled
/// network never aborts or blocks the run.
pub fn run_worker<T: Transport>(
    client: BotClient<T>,
    chat_id: i64,
    mut state: RunState,
    queue: Arc<EventQueue>,
    shutdown: Arc<AtomicBool>,
) {
    // Initial card: capture its message_id so every later edit targets it.
    let message_id = match client.send_message(chat_id, &render_card(&state)) {
        Ok(v) => v.get("message_id").and_then(Value::as_i64),
        Err(e) => {
            warn!("telegram: initial card failed: {e}");
            None
        }
    };
    // The start push is a new message so the phone buzzes once at run start.
    if let Err(e) = client.send_message(chat_id, &render_start_push(&state)) {
        warn!("telegram: start push failed: {e}");
    }

    let mut last_edit = Instant::now();
    loop {
        if shutdown.load(Ordering::SeqCst) {
            // Drain anything still pending before the terminal render.
            for event in queue.drain_blocking(Duration::from_millis(0)) {
                state.apply(event);
            }
            break;
        }
        let events = queue.drain_blocking(POLL_INTERVAL);
        let changed = !events.is_empty();
        for event in events {
            state.apply(event);
        }
        if let Some(mid) = message_id {
            if changed || last_edit.elapsed() >= REFRESH_INTERVAL {
                if let Err(e) = client.edit_message_text(chat_id, mid, &render_card(&state)) {
                    warn!("telegram: edit failed: {e}");
                }
                last_edit = Instant::now();
            }
        }
    }

    // Terminal state: a final edit to the card, then the final push.
    if let Some(mid) = message_id {
        if let Err(e) = client.edit_message_text(chat_id, mid, &render_card(&state)) {
            warn!("telegram: final edit failed: {e}");
        }
    }
    if let Err(e) = client.send_message(chat_id, &render_final_push(&state)) {
        warn!("telegram: final push failed: {e}");
    }
}

// ---------------------------------------------------------------------------
// Activation, guards, and the handle
// ---------------------------------------------------------------------------

/// Whether a run should notify: only when configured AND not `--no-telegram` AND
/// not `--dry-run` (ADR-0007 D1/D7).
pub fn should_notify(configured: bool, no_telegram: bool, dry_run: bool) -> bool {
    configured && !no_telegram && !dry_run
}

/// Confirm the bot with `getMe` and, on success, spawn the worker; on failure emit
/// a single `warn!` and return `None` (the run proceeds without notifications —
/// ADR-0007 D7). The returned [`NotifierHandle`] holds the shutdown signal and the
/// worker's join handle.
pub fn try_start_notifier<T: Transport + Send + 'static>(
    client: BotClient<T>,
    chat_id: i64,
    state: RunState,
    queue: Arc<EventQueue>,
) -> Option<NotifierHandle> {
    if let Err(e) = client.get_me() {
        warn!("Telegram on but getMe failed — continuing without notifications: {e}");
        return None;
    }
    let shutdown = Arc::new(AtomicBool::new(false));
    let worker_queue = queue.clone();
    let worker_shutdown = shutdown.clone();
    let join = std::thread::Builder::new()
        .name("ralphy-telegram".into())
        .spawn(move || run_worker(client, chat_id, state, worker_queue, worker_shutdown))
        .ok()?;
    Some(NotifierHandle {
        queue,
        shutdown,
        join: Some(join),
    })
}

/// A handle to the running notifier: the shared event queue, its shutdown flag,
/// and the worker thread.
pub struct NotifierHandle {
    queue: Arc<EventQueue>,
    shutdown: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl NotifierHandle {
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
                warn!("telegram: notifier worker did not finish in time — detaching");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{bail, Result};
    use serde_json::json;
    use std::sync::atomic::AtomicI64;

    /// A recording transport: records every call and returns a fresh `message_id`
    /// for each `sendMessage`. Cloning shares the call log and id counter so a test
    /// can inspect what the worker did after the thread joins.
    #[derive(Clone)]
    struct RecordingTransport {
        calls: Arc<Mutex<Vec<(String, Value)>>>,
        next_id: Arc<AtomicI64>,
        fail_edit: bool,
    }

    impl RecordingTransport {
        fn new() -> Self {
            RecordingTransport {
                calls: Arc::new(Mutex::new(Vec::new())),
                next_id: Arc::new(AtomicI64::new(100)),
                fail_edit: false,
            }
        }
    }

    impl Transport for RecordingTransport {
        fn get(&self, method: &str) -> Result<Value> {
            self.calls
                .lock()
                .unwrap()
                .push((method.to_string(), Value::Null));
            Ok(json!({ "ok": true, "result": { "username": "ralphy_bot" } }))
        }

        fn post(&self, method: &str, body: Value) -> Result<Value> {
            self.calls.lock().unwrap().push((method.to_string(), body));
            match method {
                "sendMessage" => {
                    let id = self.next_id.fetch_add(1, Ordering::SeqCst);
                    Ok(json!({ "ok": true, "result": { "message_id": id } }))
                }
                "editMessageText" if self.fail_edit => bail!("edit boom"),
                _ => Ok(json!({ "ok": true, "result": {} })),
            }
        }
    }

    fn methods(calls: &[(String, Value)]) -> Vec<&str> {
        calls.iter().map(|(m, _)| m.as_str()).collect()
    }

    #[test]
    fn render_card_small_queue_one_line_per_issue() {
        let mut state = RunState::new("Repo · 2 issues", 2);
        state.apply(RunEvent::IssueStarted {
            number: 1,
            title: "first".into(),
        });
        state.apply(RunEvent::IssueClosed { number: 1 });
        state.apply(RunEvent::IssueStarted {
            number: 2,
            title: "second".into(),
        });
        let card = render_card(&state);
        assert!(card.contains("✅ #1 first"), "card: {card}");
        assert!(card.contains("🧠 #2 second"), "card: {card}");
        assert!(card.len() <= TELEGRAM_LIMIT);
    }

    #[test]
    fn render_card_collapses_large_queue_within_limit() {
        let mut state = RunState::new("Big run", 200);
        for n in 1..=200u64 {
            state.apply(RunEvent::IssueStarted {
                number: n,
                title: format!("issue {n} with a moderately long descriptive title to pad bytes"),
            });
            if n < 200 {
                state.apply(RunEvent::IssueClosed { number: n });
            }
        }
        let card = render_card(&state);
        assert!(card.len() <= TELEGRAM_LIMIT, "len {}", card.len());
        assert!(card.contains("200 issues"), "card: {card}");
        // Collapsed: active issue #200 and a last-finished line are shown.
        assert!(card.contains("#200"), "card: {card}");
    }

    #[test]
    fn derive_title_covers_all_three_branches() {
        // --title wins.
        assert_eq!(
            derive_title("repo", 3, &["AFK".into()], None, Some("Override")),
            "Override"
        );
        // --only-issue: the single title.
        assert_eq!(
            derive_title("repo", 1, &[], Some("Only one"), None),
            "Only one"
        );
        // Auto-derived with labels.
        assert_eq!(
            derive_title("myrepo", 3, &["AFK".into(), "ready".into()], None, None),
            "myrepo · 3 issues [AFK, ready]"
        );
        // A blank --title falls through to the auto form.
        assert_eq!(
            derive_title("myrepo", 1, &[], None, Some("  ")),
            "myrepo · 1 issues"
        );
    }

    #[test]
    fn event_queue_drops_oldest_at_capacity() {
        let q = EventQueue::with_capacity(2);
        q.push(RunEvent::IssueClosed { number: 1 });
        q.push(RunEvent::IssueClosed { number: 2 });
        q.push(RunEvent::IssueClosed { number: 3 });
        let drained = q.drain_blocking(Duration::from_millis(0));
        assert_eq!(
            drained,
            vec![
                RunEvent::IssueClosed { number: 2 },
                RunEvent::IssueClosed { number: 3 },
            ]
        );
    }

    #[test]
    fn event_to_runevent_maps_each_consumed_shape() {
        let f = |fields: NotifierFields| {
            event_to_runevent("ralphy_core::runner", &fields.message.clone(), &fields)
        };
        assert_eq!(
            f(NotifierFields {
                message: "queue built".into(),
                count: Some(3),
                order: Some("#1 -> #2 -> #3".into()),
                ..Default::default()
            }),
            Some(RunEvent::QueueBuilt {
                count: 3,
                order: vec![1, 2, 3]
            })
        );
        assert_eq!(
            f(NotifierFields {
                message: "issue started".into(),
                number: Some(7),
                title: Some("hello".into()),
                ..Default::default()
            }),
            Some(RunEvent::IssueStarted {
                number: 7,
                title: "hello".into()
            })
        );
        assert_eq!(
            f(NotifierFields {
                message: "plan written".into(),
                number: Some(7),
                open_steps: Some(0),
                ..Default::default()
            }),
            Some(RunEvent::PlanWritten {
                number: 7,
                open_steps: 0
            })
        );
        assert_eq!(
            f(NotifierFields {
                message: "executing with interactive claude over the PTY".into(),
                budget_min: Some(45),
                ..Default::default()
            }),
            Some(RunEvent::Executing {
                number: 0,
                budget_min: 45
            })
        );
        assert_eq!(
            f(NotifierFields {
                message: "green — issue closed".into(),
                number: Some(7),
                ..Default::default()
            }),
            Some(RunEvent::IssueClosed { number: 7 })
        );
        assert_eq!(
            f(NotifierFields {
                message: "non-green — stopping run".into(),
                number: Some(7),
                outcome: Some("Stuck".into()),
                ..Default::default()
            }),
            Some(RunEvent::NonGreen {
                number: 7,
                outcome: "Stuck".into()
            })
        );
        assert_eq!(
            f(NotifierFields {
                message: "blocked by open issue(s) — skipping".into(),
                number: Some(7),
                ..Default::default()
            }),
            Some(RunEvent::Skipped { number: 7 })
        );
        assert_eq!(
            f(NotifierFields {
                message: "deadline passed — not starting issue".into(),
                number: Some(7),
                ..Default::default()
            }),
            Some(RunEvent::DeadlinePassed { number: 7 })
        );
        // An unrelated event is ignored.
        assert_eq!(
            f(NotifierFields {
                message: "some other log".into(),
                ..Default::default()
            }),
            None
        );
    }

    #[test]
    fn should_notify_truth_table() {
        assert!(should_notify(true, false, false));
        assert!(!should_notify(false, false, false));
        assert!(!should_notify(true, true, false));
        assert!(!should_notify(true, false, true));
    }

    #[test]
    fn worker_sends_card_start_edits_and_final_push() {
        let transport = RecordingTransport::new();
        let calls = transport.calls.clone();
        let client = BotClient::new(transport);
        let queue = Arc::new(EventQueue::new());
        let shutdown = Arc::new(AtomicBool::new(false));

        queue.push(RunEvent::IssueStarted {
            number: 1,
            title: "a".into(),
        });
        queue.push(RunEvent::Executing {
            number: 1,
            budget_min: 45,
        });
        queue.push(RunEvent::IssueClosed { number: 1 });

        let worker_queue = queue.clone();
        let worker_shutdown = shutdown.clone();
        let state = RunState::new("title", 1);
        let handle =
            std::thread::spawn(move || run_worker(client, 7, state, worker_queue, worker_shutdown));

        shutdown.store(true, Ordering::SeqCst);
        queue.wake();
        handle.join().unwrap();

        let calls = calls.lock().unwrap();
        let m = methods(&calls);
        // card, then start push, then at least one edit, then the final push.
        assert_eq!(m.first(), Some(&"sendMessage"));
        assert_eq!(m[1], "sendMessage");
        assert!(m.contains(&"editMessageText"));
        assert_eq!(m.last(), Some(&"sendMessage"));

        // Every edit targets the card's message_id (the first sendMessage's id).
        let edit_ids: Vec<i64> = calls
            .iter()
            .filter(|(method, _)| method == "editMessageText")
            .map(|(_, body)| body["message_id"].as_i64().unwrap())
            .collect();
        assert!(!edit_ids.is_empty());
        assert!(edit_ids.iter().all(|&id| id == 100));
    }

    #[test]
    fn worker_swallows_edit_error_and_still_sends_final_push() {
        let mut transport = RecordingTransport::new();
        transport.fail_edit = true;
        let calls = transport.calls.clone();
        let client = BotClient::new(transport);
        let queue = Arc::new(EventQueue::new());
        let shutdown = Arc::new(AtomicBool::new(true)); // run inline: drain then finish.

        queue.push(RunEvent::IssueStarted {
            number: 1,
            title: "a".into(),
        });
        queue.push(RunEvent::NonGreen {
            number: 1,
            outcome: "Stuck".into(),
        });

        run_worker(client, 7, RunState::new("t", 1), queue.clone(), shutdown);

        let calls = calls.lock().unwrap();
        let m = methods(&calls);
        // The failing edit did not abort the worker: the final push still went out.
        assert!(m.contains(&"editMessageText"));
        assert_eq!(m.last(), Some(&"sendMessage"));
    }

    #[test]
    fn try_start_notifier_returns_none_on_get_me_error() {
        struct ErrTransport;
        impl Transport for ErrTransport {
            fn get(&self, _method: &str) -> Result<Value> {
                Ok(json!({ "ok": false, "description": "Unauthorized" }))
            }
            fn post(&self, _method: &str, _body: Value) -> Result<Value> {
                Ok(json!({ "ok": true, "result": {} }))
            }
        }
        let client = BotClient::new(ErrTransport);
        let queue = Arc::new(EventQueue::new());
        let handle = try_start_notifier(client, 1, RunState::new("t", 0), queue);
        assert!(handle.is_none());
    }

    #[test]
    fn shutdown_returns_promptly_when_transport_blocks() {
        struct BlockingTransport;
        impl Transport for BlockingTransport {
            fn get(&self, _method: &str) -> Result<Value> {
                Ok(json!({ "ok": true, "result": {} }))
            }
            fn post(&self, _method: &str, _body: Value) -> Result<Value> {
                // Wedge the worker in its first network call.
                std::thread::sleep(Duration::from_secs(30));
                Ok(json!({ "ok": true, "result": {} }))
            }
        }
        let client = BotClient::new(BlockingTransport);
        let queue = Arc::new(EventQueue::new());
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_queue = queue.clone();
        let worker_shutdown = shutdown.clone();
        let join = std::thread::spawn(move || {
            run_worker(
                client,
                1,
                RunState::new("t", 0),
                worker_queue,
                worker_shutdown,
            )
        });
        let handle = NotifierHandle {
            queue,
            shutdown,
            join: Some(join),
        };
        let start = Instant::now();
        handle.shutdown_within(Duration::from_millis(300));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "shutdown did not return promptly: {:?}",
            start.elapsed()
        );
    }
}
