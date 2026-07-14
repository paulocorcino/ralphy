//! The run-time Telegram notifier (ADR-0007 D1, D3, D4, D6, D7; ADR-0024).
//!
//! [`new_notifier_layer`] installs a [`DeliveryLayer`] that translates each `tracing`
//! event into a [`RunEvent`] and pushes it onto the shared bounded, drop-oldest
//! [`EventQueue`]. The notifier is a [`TelegramEngine`] fold over the shared
//! [`crate::delivery`] worker: it owns the card's `message_id`, folds the drained
//! events into a [`RunState`], and edits the one card in place through the lifecycle
//! — buzzing only on a usage-limit sleep/resume. All HTTP goes through the injectable
//! [`BotClient`]/[`Transport`] of `client.rs`, so every mechanical claim here is
//! unit-testable behind a fake transport; only the live network round-trip is
//! review-only.
//!
//! The Layer never blocks the logging thread on the network: it only enqueues. The
//! engine swallows per-call transport errors (a stalled network must never abort or
//! block the run), and the queue drops the oldest event under back-pressure.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tracing::warn;

use chrono::Local;

use super::client::{BotClient, Transport};
use crate::delivery::{spawn_worker, DeliveryEngine, DeliveryLayer, WorkerHandle};
pub use crate::delivery::{EventQueue, WorkerHandle as NotifierHandle};
use crate::runstate::{IssueEntry, IssueStatus, RunEvent, RunState, SkipKind, SleepState};

/// Telegram's hard per-message character limit.
const TELEGRAM_LIMIT: usize = 4096;

/// Above this many issues the card collapses to counters + active + last-finished
/// rather than one line per issue (ADR-0007 D6).
const FULL_LIST_MAX: usize = 30;

/// The throttled card-refresh cadence during long silent phases (ADR-0007 D4).
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// How long the `🔔` progress ping lives before it is deleted. An edit to the card
/// never raises a Telegram notification, so a genuine progress edit posts a brief
/// ping to buzz the phone, then removes it to keep the chat clean.
const PING_TTL: Duration = Duration::from_secs(2);

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
        IssueStatus::Hitl => "🙋",
    }
}

