//! The pure, transport-agnostic run model (ADR-0007 D6).
//!
//! A run's `tracing` event stream is folded into a [`RunState`] — the run title,
//! the issues and their per-issue [`IssueStatus`], the current/active issue, and
//! the terminal summary — by a **pure** function [`RunState::apply`]. The Telegram
//! worker renders a card from this model; the future ADR-0006 presenter can render
//! a terminal UI from the *same* model without depending on Telegram, which is why
//! this lives in its own module rather than inside `telegram`.
//!
//! The fold is unit-tested in isolation in the style of the adapters' `classify_*`
//! functions, so a drift between an event and the model that reads it fails a test
//! rather than silently breaking a display.

use tracing::field::{Field, Visit};
use tracing::Level;

/// The pool of branding header faces (human + animal). One is picked per run by a
/// hash of a stable seed (the run title), so the face is "random" across runs but
/// constant across every render of one run — an animated face would re-trigger
/// edits and trip Telegram's "message is not modified".
pub const HEADER_FACES: &[&str] = &[
    "🦊", "🐶", "🐱", "🦁", "🐯", "🐰", "🐻", "🐼", "🐨", "🐸", "🐵", "🦝", "🐺", "🦄", "🐷", "🐲",
    "🦉", "🦅", "🐢", "🐙", "🐳", "🐝", "🦋", "🐧", "🦦", "🦥", "🐹", "🐭", "🐮", "🐔",
];

/// Pick a stable header face for `seed` via a small FNV-1a hash, so the same seed
/// always maps to the same face — deterministic across runs and processes (unlike a
/// randomized hasher).
pub fn header_face(seed: &str) -> &'static str {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in seed.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    HEADER_FACES[(h as usize) % HEADER_FACES.len()]
}

/// The shared branding header used by both the console and the Telegram card:
/// `🦊 Ralphy - v0.1.0` — a stable per-run face (seeded by `seed`) plus the binary's
/// own version.
pub fn ralphy_header(seed: &str) -> String {
    format!(
        "{} Ralphy - v{}",
        header_face(seed),
        env!("CARGO_PKG_VERSION")
    )
}

/// Why an issue was skipped: a `blocked-by` dependency, a `stop-before` label, or
/// a verify gate that stayed red after the runner's repair attempts (ADR-0011).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipKind {
    BlockedBy,
    StopBefore,
    VerifyFailed,
}

/// A normalized token-usage breakdown carried on a [`RunEvent`] for the live UI:
/// the four numeric fields the compact meter renders (`↑ input · ⚡ cache-read ·
/// ❄ cache-write · ↓ output`) plus the `model` the read-time USD prices on (D8).
/// Mirrors `ralphy_core::Usage` but lives in the CLI so the decoder owns it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageLite {
    pub input: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
    pub output: u64,
    pub model: Option<String>,
}

impl UsageLite {
    /// The flat token total across the four numeric fields — drives the
    /// "omit the meter when zero" guard, mirroring `Usage::total`.
    pub fn total(&self) -> u64 {
        self.input + self.cache_read + self.cache_creation + self.output
    }
}

