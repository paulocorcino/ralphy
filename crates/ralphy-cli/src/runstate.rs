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
//!
//! Split three ways (ADR-0022): [`state`] holds the pure fold/state machine,
//! [`event`] the semantic-event mapper, [`fields`] the raw `tracing` field
//! extractor. This root keeps the cross-cutting branding helpers and the two
//! types shared by more than one submodule.

mod event;
mod fields;
mod state;

pub use event::{event_to_runevent, RunEvent};
pub use fields::EventFields;
pub use state::{IssueEntry, IssueStatus, RunState, SleepState};
// `Counts`, `QueueRef`, and `fold` are not named via `crate::runstate::*` by any
// current consumer (only through `RunState::counts()`'s return type and
// `state.rs`'s own tests), but stay re-exported at this stable path per
// ADR-0022 rather than moved behind `state::` — a future consumer naming the
// type explicitly should not need to guess it lives in the private submodule.
#[allow(unused_imports)]
pub use state::{fold, Counts, QueueRef};

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

/// Why an issue was skipped: a `blocked-by` dependency, a `stop-before` label, a
/// human-return label that outranks its queue label (ADR-0016), or a verify gate
/// that stayed red after the runner's repair attempts (ADR-0011).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipKind {
    BlockedBy,
    StopBefore,
    HumanReturn,
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
