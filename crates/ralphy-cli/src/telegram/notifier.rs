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
use tracing::{warn, Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use chrono::Local;

use super::client::{BotClient, Transport};
use crate::runstate::{
    event_to_runevent, EventFields, IssueEntry, IssueStatus, RunEvent, RunState, SleepState,
};

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
        IssueStatus::Executing => "⚙️",
        IssueStatus::Done => "✅",
        IssueStatus::Skipped => "⏭️",
        IssueStatus::Blocked => "⛔",
        IssueStatus::Infeasible => "🤷",
        IssueStatus::NeedsSplit => "🧩",
        IssueStatus::NonGreen => "❌",
    }
}

/// One rendered issue line: `emoji #n title`. The card carries no per-issue clock —
/// the budget is a static ceiling (e.g. `90:00`), not elapsed time, so showing it as
/// a clock only misleads.
fn issue_line(entry: &IssueEntry) -> String {
    let emoji = status_emoji(&entry.status);
    format!("{emoji} #{} {}", entry.number, entry.title)
}

/// The card's counter line, e.g. `▶️ 4 · ✅ 2 · ⏭️ 1 · ⛔ 0 · 🤷 0 · ❌ 0`. The
/// leading `▶️ N` is the queue total (ADR-0007 D3 consolidated card). A `🧩 N`
/// needs-split counter appears only when non-zero — the common card stays
/// unchanged, but a parked-on-split run is visibly different.
fn counters_line(state: &RunState) -> String {
    let c = state.counts();
    let mut line = format!(
        "▶️ {} · ✅ {} · ⏭️ {} · ⛔ {} · 🤷 {} · ❌ {}",
        state.total, c.done, c.skipped, c.blocked, c.infeasible, c.non_green
    );
    if c.needs_split > 0 {
        line.push_str(&format!(" · 🧩 {}", c.needs_split));
    }
    line
}

/// The card's branding header: `🦊 Ralphy - v0.1.0` — a stable per-run face (seeded
/// by the run title) plus the binary's own version. Shared with the console header.
fn header_line(state: &RunState) -> String {
    crate::runstate::ralphy_header(&state.title)
}

/// The issue list block: one line per issue for a small queue, or the collapsed
/// `active`/`last` pair above [`FULL_LIST_MAX`] (ADR-0007 D6). Lines are joined by a
/// single `\n`; the caller separates this block from its neighbours with a blank
/// line. Empty when no issue has entered the lifecycle yet.
fn render_issue_block(state: &RunState) -> String {
    let mut lines: Vec<String> = Vec::new();
    if state.issues.len() <= FULL_LIST_MAX {
        for entry in &state.issues {
            lines.push(issue_line(entry));
        }
    } else {
        if let Some(active) = state.active_issue() {
            lines.push(format!("active · {}", issue_line(active)));
        }
        if let Some(last) = state.most_recent_finished() {
            lines.push(format!("last  · {}", issue_line(last)));
        }
    }
    lines.join("\n")
}

/// The live sleep line (ADR-0007 D3): `🌙 waiting for reset ~HH:MM · resumes in
/// ~Xh Ym`. The `HH:MM` is the event's raw reset hint; the countdown is
/// `max(0, target_epoch - now_epoch)` so it degrades to `~0m` once the reset is
/// due rather than going negative.
fn render_sleep_line(sleep: &SleepState, now_epoch: i64) -> String {
    let remaining = (sleep.target_epoch - now_epoch).max(0);
    let total_min = remaining / 60;
    let (h, m) = (total_min / 60, total_min % 60);
    let countdown = if h > 0 {
        format!("~{h}h {m}m")
    } else {
        format!("~{m}m")
    };
    format!(
        "🌙 waiting for reset ~{} · resumes in {}",
        sleep.reset, countdown
    )
}