/// One semantic run event, already lifted out of the raw `(target, message,
/// fields)` triple by [`event_to_runevent`]. One variant per consumed lifecycle
/// event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEvent {
    /// The queue was built: its size and the issue numbers in order.
    QueueBuilt { count: u64, order: Vec<u64> },
    /// Work began on an issue (number + title).
    IssueStarted { number: u64, title: String },
    /// The planning phase started for the active issue (adapter event). Carries
    /// the planner's display model/effort so the live region can label the
    /// planning spinner and the `plan written` scroll line. Live-region only —
    /// no permanent line; the adapter never learns the issue number.
    Planning {
        model: Option<String>,
        effort: Option<String>,
    },
    /// A plan was written; `open_steps == 0` means the plan is infeasible. The
    /// `usage` is the planning phase's token consumption for the inline meter.
    PlanWritten {
        number: u64,
        open_steps: u64,
        usage: UsageLite,
    },
    /// Execution started for the active issue. The adapter never learns the issue
    /// number, so `number` is `0` here and resolves to the active issue.
    Executing {
        number: u64,
        budget_min: u64,
        model: String,
        effort: Option<String>,
    },
    /// A green issue was closed (the cycle). `tokens` is the issue's total (plan +
    /// execute) flat count, kept for the telegram notifier; `usage` is the
    /// *execution* phase's breakdown, which the live region combines with the
    /// planning usage it stashed at `PlanWritten` to show the issue total (D11).
    IssueClosed {
        number: u64,
        tokens: u64,
        usage: UsageLite,
    },
    /// An issue finished non-green and stopped the run; `outcome` is the core's
    /// `Outcome` debug string (e.g. `Stuck`, `Blocked`, `Timeout`).
    NonGreen { number: u64, outcome: String },
    /// An issue was skipped (blocked-by an open issue, a `stop-before` label, or a
    /// verify gate still red after the repair budget).
    Skipped { number: u64, kind: SkipKind },
    /// The planner judged the issue a bundle (several backlog tasks under one
    /// number): the queue is parked on a human split. Follows the infeasible
    /// `PlanWritten { open_steps: 0 }` and upgrades the status.
    NeedsSplit { number: u64 },
    /// A WARN or ERROR event from the run: level wins over message content.
    Notice { level: Level, message: String },
    /// The deadline passed before this issue could be started.
    DeadlinePassed { number: u64 },
    /// The run hit a usage limit and is sleeping until `reset`; `target_epoch` is
    /// the Unix-seconds wake anchor (the reset plus the wait-policy buffer) for a
    /// live countdown.
    SleepStarted { reset: String, target_epoch: i64 },
    /// The reset arrived and the run resumed; clears any active sleep.
    SleepEnded,
    /// The end-of-run knowledge consolidation started, folding `notes` loose
    /// per-issue notes into `KNOWLEDGE.md`.
    KnowledgeConsolidating { notes: u64 },
    /// Knowledge consolidation finished, archiving `archived` notes into
    /// `knowledge/raw/` after curating `KNOWLEDGE.md`.
    KnowledgeConsolidated { archived: u64 },
}

/// The per-issue status the card renders. Distinguishes ⏭️ skipped (a dependency
/// or `stop-before` skip) from 🤷 infeasible (an empty plan), 🧩 needs-split (a
/// bundle verdict awaiting a human split) and ⛔ blocked (a `Blocked` execution
/// outcome) from a generic non-green stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueStatus {
    Planning,
    Executing,
    Done,
    Skipped,
    Blocked,
    Infeasible,
    NeedsSplit,
    NonGreen,
}

impl IssueStatus {
    /// Whether this is a terminal status (the issue will not change further).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            IssueStatus::Done
                | IssueStatus::Skipped
                | IssueStatus::Blocked
                | IssueStatus::Infeasible
                | IssueStatus::NeedsSplit
                | IssueStatus::NonGreen
        )
    }
}

/// An active usage-limit sleep: the reset-time hint shown on the card and the
/// Unix-seconds wake anchor the live countdown is computed against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SleepState {
    pub reset: String,
    pub target_epoch: i64,
}

/// One issue in the run, in queue order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueEntry {
    pub number: u64,
    pub title: String,
    pub status: IssueStatus,
}

/// A tally of issues by terminal/active status, for the card's counter line.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub done: usize,
    pub skipped: usize,
    pub blocked: usize,
    pub infeasible: usize,
    pub needs_split: usize,
    pub non_green: usize,
    pub planning: usize,
    pub executing: usize,
}

/// The transport-agnostic state of a run, folded from its event stream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunState {
    /// The card title (derived by the caller, not from events).
    pub title: String,
    /// The queue size from `queue built`.
    pub total: usize,
    /// The issues that have entered the lifecycle, in the order first seen.
    pub issues: Vec<IssueEntry>,
    /// The current/active issue number (the "phase" pointer): its [`IssueStatus`]
    /// is the run's current phase.
    pub active: Option<u64>,
    /// The terminal summary, set when the run stops non-green or on the deadline.
    pub final_summary: Option<String>,
    /// The active usage-limit sleep, if the run is currently waiting for a reset.
    pub sleep: Option<SleepState>,
    /// Whether the run has reached its terminal state. The worker flips this to
    /// `true` just before the final card render so the card grows its `🏁` footer
    /// (the consolidated single-component card — ADR-0007 D3); it stays `false`
    /// through the live run so the issue list is the last visible group.
    pub finished: bool,
    /// Live: the end-of-run knowledge consolidation is in progress over this many
    /// loose notes. Set by `KnowledgeConsolidating`, cleared on completion (and
    /// hidden once the run is `finished`, so a failed session leaves no stale line).
    pub consolidating: Option<u64>,
    /// Terminal: notes folded into `KNOWLEDGE.md` by the end-of-run consolidation,
    /// surfaced as a `📚` segment in the card footer.
    pub consolidated: Option<u64>,
}

