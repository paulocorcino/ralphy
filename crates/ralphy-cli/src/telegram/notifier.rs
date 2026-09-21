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
use crate::runstate::{RunEvent, RunState};

mod render;

pub use render::{
    derive_title, render_card, render_degraded_push, render_idle_reaped_push, render_recover_push,
    render_sleep_push, render_waiting_push,
};

/// The throttled card-refresh cadence during long silent phases (ADR-0007 D4).
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// How long the `🔔` progress ping lives before it is deleted. An edit to the card
/// never raises a Telegram notification, so a genuine progress edit posts a brief
/// ping to buzz the phone, then removes it to keep the chat clean.
const PING_TTL: Duration = Duration::from_secs(2);

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
        // Captured before the fold: an idle reap also clears `degraded`, and the
        // edge below would otherwise read that clear as a recovery and push
        // "API recovered, resuming" about a child Ralphy just killed.
        let reaped = match event {
            RunEvent::IdleReaped { idle_minutes } => Some(idle_minutes),
            _ => None,
        };
        // A `waiting` agent is a push (ADR-0059 §2); `working`/`done` fold
        // into the card. Edge-triggered by the adapter already (it emits on a
        // change), so one event is one buzz.
        let waiting = match &event {
            RunEvent::AgentState { state, detail, .. } if state == "waiting" => {
                Some(detail.clone())
            }
            _ => None,
        };
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
        } else if !now_degraded && self.prev_degraded && reaped.is_none() {
            self.gate(
                "recover push failed",
                self.client
                    .send_message(self.chat_id, &render_recover_push(&self.state)),
            );
        }
        if let Some(detail) = waiting {
            self.gate(
                "waiting push failed",
                self.client.send_message(
                    self.chat_id,
                    &render_waiting_push(&self.state, detail.as_deref()),
                ),
            );
        }
        // The reap gets its own push either way: it is terminal news for this
        // issue, whether or not a degraded episode preceded it.
        if let Some(idle_minutes) = reaped {
            self.gate(
                "idle reap push failed",
                self.client.send_message(
                    self.chat_id,
                    &render_idle_reaped_push(&self.state, idle_minutes),
                ),
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