/// Render the live card from a [`RunState`], guaranteed within Telegram's
/// 4096-char limit. A small queue renders one line per issue; a large one (over
/// [`FULL_LIST_MAX`]) collapses to the counters plus the active issue and the
/// most-recently-finished one (ADR-0007 D6). `now_epoch` (Unix seconds) anchors
/// the live sleep countdown.
pub fn render_card(state: &RunState, now_epoch: i64) -> String {
    // The card is one consolidated component: four groups separated by a blank
    // line (ADR-0007 D3). Each group is a section string; only non-empty sections
    // join, so a not-yet-started run (no issues) never leaves a stray blank line.
    let mut sections: Vec<String> = Vec::new();

    // 1) Branding header: a stable per-run face + the binary version.
    sections.push(header_line(state));

    // 2) Title + counters — one group, no blank line between the two lines.
    sections.push(format!("{}\n{}", state.title, counters_line(state)));

    // 3) The live sleep block, its own group while waiting for a reset.
    if let Some(sleep) = &state.sleep {
        sections.push(render_sleep_line(sleep, now_epoch));
    }

    // 4) The issue list (collapsed above FULL_LIST_MAX).
    let issues = render_issue_block(state);
    if !issues.is_empty() {
        sections.push(issues);
    }

    // 4b) The live knowledge-consolidation line (end-of-run trigger), its own group
    // while the session runs. Hidden once `finished` so a failed session — which
    // never clears `consolidating` — leaves no stale line on the terminal card; a
    // successful one is summarised in the footer instead.
    if let Some(notes) = state.consolidating {
        if !state.finished {
            sections.push(format!(
                "📚 consolidating {notes} knowledge note(s) into KNOWLEDGE.md…"
            ));
        }
    }

    // 5) The terminal footer — only once the run has finished, so the issue list
    // is the last group through the live run.
    if state.finished {
        sections.push(render_final_push(state));
    }

    truncate_chars(sections.join("\n\n"), TELEGRAM_LIMIT)
}

/// The run's terminal footer, embedded as the last group of the consolidated card
/// (`🏁 <title> — <head> · ✅ N done, ⏭️ M skipped`). Bounded to the message limit so
/// an over-long `--title` cannot make Telegram reject the edit.
pub fn render_final_push(state: &RunState) -> String {
    let c = state.counts();
    let head = state
        .final_summary
        .clone()
        .unwrap_or_else(|| "run finished".to_string());
    // A bundle verdict parks the queue on a human split — the footer must say
    // so, or a run that ends "green" hides the pending human step.
    let split_part = if c.needs_split > 0 {
        format!(", 🧩 {} awaiting split", c.needs_split)
    } else {
        String::new()
    };
    // The end-of-run knowledge consolidation, when it ran: a `📚 N consolidated`
    // segment so the curation step is visible on the terminal card.
    let knowledge_part = match state.consolidated {
        Some(n) => format!(", 📚 {n} consolidated"),
        None => String::new(),
    };
    truncate_chars(
        format!(
            "🏁 {} — {} · ✅ {} done, ⏭️ {} skipped{split_part}{knowledge_part}",
            state.title, head, c.done, c.skipped
        ),
        TELEGRAM_LIMIT,
    )
}

/// The push sent on entering a usage-limit sleep (a new message so the phone
/// buzzes, ADR-0007 D3). Bounded like the other pushes.
pub fn render_sleep_push(state: &RunState) -> String {
    let reset = state.sleep.as_ref().map(|s| s.reset.as_str()).unwrap_or("");
    truncate_chars(
        format!(
            "🌙 {} — usage limit, waiting for reset ~{}",
            state.title, reset
        ),
        TELEGRAM_LIMIT,
    )
}