impl RunState {
    /// A fresh state with a known title and queue size (the worker seeds these
    /// since the card is sent before the first folded event).
    pub fn new(title: impl Into<String>, total: usize) -> Self {
        RunState {
            title: title.into(),
            total,
            ..Default::default()
        }
    }

    /// Resolve a possibly-zero issue number (the adapter's execution events carry
    /// no number) to the active issue. `None` when it is zero and there is no
    /// active issue — e.g. an `IssueStarted` was dropped under back-pressure — so
    /// callers skip rather than materialize a phantom issue `#0`.
    fn resolve(&self, number: u64) -> Option<u64> {
        if number == 0 {
            self.active
        } else {
            Some(number)
        }
    }

    /// Get the entry for `number`, inserting a planning placeholder if unseen.
    fn entry_mut(&mut self, number: u64) -> &mut IssueEntry {
        if let Some(pos) = self.issues.iter().position(|e| e.number == number) {
            &mut self.issues[pos]
        } else {
            self.issues.push(IssueEntry {
                number,
                title: String::new(),
                status: IssueStatus::Planning,
            });
            self.issues.last_mut().expect("just pushed")
        }
    }

    /// Fold one event into the state. Pure over `(self, event)`.
    pub fn apply(&mut self, event: RunEvent) {
        match event {
            RunEvent::QueueBuilt { count, .. } => {
                self.total = count as usize;
            }
            RunEvent::IssueStarted { number, title } => {
                self.active = Some(number);
                let e = self.entry_mut(number);
                e.title = title;
                e.status = IssueStatus::Planning;
            }
            // Live-region only in the presenter; the card fold ignores it (the
            // planner's model/effort never changes an issue's status).
            RunEvent::Planning { .. } => {}
            RunEvent::PlanWritten {
                number, open_steps, ..
            } => {
                let Some(n) = self.resolve(number) else {
                    return;
                };
                let e = self.entry_mut(n);
                e.status = if open_steps == 0 {
                    IssueStatus::Infeasible
                } else {
                    IssueStatus::Planning
                };
            }
            RunEvent::Executing { number, .. } => {
                let Some(n) = self.resolve(number) else {
                    return;
                };
                self.entry_mut(n).status = IssueStatus::Executing;
            }
            RunEvent::IssueClosed { number, .. } => {
                let Some(n) = self.resolve(number) else {
                    return;
                };
                self.entry_mut(n).status = IssueStatus::Done;
            }
            RunEvent::NonGreen { number, outcome } => {
                let Some(n) = self.resolve(number) else {
                    return;
                };
                // A `Blocked` execution outcome is its own status; everything else
                // non-green collapses to NonGreen.
                let status = if outcome.starts_with("Blocked") {
                    IssueStatus::Blocked
                } else {
                    IssueStatus::NonGreen
                };
                self.entry_mut(n).status = status;
                self.final_summary = Some(format!("stopped on #{n}: {outcome}"));
            }
            RunEvent::Skipped { number, .. } => {
                self.entry_mut(number).status = IssueStatus::Skipped;
            }
            RunEvent::NeedsSplit { number } => {
                let Some(n) = self.resolve(number) else {
                    return;
                };
                self.entry_mut(n).status = IssueStatus::NeedsSplit;
            }
            RunEvent::Notice { .. } => {}
            RunEvent::DeadlinePassed { number } => {
                self.final_summary = Some(format!("deadline reached before #{number}"));
            }
            RunEvent::SleepStarted {
                reset,
                target_epoch,
            } => {
                self.sleep = Some(SleepState {
                    reset,
                    target_epoch,
                });
            }
            RunEvent::SleepEnded => {
                self.sleep = None;
            }
            RunEvent::KnowledgeConsolidating { notes } => {
                self.consolidating = Some(notes);
            }
            RunEvent::KnowledgeConsolidated { archived } => {
                self.consolidating = None;
                self.consolidated = Some(archived);
            }
        }
    }

    /// Tally the issues by status for the counter line.
    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for e in &self.issues {
            match e.status {
                IssueStatus::Done => c.done += 1,
                IssueStatus::Skipped => c.skipped += 1,
                IssueStatus::Blocked => c.blocked += 1,
                IssueStatus::Infeasible => c.infeasible += 1,
                IssueStatus::NeedsSplit => c.needs_split += 1,
                IssueStatus::NonGreen => c.non_green += 1,
                IssueStatus::Planning => c.planning += 1,
                IssueStatus::Executing => c.executing += 1,
            }
        }
        c
    }

    /// The active issue entry, if any.
    pub fn active_issue(&self) -> Option<&IssueEntry> {
        let n = self.active?;
        self.issues.iter().find(|e| e.number == n)
    }

    /// The most-recently-seen issue in a terminal status (for the collapsed card).
    pub fn most_recent_finished(&self) -> Option<&IssueEntry> {
        self.issues.iter().rev().find(|e| e.status.is_terminal())
    }
}