/// One rendered issue line: `emoji #n title`. The card carries no per-issue clock —
/// the budget is a static ceiling (e.g. `90:00`), not elapsed time, so showing it as
/// a clock only misleads. A dependency skip appends ` (blocked by #N)` when the
/// gating blocker(s) are known, so the operator sees which issue held it.
fn issue_line(entry: &IssueEntry) -> String {
    let emoji = status_emoji(&entry.status);
    let blocked_by = if entry.status == IssueStatus::Skipped
        && entry.kind == Some(SkipKind::BlockedBy)
        && !entry.blocked_by.is_empty()
    {
        let by = entry
            .blocked_by
            .iter()
            .map(|n| format!("#{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(" (blocked by {by})")
    } else {
        String::new()
    };
    format!("{emoji} #{} {}{blocked_by}", entry.number, entry.title)
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
    // A `🙋 N` counter appears only when a chain is parked on a human gate
    // (ADR-0014) — the common card stays unchanged, but a run waiting on a
    // person is visibly different.
    if c.hitl > 0 {
        line.push_str(&format!(" · 🙋 {}", c.hitl));
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
    // A run that reaches its terminal edge without a single issue finishing,
    // skipping, or parking never actually did any work — it was interrupted
    // (killed, superseded, or bailed at startup before the first `IssueStarted`).
    // The celebratory `🏁 … ✅ 0 done` footer misreads that as a clean completion:
    // an aborted run's card then sits above the next run's fresh card and reads
    // "finished → started" (FinCal, 2026-07-13). Render a distinct stopped footer so
    // a no-op run never masquerades as a completed one.
    let processed =
        c.done + c.skipped + c.blocked + c.infeasible + c.non_green + c.needs_split + c.hitl;
    if processed == 0 {
        return truncate_chars(
            format!(
                "🛑 {} — stopped before any issue was processed",
                state.title
            ),
            TELEGRAM_LIMIT,
        );
    }
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
    // A chain parked on a human gate (ADR-0014) is a pending human step the
    // footer must name, or a run that ends "green" hides it.
    let hitl_part = if c.hitl > 0 {
        format!(", 🙋 {} waiting on human", c.hitl)
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
            "🏁 {} — {} · ✅ {} done, ⏭️ {} skipped{hitl_part}{split_part}{knowledge_part}",
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

/// The push sent when the active child enters a sustained API-degraded state
/// (issue #149). A new message so the phone buzzes, mirroring the sleep push.
pub fn render_degraded_push(state: &RunState) -> String {
    truncate_chars(
        format!("⚠️ {} — API degraded, child retrying", state.title),
        TELEGRAM_LIMIT,
    )
}

/// The push sent when the API recovers, matching a prior degraded push.
pub fn render_recover_push(state: &RunState) -> String {
    truncate_chars(
        format!("✅ {} — API recovered, resuming", state.title),
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
// The tracing Layer
// ---------------------------------------------------------------------------

/// The substring identifying the notifier's own `tracing` target, so the worker's
/// runtime `warn!`s never feed back into the Layer and loop (ADR-0007 decision).
const SELF_TARGET_MARKER: &str = "telegram::notifier";

/// The notifier's `tracing` Layer: a [`DeliveryLayer`] over the shared ring, tagged
/// with the notifier's own target so a runtime `warn!` never feeds back into the ring.
pub fn new_notifier_layer(queue: Arc<EventQueue>) -> DeliveryLayer {
    DeliveryLayer::new(queue, SELF_TARGET_MARKER)
}

// ---------------------------------------------------------------------------
// The engine (ADR-0007 D4, ADR-0024)
// ---------------------------------------------------------------------------

/// The Telegram sink fold (ADR-0007 D4, ADR-0024): send the card once (`on_start`),
/// fold each drained event and buzz on a usage-limit sleep/resume edge (`on_event`),
/// edit the one owned `message_id` on change with a throttled ~60s refresh
/// (`on_tick`), and grow the terminal `🏁` footer on the way out (`on_finish`). The
/// card is a single consolidated component edited in place — no start/final pushes.
/// Entering a usage-limit sleep buzzes via a disposable notice; a re-parking limit
/// replaces it (delete + resend) so at most one is ever live, and resuming deletes
/// it, leaving the chat clean. Every per-call transport error is swallowed (`warn!`ed)
/// so a stalled network never aborts or blocks the run.
struct TelegramEngine<T: Transport> {
    client: BotClient<T>,
    chat_id: i64,
    state: RunState,
    /// The owned card's message id, captured from the initial `sendMessage`; every
    /// later edit targets it. `None` if the initial send failed.
    message_id: Option<i64>,
    /// The last card text actually pushed to Telegram. The idle ~60s refresh re-runs
    /// `should_edit` on the time floor even when nothing changed; editing with an
    /// identical body makes the Bot API reject it ("message is not modified"), so we
    /// compare against this and only edit when the render genuinely differs.
    last_card: String,
    last_edit: Instant,
    /// The one live usage-limit sleep notice, treated as a disposable slot: it is
    /// deleted before a fresh notice is posted (so a re-parking limit never piles
    /// up messages) and deleted outright on resume, leaving the chat clean. `None`
    /// when no notice is currently shown.
    sleep_notice_id: Option<i64>,
    /// A pending `🔔` progress ping and when it was sent. A card edit is silent, so
    /// a genuine change posts this ping to force a notification, then a later tick
    /// deletes it once [`PING_TTL`] has elapsed. `None` when none is outstanding;
    /// while `Some`, a burst of edits coalesces into the one buzz already sent.
    ping: Option<(i64, Instant)>,
    prev_sleeping: bool,
    /// Tracks the folded `degraded` flag so the API-degraded push fires on the
    /// false→true edge and the recover push on true→false — matched pairs, never
    /// a lone recover (issue #149).
    prev_degraded: bool,
    /// The run's single "network dropped" gate. A stalled network fails every
    /// call — the ~60s idle refresh alone would otherwise `warn!` once a minute
    /// forever — so the first failure warns concisely and every later one is
    /// silent, until a success clears the gate (a genuinely new drop warns again).
    /// Mirrors the CloudEvents sink's warn-once discipline (`events::sink`).
    net_warned: bool,
}

impl<T: Transport> DeliveryEngine for TelegramEngine<T> {
    fn on_start(&mut self) {
        // Initial card: capture its message_id so every later edit targets it, and
        // remember its rendered text so the idle refresh can skip a no-op edit.
        let initial_card = render_card(&self.state, now_epoch());
        self.message_id = self
            .gate(
                "initial card failed",
                self.client.send_message(self.chat_id, &initial_card),
            )
            .and_then(|v| v.get("message_id").and_then(Value::as_i64));
        self.last_card = initial_card;
        // No separate start/final pushes: the card is one consolidated component
        // edited in place. The sleep/resume pushes are kept — a usage-limit pause is
        // an exceptional event worth a buzz.
        self.last_edit = Instant::now();
        self.prev_sleeping = self.state.sleep.is_some();
        self.prev_degraded = self.state.degraded;
    }

    fn on_event(&mut self, event: RunEvent) {
        // Detect the sleep edge per applied event, not once per drained batch: a
        // `SleepStarted` immediately followed by a `SleepEnded` in the SAME drain
        // would net to `sleep = None` and silently swallow both pushes if compared
        // only batch-to-batch. Per-event a false→true edge buzzes on entering a
        // sleep and a true→false edge buzzes on resuming; comparing against the
        // folded state keeps it idempotent, so a drop under back-pressure cannot
        // double-fire.
        self.state.apply(event);
        let now_sleeping = self.state.sleep.is_some();
        if now_sleeping && !self.prev_sleeping {
            // Disposable notice: a usage limit that keeps re-parking (synthetic
            // reset, ADR-0030) would otherwise post a fresh buzz every cycle and
            // bury the chat. Delete the prior notice first so at most one is ever
            // live, then buzz with the current reset time.
            self.delete_sleep_notice();
            self.sleep_notice_id = self
                .gate(
                    "sleep push failed",
                    self.client
                        .send_message(self.chat_id, &render_sleep_push(&self.state)),
                )
                .and_then(|v| v.get("message_id").and_then(Value::as_i64));
        } else if !now_sleeping && self.prev_sleeping {
            // Resumed: the pause is over, so drop the disposable notice to keep the
            // chat organized. No resume push — the live card already reflects the
            // resume, and a lingering "resuming" line is exactly the clutter this
            // change removes.
            self.delete_sleep_notice();
        }
        self.prev_sleeping = now_sleeping;

        // The API-degraded edge, same matched-pair shape as the sleep edge: a
        // false→true buzzes on entering degraded, true→false on recovery. A lone
        // `ApiRecovered` (no prior degraded folded) is a no-op.
        let now_degraded = self.state.degraded;
        if now_degraded && !self.prev_degraded {
            self.gate(
                "degraded push failed",
                self.client
                    .send_message(self.chat_id, &render_degraded_push(&self.state)),
            );
        } else if !now_degraded && self.prev_degraded {
            self.gate(
                "recover push failed",
                self.client
                    .send_message(self.chat_id, &render_recover_push(&self.state)),
            );
        }
        self.prev_degraded = now_degraded;
    }

    fn on_tick(&mut self, changed: bool) {
        // Retire an expired progress ping first, every tick, independent of whether
        // an edit happens this pass.
        self.expire_ping();
        if let Some(mid) = self.message_id {
            if should_edit(changed, self.last_edit.elapsed(), REFRESH_INTERVAL) {
                let card = render_card(&self.state, now_epoch());
                // Skip the round-trip when the body is unchanged: Telegram rejects an
                // identical edit, which would otherwise warn! once per idle refresh.
                if card != self.last_card {
                    // Only record the card as shown on success, so a transient
                    // failure is retried on the next refresh rather than masked.
                    if self
                        .gate(
                            "edit failed",
                            self.client.edit_message_text(self.chat_id, mid, &card),
                        )
                        .is_some()
                    {
                        self.last_card = card;
                        // The edit is silent — buzz the phone on genuine progress.
                        // Suppressed while sleeping: the disposable sleep notice
                        // already buzzes, and the 60s countdown re-render must not
                        // ping every minute.
                        if self.state.sleep.is_none() {
                            self.fire_ping();
                        }
                    }
                }
                self.last_edit = Instant::now();
            }
        }
    }

    fn on_finish(&mut self) {
        // Terminal state: mark the run finished so the card grows its `🏁` footer,
        // then a final in-place edit. No final push — the footer lands silently on the
        // card. Skip the edit when the terminal render matches what's already shown,
        // for the same "message is not modified" reason as the idle refresh above.
        self.state.finished = true;
        // Drop any disposable sleep notice still up (e.g. the run ended while
        // parked) so it doesn't outlive the run.
        self.delete_sleep_notice();
        // Retire a still-live progress ping before the terminal edit, so the run
        // ends on the card edit (not a trailing deleteMessage) and no `🔔` lingers.
        self.delete_ping();
        if let Some(mid) = self.message_id {
            let card = render_card(&self.state, now_epoch());
            if card != self.last_card {
                self.gate(
                    "final edit failed",
                    self.client.edit_message_text(self.chat_id, mid, &card),
                );
            }
        }
    }
}

impl<T: Transport> TelegramEngine<T> {
    /// Fold a transport result into the run's single [`net_warned`](Self::net_warned)
    /// gate: on success clear the gate and hand back the value; on failure warn
    /// once with a compact one-line reason (never the raw multi-line anyhow chain)
    /// then stay silent until the next success. This is the ONE place a transport
    /// error becomes console noise, so a wedged network buzzes once, not per call.
    fn gate(&mut self, what: &str, result: anyhow::Result<Value>) -> Option<Value> {
        match result {
            Ok(v) => {
                self.net_warned = false;
                Some(v)
            }
            Err(e) => {
                if !self.net_warned {
                    warn!("telegram: {what} — {}", short_reason(&e));
                    self.net_warned = true;
                }
                None
            }
        }
    }

    /// Delete the current disposable sleep notice, if any, clearing the slot.
    /// Best-effort: a failed delete is gated (warn-once), never fatal — a stale
    /// notice is preferable to aborting the run.
    fn delete_sleep_notice(&mut self) {
        if let Some(mid) = self.sleep_notice_id.take() {
            self.gate(
                "sleep notice delete failed",
                self.client.delete_message(self.chat_id, mid),
            );
        }
    }

    /// Post the `🔔` progress ping, unless one is already outstanding — while a
    /// ping is live it has already buzzed, so a burst of edits coalesces into it.
    fn fire_ping(&mut self) {
        if self.ping.is_some() {
            return;
        }
        if let Some(v) = self.gate(
            "ping send failed",
            self.client.send_message(self.chat_id, "🔔"),
        ) {
            if let Some(id) = v.get("message_id").and_then(Value::as_i64) {
                self.ping = Some((id, Instant::now()));
            }
        }
    }

    /// Delete the pending progress ping once it has outlived [`PING_TTL`].
    fn expire_ping(&mut self) {
        if let Some((_, sent)) = self.ping {
            if sent.elapsed() >= PING_TTL {
                self.delete_ping();
            }
        }
    }

    /// Delete the pending progress ping now, regardless of age, clearing the slot.
    /// Best-effort: a failed delete is `warn!`ed, never fatal.
    fn delete_ping(&mut self) {
        if let Some((id, _)) = self.ping.take() {
            self.gate(
                "ping delete failed",
                self.client.delete_message(self.chat_id, id),
            );
        }
    }
}

/// The current wall-clock Unix-seconds anchor for the live card countdown.
fn now_epoch() -> i64 {
    Local::now().timestamp()
}

/// Collapse a transport error into ONE short console-friendly clause. The raw
/// `ureq`→anyhow chain repeats the same OS message three times over two lines
/// (`Dns Failed: … (os error 11001): … (os error 11001)`), which is exactly the
/// noise this run reported. A DNS/connect/timeout failure is the network being
/// down — say that in four words; anything else falls back to the first line of
/// the chain, never the whole multi-line blast.
fn short_reason(e: &anyhow::Error) -> String {
    let full = format!("{e:#}");
    let low = full.to_lowercase();
    if low.contains("dns failed") || low.contains("resolve dns") {
        return "network unreachable (DNS)".to_string();
    }
    if low.contains("os error 10060") || low.contains("timed out") || low.contains("timeout") {
        return "network unreachable (timeout)".to_string();
    }
    if low.contains("network error") || low.contains("connection") || low.contains("connect") {
        return "network unreachable".to_string();
    }
    // Not a recognised network drop: keep just the first line so a genuine API
    // rejection (bad token, chat gone) is still legible without the chain dump.
    full.lines()
        .next()
        .unwrap_or("send failed")
        .trim()
        .to_string()
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

/// The notifier's detach-warn hook (ADR-0024): emits the "worker did not finish"
/// `warn!` under the notifier's OWN `tracing` module target (default target =
/// `ralphy_cli::telegram::notifier`, which contains [`SELF_TARGET_MARKER`]) so
/// [`DeliveryLayer`]'s self-target filter drops it instead of folding it into a
/// `RunEvent::Notice` and looping it back into the ring.
fn detach_warn() {
    warn!("telegram: notifier worker did not finish in time — detaching");
}

/// Confirm the bot with `getMe` and, on success, spawn the worker; on failure emit
/// a single `warn!` and return `None` (the run proceeds without notifications —
/// ADR-0007 D7). The returned [`WorkerHandle`] holds the shutdown signal and the
/// worker's join handle.
pub fn try_start_notifier<T: Transport + Send + 'static>(
    client: BotClient<T>,
    chat_id: i64,
    state: RunState,
    queue: Arc<EventQueue>,
) -> Option<WorkerHandle> {
    if let Err(e) = client.get_me() {
        warn!("Telegram on but getMe failed — continuing without notifications: {e}");
        return None;
    }
    let engine = TelegramEngine {
        client,
        chat_id,
        state,
        message_id: None,
        last_card: String::new(),
        last_edit: Instant::now(),
        sleep_notice_id: None,
        ping: None,
        prev_sleeping: false,
        prev_degraded: false,
        net_warned: false,
    };
    spawn_worker("ralphy-telegram", engine, queue, detach_warn)
}

#[cfg(test)]
mod tests;