/// The push sent on resuming from a usage-limit sleep. Bounded like the others.
pub fn render_resume_push(state: &RunState) -> String {
    truncate_chars(
        format!("⏰ {} — reset reached, resuming", state.title),
        TELEGRAM_LIMIT,
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
// The tracing Layer
// ---------------------------------------------------------------------------

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
// The worker
// ---------------------------------------------------------------------------

/// The background worker (ADR-0007 D4): send the card once, fold each drained event,
/// edit the one owned `message_id` on change (with a throttled ~60s refresh), and on
/// shutdown render the terminal card with its footer. The card is a single
/// consolidated component edited in place — no start/final pushes — though a
/// usage-limit sleep/resume still buzzes via its own push. Every per-call transport
/// error is swallowed (`warn!`ed) so a stalled network never aborts or blocks the run.
pub fn run_worker<T: Transport>(
    client: BotClient<T>,
    chat_id: i64,
    mut state: RunState,
    queue: Arc<EventQueue>,
    shutdown: Arc<AtomicBool>,
) {
    // Initial card: capture its message_id so every later edit targets it, and
    // remember its rendered text so the idle refresh can skip a no-op edit.
    let initial_card = render_card(&state, now_epoch());
    let message_id = match client.send_message(chat_id, &initial_card) {
        Ok(v) => v.get("message_id").and_then(Value::as_i64),
        Err(e) => {
            warn!("telegram: initial card failed: {e}");
            None
        }
    };
    // The last card text actually pushed to Telegram. The idle ~60s refresh re-runs
    // `should_edit` on the time floor even when nothing changed; editing with an
    // identical body makes the Bot API reject it ("message is not modified"), so we
    // compare against this and only edit when the render genuinely differs.
    let mut last_card = initial_card;
    // No separate start/final pushes: the card is one consolidated component edited
    // in place (the operator opted to drop the buzzes). The initial `sendMessage`
    // above already surfaces the card once; every later change is a silent edit, and
    // the terminal `🏁` footer is the final edit below. The sleep/resume pushes are
    // kept — a usage-limit pause is an exceptional event worth a buzz.

    let mut last_edit = Instant::now();
    let mut prev_sleeping = state.sleep.is_some();
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
        // Detect the sleep edge per applied event, not once per drained batch: a
        // `SleepStarted` immediately followed by a `SleepEnded` in the SAME drain
        // would net to `sleep = None` and silently swallow both pushes if compared
        // only batch-to-batch. Per-event a false→true edge buzzes on entering a
        // sleep and a true→false edge buzzes on resuming; comparing against the
        // folded state keeps it idempotent, so a drop under back-pressure cannot
        // double-fire.
        for event in events {
            state.apply(event);
            let now_sleeping = state.sleep.is_some();
            if now_sleeping && !prev_sleeping {
                if let Err(e) = client.send_message(chat_id, &render_sleep_push(&state)) {
                    warn!("telegram: sleep push failed: {e}");
                }
            } else if !now_sleeping && prev_sleeping {
                if let Err(e) = client.send_message(chat_id, &render_resume_push(&state)) {
                    warn!("telegram: resume push failed: {e}");
                }
            }
            prev_sleeping = now_sleeping;
        }

        if let Some(mid) = message_id {
            if should_edit(changed, last_edit.elapsed(), REFRESH_INTERVAL) {
                let card = render_card(&state, now_epoch());
                // Skip the round-trip when the body is unchanged: Telegram rejects an
                // identical edit, which would otherwise warn! once per idle refresh.
                if card != last_card {
                    match client.edit_message_text(chat_id, mid, &card) {
                        // Only record the card as shown on success, so a transient
                        // failure is retried on the next refresh rather than masked.
                        Ok(_) => last_card = card,
                        Err(e) => warn!("telegram: edit failed: {e}"),
                    }
                }
                last_edit = Instant::now();
            }
        }

        if stopping {
            break;
        }
    }

    // Terminal state: mark the run finished so the card grows its `🏁` footer, then a
    // final in-place edit. No final push — the footer lands silently on the card.
    // Skip the edit when the terminal render matches what's already shown, for the
    // same "message is not modified" reason as the idle refresh above.
    state.finished = true;
    if let Some(mid) = message_id {
        let card = render_card(&state, now_epoch());
        if card != last_card {
            if let Err(e) = client.edit_message_text(chat_id, mid, &card) {
                warn!("telegram: final edit failed: {e}");
            }
        }
    }
}

/// The current wall-clock Unix-seconds anchor for the live card countdown.
fn now_epoch() -> i64 {
    Local::now().timestamp()
}

/// The worker's edit gate, factored out so the ~60s cadence is testable without
/// real-time sleeping (ADR-0007 D4): edit when something changed, or when the
/// idle refresh interval has elapsed since the last edit.
fn should_edit(changed: bool, since_last_edit: Duration, interval: Duration) -> bool {
    changed || since_last_edit >= interval
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

    /// Live, opt-in demo that the notifier updates ONE message in place: it sends a
    /// card then edits it repeatedly with visibly-changing content (issues
    /// advancing + a live clock), ~2s apart, so the operator watches it animate.
    /// Run with `cargo test -p ralphy-cli -- --ignored live_animate_card --nocapture`.
    #[test]
    #[ignore = "hits the live Telegram Bot API; needs `telegram setup` first"]
    fn live_animate_card() {
        use crate::telegram::client::UreqTransport;
        use crate::telegram::config::{effective_token, TelegramConfig};

        let Some(cfg) = TelegramConfig::load().expect("load config") else {
            eprintln!("SKIP: Telegram not configured — run `ralphy telegram setup`");
            return;
        };
        let Some(chat_id) = cfg.chat_id else {
            eprintln!("SKIP: no chat captured — run `ralphy telegram setup`");
            return;
        };
        let token = effective_token(Some(&cfg.token)).expect("a token");
        let client = BotClient::new(UreqTransport::new(token));

        // Three issues; we walk them through planning → executing → done so each
        // rendered card differs from the last (no "message is not modified").
        let total = 3u64;
        let mut state = RunState::new("🔬 ralphy live card", total as usize);
        let card0 = render_card(&state, now_epoch());
        let sent = client.send_message(chat_id, &card0).expect("send card");
        let mid = sent["message_id"].as_i64().expect("message_id");
        eprintln!("animating message_id={mid}");

        let mut last_card = card0;
        // A helper that edits only when the render actually changed — the same guard
        // the worker uses — and reports each attempt.
        let push = |client: &BotClient<UreqTransport>, state: &RunState, last: &mut String| {
            let card = render_card(state, now_epoch());
            if &card == last {
                eprintln!("  (unchanged — skipped, as the worker would)");
                return;
            }
            match client.edit_message_text(chat_id, mid, &card) {
                Ok(_) => {
                    *last = card;
                    eprintln!("  edit OK");
                }
                Err(e) => eprintln!("  edit FAILED: {e}"),
            }
            std::thread::sleep(Duration::from_secs(2));
        };

        for n in 1..=total {
            state.apply(RunEvent::IssueStarted {
                number: n,
                title: format!("W{} live step", n - 1),
            });
            push(&client, &state, &mut last_card);

            state.apply(RunEvent::Executing {
                number: n,
                budget_min: 45,
                model: String::new(),
            });
            push(&client, &state, &mut last_card);

            state.apply(RunEvent::IssueClosed {
                number: n,
                tokens: 0,
            });
            push(&client, &state, &mut last_card);
        }

        state.final_summary = Some("✅ live demo finished".into());
        state.finished = true;
        push(&client, &state, &mut last_card);
        eprintln!("done — final card left on the message");
    }

    /// Live, opt-in proof against the real Bot API that the no-op-edit fix holds:
    /// run with `cargo test -p ralphy-cli -- --ignored live_edit_dedup_against_real_telegram --nocapture`.
    /// It uses the operator's stored token + chat (auto-skips if unconfigured),
    /// sends a card, edits it with CHANGED text (must succeed), then edits with
    /// IDENTICAL text (the Bot API rejects this with "message is not modified" —
    /// the exact bug), and finally confirms `render_card` is byte-identical across
    /// two unchanged renders, so the worker's `card != last_card` guard skips it.
    #[test]
    #[ignore = "hits the live Telegram Bot API; needs `telegram setup` first"]
    fn live_edit_dedup_against_real_telegram() {
        use crate::telegram::client::UreqTransport;
        use crate::telegram::config::{effective_token, TelegramConfig};

        let Some(cfg) = TelegramConfig::load().expect("load config") else {
            eprintln!("SKIP: Telegram not configured — run `ralphy telegram setup`");
            return;
        };
        let Some(chat_id) = cfg.chat_id else {
            eprintln!("SKIP: no chat captured — run `ralphy telegram setup`");
            return;
        };
        let token = effective_token(Some(&cfg.token)).expect("a token");
        let client = BotClient::new(UreqTransport::new(token));

        // A run state matching the stuck-in-planning scenario from the bug report.
        let mut state = RunState::new("🔬 ralphy dedup self-test", 1);
        state.apply(RunEvent::IssueStarted {
            number: 1,
            title: "W0: planning (live notifier self-test)".into(),
        });

        // 1) Send the initial card and capture its message_id.
        let card_v1 = render_card(&state, now_epoch());
        let sent = client.send_message(chat_id, &card_v1).expect("send card");
        let mid = sent["message_id"].as_i64().expect("message_id");
        eprintln!("sent card message_id={mid}");

        // 2) A genuinely changed render must edit successfully.
        state.apply(RunEvent::Executing {
            number: 1,
            budget_min: 45,
            model: String::new(),
        });
        let card_v2 = render_card(&state, now_epoch());
        assert_ne!(card_v1, card_v2, "state change should alter the render");
        client
            .edit_message_text(chat_id, mid, &card_v2)
            .expect("changed edit must succeed");
        eprintln!("changed edit OK");

        // 3) Re-editing with the SAME body is exactly what Telegram rejects — this
        // documents the root cause the guard exists to avoid.
        let err = client
            .edit_message_text(chat_id, mid, &card_v2)
            .expect_err("identical edit must be rejected by Telegram");
        let msg = err.to_string();
        eprintln!("identical edit rejected as expected: {msg}");
        assert!(
            msg.contains("message is not modified"),
            "expected the not-modified rejection, got: {msg}"
        );

        // 4) The guard's premise: two unchanged renders are byte-identical, so
        // `card != last_card` is false and the worker never makes call (3).
        let card_again = render_card(&state, now_epoch());
        assert_eq!(
            card_v2, card_again,
            "unchanged state must render identically — the guard relies on this"
        );
        eprintln!("PASS: unchanged render is identical → idle refresh is skipped");
    }

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
        state.apply(RunEvent::IssueClosed {
            number: 1,
            tokens: 0,
        });
        state.apply(RunEvent::IssueStarted {
            number: 2,
            title: "second".into(),
        });
        let card = render_card(&state, 0);
        assert!(card.contains("✅ #1 first"), "card: {card}");
        assert!(card.contains("🧠 #2 second"), "card: {card}");
        assert!(card.len() <= TELEGRAM_LIMIT);
    }

    #[test]
    fn render_card_and_footer_surface_needs_split() {
        let mut state = RunState::new("repo · 1 issues", 1);
        state.apply(RunEvent::IssueStarted {
            number: 3,
            title: "W1 bundle".into(),
        });
        state.apply(RunEvent::PlanWritten {
            number: 3,
            open_steps: 0,
        });
        state.apply(RunEvent::NeedsSplit { number: 3 });
        let card = render_card(&state, 0);
        assert!(card.contains("🧩 #3 W1 bundle"), "issue line: {card}");
        assert!(card.contains("· 🧩 1"), "counter: {card}");
        state.finished = true;
        let footer = render_final_push(&state);
        assert!(footer.contains("🧩 1 awaiting split"), "footer: {footer}");
        // Without a bundle, neither the counter nor the footer mention it.
        let clean = RunState::new("repo · 1 issues", 1);
        assert!(!render_card(&clean, 0).contains("🧩"));
        assert!(!render_final_push(&clean).contains("🧩"));
    }

    #[test]
    fn render_card_has_header_counters_and_blank_line_grouping() {
        let mut state = RunState::new("ocs-inventory · 2 issues [AFK]", 2);
        state.apply(RunEvent::IssueStarted {
            number: 1,
            title: "first".into(),
        });
        let card = render_card(&state, 0);
        // Branding header with the binary version.
        assert!(card.contains("Ralphy - v"), "header missing: {card}");
        assert!(
            card.contains(env!("CARGO_PKG_VERSION")),
            "version missing: {card}"
        );
        // The counter line leads with `▶️ N`, the queue total (not `N issues`).
        assert!(card.contains("▶️ 2 · ✅ 0"), "counters: {card}");
        assert!(!card.contains("2 issues ·"), "old counter form: {card}");
        // Groups are separated by a blank line.
        assert!(card.contains("\n\n"), "blank-line grouping: {card}");
        // No footer mid-run — the issue list is the last group.
        assert!(!card.contains("🏁"), "footer must not show mid-run: {card}");
    }

    #[test]
    fn render_card_shows_live_consolidation_line_then_footer_segment() {
        let mut state = RunState::new("repo · 1 issues", 1);
        state.apply(RunEvent::IssueStarted {
            number: 1,
            title: "a".into(),
        });
        state.apply(RunEvent::IssueClosed {
            number: 1,
            tokens: 0,
        });
        // Mid-consolidation: the live 📚 line shows, no footer yet.
        state.apply(RunEvent::KnowledgeConsolidating { notes: 4 });
        let live = render_card(&state, 0);
        assert!(
            live.contains("📚 consolidating 4 knowledge note(s)"),
            "live consolidation line: {live}"
        );
        assert!(!live.contains("🏁"), "no footer mid-run: {live}");

        // Completion + terminal: the live line is gone, the footer carries the count.
        state.apply(RunEvent::KnowledgeConsolidated { archived: 4 });
        state.finished = true;
        let card = render_card(&state, 0);
        assert!(
            !card.contains("consolidating 4"),
            "live line hidden once finished: {card}"
        );
        assert!(card.contains("📚 4 consolidated"), "footer segment: {card}");
    }

    #[test]
    fn render_card_hides_stale_consolidating_line_on_finished_card() {
        // A failed session never emits `KnowledgeConsolidated`, so `consolidating`
        // stays set — the terminal card must still drop the stale 📚 line.
        let mut state = RunState::new("repo · 1 issues", 1);
        state.apply(RunEvent::KnowledgeConsolidating { notes: 2 });
        state.finished = true;
        let card = render_card(&state, 0);
        assert!(
            !card.contains("consolidating"),
            "no stale live line: {card}"
        );
        assert!(
            !card.contains("📚"),
            "no consolidated footer segment: {card}"
        );
    }

    #[test]
    fn render_card_shows_footer_only_when_finished() {
        let mut state = RunState::new("repo · 1 issues", 1);
        state.apply(RunEvent::IssueStarted {
            number: 1,
            title: "a".into(),
        });
        state.apply(RunEvent::IssueClosed {
            number: 1,
            tokens: 0,
        });
        // During the run: no footer.
        assert!(!render_card(&state, 0).contains("🏁"), "no footer mid-run");
        // Finished: the footer appears with the done/skipped tally.
        state.finished = true;
        let card = render_card(&state, 0);
        assert!(card.contains("🏁"), "footer missing when finished: {card}");
        assert!(card.contains("run finished"), "footer head: {card}");
        assert!(card.contains("✅ 1 done"), "footer tally: {card}");
    }

    #[test]
    fn header_face_is_stable_per_title_but_varies_across_titles() {
        // Same title → same face on every edit (so the card never re-edits just to
        // animate the face).
        assert_eq!(
            header_line(&RunState::new("ocs-inventory · 10 issues", 10)),
            header_line(&RunState::new("ocs-inventory · 10 issues", 10))
        );
        // The face is drawn from the curated pool.
        let face = crate::runstate::header_face("ocs-inventory · 10 issues");
        assert!(
            crate::runstate::HEADER_FACES.contains(&face),
            "face off-pool: {face}"
        );
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
                state.apply(RunEvent::IssueClosed {
                    number: n,
                    tokens: 0,
                });
            }
        }
        let card = render_card(&state, 0);
        assert!(card.len() <= TELEGRAM_LIMIT, "len {}", card.len());
        assert!(card.contains("▶️ 200"), "card: {card}");
        // Collapsed: active issue #200 and a last-finished line are shown.
        assert!(card.contains("#200"), "card: {card}");
    }

    #[test]
    fn render_card_shows_sleep_line_with_live_countdown() {
        use crate::runstate::SleepState;
        let mut state = RunState::new("Repo", 1);
        state.sleep = Some(SleepState {
            reset: "14:30".into(),
            // 2h13m ahead of `now`.
            target_epoch: 1_700_000_000 + 2 * 3600 + 13 * 60,
        });
        let card = render_card(&state, 1_700_000_000);
        assert!(card.contains('🌙'), "card: {card}");
        assert!(card.contains("14:30"), "card: {card}");
        assert!(card.contains("resumes in ~"), "card: {card}");
        assert!(card.contains("~2h 13m"), "card: {card}");
    }

    #[test]
    fn render_sleep_line_clamps_to_zero_when_reset_due() {
        use crate::runstate::SleepState;
        // `now` is past the target: the countdown degrades to `~0m`, not negative.
        let sleep = SleepState {
            reset: "09:00".into(),
            target_epoch: 1_700_000_000,
        };
        let line = render_sleep_line(&sleep, 1_700_000_500);
        assert!(line.contains("~0m"), "line: {line}");
        assert!(!line.contains('-'), "line should not go negative: {line}");
    }

    #[test]
    fn should_edit_respects_change_and_60s_floor() {
        let interval = Duration::from_secs(60);
        // A change always edits, regardless of elapsed time.
        assert!(should_edit(true, Duration::from_secs(0), interval));
        // Idle below the floor does not edit.
        assert!(!should_edit(false, Duration::from_secs(59), interval));
        // Idle at/after the floor edits.
        assert!(should_edit(false, Duration::from_secs(60), interval));
        assert!(should_edit(false, Duration::from_secs(120), interval));
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
        q.push(RunEvent::IssueClosed {
            number: 1,
            tokens: 0,
        });
        q.push(RunEvent::IssueClosed {
            number: 2,
            tokens: 0,
        });
        q.push(RunEvent::IssueClosed {
            number: 3,
            tokens: 0,
        });
        let drained = q.drain_blocking(Duration::from_millis(0));
        assert_eq!(
            drained,
            vec![
                RunEvent::IssueClosed {
                    number: 2,
                    tokens: 0
                },
                RunEvent::IssueClosed {
                    number: 3,
                    tokens: 0
                },
            ]
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
    fn worker_sends_one_card_then_edits_in_place_no_pushes() {
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
            model: String::new(),
        });
        queue.push(RunEvent::IssueClosed {
            number: 1,
            tokens: 0,
        });

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
        // Exactly ONE sendMessage — the card itself. No start/final pushes; every
        // later change is an in-place edit.
        let sends = m.iter().filter(|&&x| x == "sendMessage").count();
        assert_eq!(sends, 1, "only the card is sent, not pushed: {m:?}");
        assert_eq!(m.first(), Some(&"sendMessage"));
        assert!(m.contains(&"editMessageText"));
        // The run ends on an edit (the terminal footer), never a push.
        assert_eq!(m.last(), Some(&"editMessageText"));

        // Every edit targets the card's message_id (the first sendMessage's id).
        let edit_ids: Vec<i64> = calls
            .iter()
            .filter(|(method, _)| method == "editMessageText")
            .map(|(_, body)| body["message_id"].as_i64().unwrap())
            .collect();
        assert!(!edit_ids.is_empty());
        assert!(edit_ids.iter().all(|&id| id == 100));
    }

    /// Block (bounded) until `pred` holds over the recorded calls, so the sleep
    /// test waits for the worker to fold one event before enqueuing the next
    /// without a fixed sleep. Panics if it never holds (a real regression).
    fn wait_until(
        calls: &Arc<Mutex<Vec<(String, Value)>>>,
        pred: impl Fn(&[(String, Value)]) -> bool,
    ) {
        for _ in 0..200 {
            if pred(&calls.lock().unwrap()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("condition never held within timeout");
    }

    fn send_texts(calls: &[(String, Value)]) -> Vec<String> {
        calls
            .iter()
            .filter(|(m, _)| m == "sendMessage")
            .map(|(_, b)| b["text"].as_str().unwrap_or("").to_string())
            .collect()
    }

    #[test]
    fn worker_pushes_on_sleep_enter_and_resume() {
        let transport = RecordingTransport::new();
        let calls = transport.calls.clone();
        let client = BotClient::new(transport);
        let queue = Arc::new(EventQueue::new());
        let shutdown = Arc::new(AtomicBool::new(false));

        let worker_queue = queue.clone();
        let worker_shutdown = shutdown.clone();
        let state = RunState::new("title", 1);
        let handle =
            std::thread::spawn(move || run_worker(client, 7, state, worker_queue, worker_shutdown));

        // Enter a sleep, then wait for the worker to fold it and buzz the phone.
        queue.push(RunEvent::SleepStarted {
            reset: "14:30".into(),
            target_epoch: 1_700_000_000,
        });
        queue.wake();
        wait_until(&calls, |c| {
            send_texts(c).iter().any(|t| t.contains("usage limit"))
        });

        // Resume, then wait for the resume buzz.
        queue.push(RunEvent::SleepEnded);
        queue.wake();
        wait_until(&calls, |c| {
            send_texts(c).iter().any(|t| t.contains("resuming"))
        });

        shutdown.store(true, Ordering::SeqCst);
        queue.wake();
        handle.join().unwrap();

        let calls = calls.lock().unwrap();
        let texts = send_texts(&calls);
        let sleep_idx = texts
            .iter()
            .position(|t| t.contains("usage limit"))
            .expect("sleep push");
        let resume_idx = texts
            .iter()
            .position(|t| t.contains("resuming"))
            .expect("resume push");
        // Order: the sleep-in push fires before the resume push, both after the
        // initial card. There are no start/final pushes anymore.
        assert!(
            sleep_idx < resume_idx,
            "sleep push must precede resume push: {texts:?}"
        );
        // initial card + sleep + resume = three sendMessage calls (no start/final).
        assert_eq!(
            texts.len(),
            3,
            "expected exactly 3 sendMessage, got {texts:?}"
        );
    }

    #[test]
    fn worker_fires_both_pushes_when_sleep_events_co_batch() {
        // A SleepStarted immediately followed by a SleepEnded drained in ONE batch
        // nets to `sleep = None`; per-event edge detection must still fire both the
        // sleep-in and the resume push (a batch-to-batch compare would swallow them).
        let transport = RecordingTransport::new();
        let calls = transport.calls.clone();
        let client = BotClient::new(transport);
        let queue = Arc::new(EventQueue::new());
        // Inline run: shutdown already set, so the first drain takes both events.
        let shutdown = Arc::new(AtomicBool::new(true));

        queue.push(RunEvent::SleepStarted {
            reset: "14:30".into(),
            target_epoch: 1_700_000_000,
        });
        queue.push(RunEvent::SleepEnded);

        run_worker(client, 7, RunState::new("t", 1), queue.clone(), shutdown);

        let calls = calls.lock().unwrap();
        let texts = send_texts(&calls);
        let sleep_idx = texts
            .iter()
            .position(|t| t.contains("usage limit"))
            .expect("sleep push fired");
        let resume_idx = texts
            .iter()
            .position(|t| t.contains("resuming"))
            .expect("resume push fired");
        assert!(sleep_idx < resume_idx, "order: {texts:?}");
    }

    #[test]
    fn worker_swallows_edit_error_and_finishes_cleanly() {
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
        // The failing edit was swallowed, not fatal: the worker still attempted the
        // edit and returned. Only the card was sent (no pushes exist to fall back on).
        assert!(m.contains(&"editMessageText"));
        let sends = m.iter().filter(|&&x| x == "sendMessage").count();
        assert_eq!(sends, 1, "only the card is sent: {m:?}");
    }

    #[test]
    fn worker_terminal_edit_adds_footer_as_the_last_call() {
        // With no state-changing events the idle loop makes no edit (an identical
        // body would be rejected as "message is not modified"). The one terminal
        // edit is the `finished` flip growing the `🏁` footer — a genuine change —
        // and it is the LAST call: there is no final push after it.
        let transport = RecordingTransport::new();
        let calls = transport.calls.clone();
        let client = BotClient::new(transport);
        let queue = Arc::new(EventQueue::new());
        let shutdown = Arc::new(AtomicBool::new(true));

        run_worker(client, 7, RunState::new("idle", 1), queue, shutdown);

        let calls = calls.lock().unwrap();
        let m = methods(&calls);
        // Initial card (sent once), then exactly one terminal footer edit — last.
        assert_eq!(m.first(), Some(&"sendMessage"));
        assert_eq!(m.last(), Some(&"editMessageText"));
        let edits: Vec<&Value> = calls
            .iter()
            .filter(|(method, _)| method == "editMessageText")
            .map(|(_, body)| body)
            .collect();
        assert_eq!(edits.len(), 1, "exactly one terminal footer edit: {m:?}");
        let edited_text = edits[0]["text"].as_str().unwrap_or("");
        assert!(
            edited_text.contains("🏁") && edited_text.contains("run finished"),
            "terminal edit must carry the footer: {edited_text}"
        );
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