/// Fold a whole event stream into a [`RunState`], seeded with a title and size.
///
/// A convenience over repeated [`RunState::apply`], used by the fold tests and
/// available to the future ADR-0006 presenter; the live worker applies events one
/// at a time, so this is unused by the binary itself.
#[allow(dead_code)]
pub fn fold(
    title: impl Into<String>,
    total: usize,
    events: impl IntoIterator<Item = RunEvent>,
) -> RunState {
    let mut state = RunState::new(title, total);
    for event in events {
        state.apply(event);
    }
    state
}

// ---------------------------------------------------------------------------
// Canonical event decoder (ADR-0007 D6)
// ---------------------------------------------------------------------------

/// The typed fields extracted off one `tracing` event. Populated by the [`Visit`]
/// impl and consumed by [`event_to_runevent`]. The union of all fields across every
/// consumed event shape; unused fields remain at their `Default` values.
#[derive(Debug)]
pub struct EventFields {
    pub level: Level,
    pub message: String,
    pub number: Option<u64>,
    pub title: Option<String>,
    pub open_steps: Option<u64>,
    pub count: Option<u64>,
    pub budget_min: Option<u64>,
    pub order: Option<String>,
    pub outcome: Option<String>,
    pub reset: Option<String>,
    pub target_epoch: Option<i64>,
    pub model: Option<String>,
    pub tokens: Option<u64>,
    /// Reasoning effort label (`low`/`medium`/`high`); adapters also report it as
    /// `variant` (OpenCode), folded into the same slot.
    pub effort: Option<String>,
    /// Per-phase token breakdown carried on `plan written` / `green — issue
    /// closed`: `up` input, `cr` cache-read, `cw` cache-write, `out` output.
    pub up: Option<u64>,
    pub cr: Option<u64>,
    pub cw: Option<u64>,
    pub out: Option<u64>,
}

impl Default for EventFields {
    fn default() -> Self {
        EventFields {
            level: Level::INFO,
            message: String::new(),
            number: None,
            title: None,
            open_steps: None,
            count: None,
            budget_min: None,
            order: None,
            outcome: None,
            reset: None,
            target_epoch: None,
            model: None,
            tokens: None,
            effort: None,
            up: None,
            cr: None,
            cw: None,
            out: None,
        }
    }
}

impl Visit for EventFields {
    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            "number" => self.number = Some(value),
            "open_steps" => self.open_steps = Some(value),
            "count" => self.count = Some(value),
            "budget_min" => self.budget_min = Some(value),
            "tokens" => self.tokens = Some(value),
            "up" => self.up = Some(value),
            "cr" => self.cr = Some(value),
            "cw" => self.cw = Some(value),
            "out" => self.out = Some(value),
            _ => {}
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if field.name() == "target_epoch" {
            self.target_epoch = Some(value);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.message = value.to_string(),
            "title" => self.title = Some(value.to_string()),
            "order" => self.order = Some(value.to_string()),
            "outcome" => self.outcome = Some(value.to_string()),
            "reset" => self.reset = Some(value.to_string()),
            "model" => self.model = clean_opt(value),
            "effort" | "variant" => self.effort = clean_opt(value),
            _ => {}
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let rendered = format!("{value:?}");
        match field.name() {
            "message" => self.message = rendered,
            "title" => self.title = Some(rendered),
            "order" => self.order = Some(rendered),
            "outcome" => self.outcome = Some(rendered),
            "reset" => self.reset = Some(rendered.clone()),
            // Debug-formatted `Option<String>` / `&str` adapter fields: strip the
            // `Some("…")` / quote wrapping and treat `None`/empty as absent so the
            // decoder never carries a literal `None` or `""` into a display label.
            "model" => self.model = clean_opt(&rendered),
            "effort" | "variant" => self.effort = clean_opt(&rendered),
            _ => {}
        }
    }
}

/// Normalize a possibly-`Debug`-wrapped adapter field to a clean display string,
/// or `None` when it is absent/empty. Strips a `Some("…")` wrapper and surrounding
/// quotes (the `?`-formatted `Option<String>` form some adapters still emit) and
/// maps `None`/`""` to `None` so a label never reads `None` or blank.
fn clean_opt(raw: &str) -> Option<String> {
    let mut s = raw.trim();
    if s == "None" || s.is_empty() {
        return None;
    }
    if let Some(inner) = s.strip_prefix("Some(").and_then(|r| r.strip_suffix(')')) {
        s = inner.trim();
    }
    let s = s.trim_matches('"').trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Map an event's `(target, message, fields)` to a [`RunEvent`], or `None` for an
/// event the run ignores. Pure over its inputs and unit-tested per consumed event
/// so an event/model drift fails a test (ADR-0007 D6).
///
/// Level wins: a WARN or ERROR event emits [`RunEvent::Notice`] regardless of its
/// message content, so a warning can never silently vanish into an unmatched arm.
///
/// `target` is currently informational — the message + fields uniquely identify
/// every consumed event — but kept in the signature for future disambiguation.
/// Lift the four breakdown fields + pricing model off an event into a [`UsageLite`].
fn usage_from(fields: &EventFields) -> UsageLite {
    UsageLite {
        input: fields.up.unwrap_or(0),
        cache_read: fields.cr.unwrap_or(0),
        cache_creation: fields.cw.unwrap_or(0),
        output: fields.out.unwrap_or(0),
        model: fields.model.clone(),
    }
}

pub fn event_to_runevent(target: &str, message: &str, fields: &EventFields) -> Option<RunEvent> {
    let _ = target;
    // Level wins: WARN and ERROR always surface as Notice.
    if fields.level == Level::WARN || fields.level == Level::ERROR {
        return Some(RunEvent::Notice {
            level: fields.level,
            message: message.to_string(),
        });
    }
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
        // The adapter's planning events carry no issue number; the fold applies
        // the display model/effort to the active issue's planning spinner.
        "planning with claude -p" | "planning with codex exec" | "planning with opencode run" => {
            Some(RunEvent::Planning {
                model: fields.model.clone(),
                effort: fields.effort.clone(),
            })
        }
        "plan written" => Some(RunEvent::PlanWritten {
            number,
            open_steps: fields.open_steps.unwrap_or(0),
            usage: usage_from(fields),
        }),
        // The adapter's execution events carry no issue number; the fold applies
        // this to the active issue.
        "executing with interactive claude over the PTY"
        | "executing with headless claude -p loop"
        | "executing with codex exec"
        | "executing with opencode run" => Some(RunEvent::Executing {
            number,
            budget_min: fields.budget_min.unwrap_or(0),
            model: fields.model.clone().unwrap_or_default(),
            effort: fields.effort.clone(),
        }),
        "green — issue closed" => Some(RunEvent::IssueClosed {
            number,
            tokens: fields.tokens.unwrap_or(0),
            usage: usage_from(fields),
        }),
        "non-green — stopping run" => Some(RunEvent::NonGreen {
            number,
            outcome: fields.outcome.clone().unwrap_or_default(),
        }),
        "bundle plan — needs split" => Some(RunEvent::NeedsSplit { number }),
        "blocked by open issue(s) — skipping" => Some(RunEvent::Skipped {
            number,
            kind: SkipKind::BlockedBy,
        }),
        "stop-before label — halting run before this issue" => Some(RunEvent::Skipped {
            number,
            kind: SkipKind::StopBefore,
        }),
        // The verify gate stayed red after the repair budget: the issue is left
        // open and the queue marches on (ADR-0011). Surfaced as a skip so the miss
        // is visible in the live card and the final counts.
        "verify gate failed — skipping issue" => Some(RunEvent::Skipped {
            number,
            kind: SkipKind::VerifyFailed,
        }),
        "deadline passed — not starting issue" => Some(RunEvent::DeadlinePassed { number }),
        // The run entered a usage-limit sleep; the fold carries the reset hint and
        // the wake anchor for a live countdown.
        "usage limit — waiting for reset" => Some(RunEvent::SleepStarted {
            reset: fields.reset.clone().unwrap_or_default(),
            target_epoch: fields.target_epoch.unwrap_or(0),
        }),
        "reset reached — resuming" => Some(RunEvent::SleepEnded),
        // The end-of-run knowledge consolidation trigger: both events reuse the
        // generic `count` field (notes in / notes archived).
        "consolidating knowledge" => Some(RunEvent::KnowledgeConsolidating {
            notes: fields.count.unwrap_or(0),
        }),
        "knowledge consolidated" => Some(RunEvent::KnowledgeConsolidated {
            archived: fields.count.unwrap_or(0),
        }),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_lifecycle_yields_expected_statuses_and_summary() {
        let events = vec![
            RunEvent::QueueBuilt {
                count: 2,
                order: vec![1, 2],
            },
            RunEvent::IssueStarted {
                number: 1,
                title: "one".into(),
            },
            RunEvent::PlanWritten {
                number: 1,
                open_steps: 3,
                usage: UsageLite::default(),
            },
            // The execution event carries no number; it must land on the active issue.
            RunEvent::Executing {
                number: 0,
                budget_min: 45,
                model: String::new(),
                effort: None,
            },
            RunEvent::IssueClosed {
                number: 1,
                tokens: 0,
                usage: UsageLite::default(),
            },
            RunEvent::IssueStarted {
                number: 2,
                title: "two".into(),
            },
            RunEvent::PlanWritten {
                number: 2,
                open_steps: 2,
                usage: UsageLite::default(),
            },
            RunEvent::Executing {
                number: 0,
                budget_min: 45,
                model: String::new(),
                effort: None,
            },
            RunEvent::NonGreen {
                number: 2,
                outcome: "Stuck".into(),
            },
        ];
        let state = fold("title", 2, events);
        assert_eq!(state.total, 2);
        assert_eq!(state.issues.len(), 2);
        assert_eq!(state.issues[0].status, IssueStatus::Done);
        assert_eq!(state.issues[0].title, "one");
        assert_eq!(state.issues[1].status, IssueStatus::NonGreen);
        let summary = state.final_summary.as_deref().unwrap();
        assert!(summary.contains("#2"), "summary: {summary}");
        assert!(summary.contains("Stuck"), "summary: {summary}");
    }

    #[test]
    fn plan_written_with_zero_steps_is_infeasible() {
        let mut state = RunState::new("t", 1);
        state.apply(RunEvent::IssueStarted {
            number: 5,
            title: "x".into(),
        });
        state.apply(RunEvent::PlanWritten {
            number: 5,
            open_steps: 0,
            usage: UsageLite::default(),
        });
        assert_eq!(state.issues[0].status, IssueStatus::Infeasible);
    }

    #[test]
    fn needs_split_upgrades_infeasible_and_decodes_from_stable_message() {
        // The runner emits "plan written" (0 steps) then "bundle plan — needs
        // split"; the fold must land on NeedsSplit, not stay Infeasible.
        let mut state = RunState::new("t", 1);
        state.apply(RunEvent::IssueStarted {
            number: 3,
            title: "W1 bundle".into(),
        });
        state.apply(RunEvent::PlanWritten {
            number: 3,
            open_steps: 0,
            usage: UsageLite::default(),
        });
        assert_eq!(state.issues[0].status, IssueStatus::Infeasible);
        state.apply(RunEvent::NeedsSplit { number: 3 });
        assert_eq!(state.issues[0].status, IssueStatus::NeedsSplit);
        assert!(state.issues[0].status.is_terminal());
        assert_eq!(state.counts().needs_split, 1);
        assert_eq!(state.counts().infeasible, 0);

        // Decoder: the stable runner message maps to the typed event.
        assert_eq!(
            event_to_runevent(
                "ralphy_core::runner",
                "bundle plan — needs split",
                &EventFields {
                    message: "bundle plan — needs split".into(),
                    number: Some(3),
                    ..Default::default()
                }
            ),
            Some(RunEvent::NeedsSplit { number: 3 })
        );
    }

    #[test]
    fn skipped_event_sets_skipped_status() {
        let mut state = RunState::new("t", 1);
        state.apply(RunEvent::Skipped {
            number: 9,
            kind: SkipKind::BlockedBy,
        });
        assert_eq!(state.issues[0].status, IssueStatus::Skipped);
    }

    #[test]
    fn non_green_blocked_outcome_maps_to_blocked() {
        let mut state = RunState::new("t", 1);
        state.apply(RunEvent::IssueStarted {
            number: 1,
            title: "a".into(),
        });
        state.apply(RunEvent::NonGreen {
            number: 1,
            outcome: "Blocked".into(),
        });
        assert_eq!(state.issues[0].status, IssueStatus::Blocked);
    }

    #[test]
    fn deadline_event_sets_terminal_summary() {
        let mut state = RunState::new("t", 3);
        state.apply(RunEvent::DeadlinePassed { number: 7 });
        assert!(state.final_summary.as_deref().unwrap().contains("#7"));
    }

    #[test]
    fn zero_numbered_event_without_active_is_ignored() {
        // An `Executing` (number 0) whose `IssueStarted` was dropped under
        // back-pressure must not materialize a phantom issue `#0`.
        let mut state = RunState::new("t", 1);
        state.apply(RunEvent::Executing {
            number: 0,
            budget_min: 45,
            model: String::new(),
            effort: None,
        });
        assert!(state.issues.is_empty());
    }

    #[test]
    fn sleep_started_sets_state_and_sleep_ended_clears_it() {
        let mut state = RunState::new("t", 1);
        assert!(state.sleep.is_none());
        state.apply(RunEvent::SleepStarted {
            reset: "14:30".into(),
            target_epoch: 1_700_000_000,
        });
        let sleep = state.sleep.as_ref().expect("sleep set on start");
        assert_eq!(sleep.reset, "14:30");
        assert_eq!(sleep.target_epoch, 1_700_000_000);
        state.apply(RunEvent::SleepEnded);
        assert!(state.sleep.is_none(), "resume clears the sleep");
    }

    #[test]
    fn counts_tally_each_status() {
        let mut state = RunState::new("t", 4);
        state.apply(RunEvent::IssueStarted {
            number: 1,
            title: "a".into(),
        });
        state.apply(RunEvent::IssueClosed {
            number: 1,
            tokens: 0,
            usage: UsageLite::default(),
        });
        state.apply(RunEvent::Skipped {
            number: 2,
            kind: SkipKind::BlockedBy,
        });
        state.apply(RunEvent::IssueStarted {
            number: 3,
            title: "c".into(),
        });
        let c = state.counts();
        assert_eq!(c.done, 1);
        assert_eq!(c.skipped, 1);
        assert_eq!(c.planning, 1);
        assert_eq!(state.active_issue().map(|e| e.number), Some(3));
        assert_eq!(state.most_recent_finished().map(|e| e.number), Some(2));
    }

    // -----------------------------------------------------------------------
    // Decoder suite
    // -----------------------------------------------------------------------

    fn decode(fields: EventFields) -> Option<RunEvent> {
        event_to_runevent("ralphy_core::runner", &fields.message.clone(), &fields)
    }

    #[test]
    fn decoder_maps_each_consumed_info_shape() {
        assert_eq!(
            decode(EventFields {
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
            decode(EventFields {
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
            decode(EventFields {
                message: "plan written".into(),
                number: Some(7),
                open_steps: Some(0),
                up: Some(12_400),
                cr: Some(184_000),
                cw: Some(8_100),
                out: Some(3_200),
                model: Some("claude-opus-4".into()),
                ..Default::default()
            }),
            Some(RunEvent::PlanWritten {
                number: 7,
                open_steps: 0,
                usage: UsageLite {
                    input: 12_400,
                    cache_read: 184_000,
                    cache_creation: 8_100,
                    output: 3_200,
                    model: Some("claude-opus-4".into()),
                },
            })
        );
        // The adapter's planning event seeds the planning spinner's model/effort.
        assert_eq!(
            decode(EventFields {
                message: "planning with claude -p".into(),
                model: Some("opus".into()),
                effort: Some("high".into()),
                ..Default::default()
            }),
            Some(RunEvent::Planning {
                model: Some("opus".into()),
                effort: Some("high".into()),
            })
        );
        assert_eq!(
            decode(EventFields {
                message: "executing with interactive claude over the PTY".into(),
                budget_min: Some(45),
                model: Some("claude-sonnet-4".into()),
                effort: Some("medium".into()),
                ..Default::default()
            }),
            Some(RunEvent::Executing {
                number: 0,
                budget_min: 45,
                model: "claude-sonnet-4".into(),
                effort: Some("medium".into()),
            })
        );
        assert_eq!(
            decode(EventFields {
                message: "executing with headless claude -p loop".into(),
                budget_min: Some(30),
                ..Default::default()
            }),
            Some(RunEvent::Executing {
                number: 0,
                budget_min: 30,
                model: String::new(),
                effort: None,
            })
        );
        assert_eq!(
            decode(EventFields {
                message: "green — issue closed".into(),
                number: Some(7),
                tokens: Some(1_200_000),
                up: Some(41_200),
                cr: Some(902_000),
                cw: Some(22_000),
                out: Some(18_400),
                model: Some("claude-sonnet-4".into()),
                ..Default::default()
            }),
            Some(RunEvent::IssueClosed {
                number: 7,
                tokens: 1_200_000,
                usage: UsageLite {
                    input: 41_200,
                    cache_read: 902_000,
                    cache_creation: 22_000,
                    output: 18_400,
                    model: Some("claude-sonnet-4".into()),
                },
            })
        );
        assert_eq!(
            decode(EventFields {
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
            decode(EventFields {
                message: "blocked by open issue(s) — skipping".into(),
                number: Some(7),
                ..Default::default()
            }),
            Some(RunEvent::Skipped {
                number: 7,
                kind: SkipKind::BlockedBy
            })
        );
        assert_eq!(
            decode(EventFields {
                message: "stop-before label — halting run before this issue".into(),
                number: Some(8),
                ..Default::default()
            }),
            Some(RunEvent::Skipped {
                number: 8,
                kind: SkipKind::StopBefore
            })
        );
    }

    #[test]
    fn decoder_maps_sleep_and_deadline_events() {
        assert_eq!(
            decode(EventFields {
                message: "usage limit — waiting for reset".into(),
                reset: Some("14:30".into()),
                target_epoch: Some(1_700_000_000),
                ..Default::default()
            }),
            Some(RunEvent::SleepStarted {
                reset: "14:30".into(),
                target_epoch: 1_700_000_000
            })
        );
        assert_eq!(
            decode(EventFields {
                message: "reset reached — resuming".into(),
                ..Default::default()
            }),
            Some(RunEvent::SleepEnded)
        );
        assert_eq!(
            decode(EventFields {
                message: "deadline passed — not starting issue".into(),
                number: Some(7),
                ..Default::default()
            }),
            Some(RunEvent::DeadlinePassed { number: 7 })
        );
    }

    #[test]
    fn decoder_level_wins_warn_and_error_emit_notice() {
        // WARN: level wins even when message matches a known INFO shape.
        let result = decode(EventFields {
            level: Level::WARN,
            message: "queue built".into(),
            count: Some(3),
            order: Some("#1 -> #2 -> #3".into()),
            ..Default::default()
        });
        assert_eq!(
            result,
            Some(RunEvent::Notice {
                level: Level::WARN,
                message: "queue built".into()
            })
        );
        // ERROR: same treatment.
        let result = decode(EventFields {
            level: Level::ERROR,
            message: "something bad happened".into(),
            ..Default::default()
        });
        assert_eq!(
            result,
            Some(RunEvent::Notice {
                level: Level::ERROR,
                message: "something bad happened".into()
            })
        );
    }

    #[test]
    fn decoder_maps_knowledge_consolidation_events() {
        assert_eq!(
            decode(EventFields {
                message: "consolidating knowledge".into(),
                count: Some(4),
                ..Default::default()
            }),
            Some(RunEvent::KnowledgeConsolidating { notes: 4 })
        );
        assert_eq!(
            decode(EventFields {
                message: "knowledge consolidated".into(),
                count: Some(4),
                ..Default::default()
            }),
            Some(RunEvent::KnowledgeConsolidated { archived: 4 })
        );
    }

    #[test]
    fn apply_knowledge_consolidation_sets_then_clears_live_and_records_count() {
        let mut state = RunState::new("t", 1);
        state.apply(RunEvent::KnowledgeConsolidating { notes: 4 });
        assert_eq!(state.consolidating, Some(4));
        assert_eq!(state.consolidated, None);
        // Completion clears the live flag and records the archived tally.
        state.apply(RunEvent::KnowledgeConsolidated { archived: 4 });
        assert_eq!(state.consolidating, None);
        assert_eq!(state.consolidated, Some(4));
    }

    #[test]
    fn decoder_unknown_info_message_returns_none() {
        assert_eq!(
            decode(EventFields {
                message: "some unrelated log line".into(),
                ..Default::default()
            }),
            None
        );
    }

    #[test]
    fn apply_notice_is_noop_on_runstate() {
        let mut before = RunState::new("t", 1);
        before.apply(RunEvent::IssueStarted {
            number: 1,
            title: "a".into(),
        });
        let mut after = before.clone();
        after.apply(RunEvent::Notice {
            level: Level::WARN,
            message: "some warning".into(),
        });
        assert_eq!(before, after);
    }

    #[test]
    fn apply_skipped_with_both_kinds_sets_skipped_status() {
        let mut state = RunState::new("t", 2);
        state.apply(RunEvent::Skipped {
            number: 1,
            kind: SkipKind::BlockedBy,
        });
        state.apply(RunEvent::Skipped {
            number: 2,
            kind: SkipKind::StopBefore,
        });
        assert_eq!(state.issues[0].status, IssueStatus::Skipped);
        assert_eq!(state.issues[1].status, IssueStatus::Skipped);
    }

    #[test]
    fn apply_executing_with_model_sets_executing_status() {
        let mut state = RunState::new("t", 1);
        state.apply(RunEvent::IssueStarted {
            number: 1,
            title: "a".into(),
        });
        state.apply(RunEvent::Executing {
            number: 1,
            budget_min: 45,
            model: "claude-opus-4".into(),
            effort: None,
        });
        assert_eq!(state.issues[0].status, IssueStatus::Executing);
    }
}
