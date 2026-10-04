//! The desk store (ADR-0050): which consoles were open and where each window
//! sat, persisted as `desk.toml` beside `repos.toml` in the global daemon store.
//!
//! The desk is DAEMON state, not browser state — a workbench session survives
//! the browser, so its window must too. Modelled on `registry`: pure sync,
//! path-explicit, tests pass a temp path and never touch the process env.
//!
//! The record shape is spelled `camelCase` on the wire and in the file so one
//! spelling holds end to end. A page writes the desk as a list of desk
//! changes ([`apply`]), never as whole records.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod apply;
pub mod history;
mod store;

pub use store::{is_parse_error, load_from, move_aside, save_to};

/// A window's restore box, in absolute STAGE pixels. No proportional or
/// per-resolution form: the stage is a plane whose origin is pinned at 0,0, so a
/// desk saved on a larger monitor reopens verbatim and the viewport scrolls over
/// it (ADR-0051 §4, superseding ADR-0050 §4's refit-on-restore).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeskRect {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

/// One desk record: a window keyed by its STABLE client-side `id`. The daemon's
/// `session_id` is a volatile attribute (a session does not outlive its daemon,
/// so a restart leaves every recorded id naming nothing), which is why it is
/// nullable and never the key.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeskRecord {
    pub id: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub kind: String,
    pub rect: DeskRect,
    #[serde(default)]
    pub max: bool,
    #[serde(default)]
    pub session_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// The worktree the console was launched in (#411; ADR-0063 §4 amendment):
    /// a NAME, written by the shell from the daemon's own `session-open`
    /// announcement, so a relaunch after a daemon restart lands the console
    /// back in the tree it lived in. `None` is the primary tree, and is not
    /// serialised so an older desk and an older shell keep their exact shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout: Option<String>,
    /// The operator locked this console in place (ADR-0050 amendment
    /// 2026-09-20, lock): no client drags or resizes it. A plain `bool`, so a
    /// shell must send `true`/`false` and never `null`. `false` is not
    /// serialised, so an older desk and an older shell keep their exact shape.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    /// The name a person reads for this console (ADR-0066 §1; ADR-0050
    /// amendment 2026-09-27). A label, not an identity: `id` stays the key.
    /// `None` is not serialised, so an older desk and an older shell keep their
    /// exact shape. The store cuts it to [`CONSOLE_NAME_MAX`] characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console_name: Option<String>,
    #[serde(default)]
    pub ts: i64,
}

/// One fence: a named rectangle anchored on the stage, drawn on a floor tier
/// below every window (ADR-0051 §6). Free-form — never bound to a project, so it
/// may hold consoles from several repos.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeskFence {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub rect: DeskRect,
    /// The operator locked this fence in place (ADR-0051 §6 amendment
    /// 2026-09-20): no client moves, resizes or tiles it, and the consoles it
    /// holds refuse a drag too. Same shape rules as `DeskRecord::locked`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    #[serde(default)]
    pub ts: i64,
}

/// One note card on the stage (ADR-0064 §2): PLACEMENT only. The note itself
/// is the `.note` file at `path` inside `checkout` — its text, its title and
/// its colour live there, so closing a card and reopening it from the explorer
/// restores all three. The desk knows only where the card sat.
///
/// Identity is `(repo, checkout, path)`; `id` is the shell's stable handle for
/// the DOM node, the same role it plays for a window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeskNote {
    pub id: String,
    /// The project the file lives in, as the registry's ref. ADR-0064 §2 wrote
    /// the record as `{id, checkout, path, rect, locked}` and the ADR is
    /// amended here: the desk is one plane across every project, and the note
    /// verbs take a `repo` like every other verb, so `(checkout, path)` alone
    /// does not say which tree `path` is relative to.
    #[serde(default)]
    pub repo: String,
    /// The note file, repo-relative inside its checkout. Empty until the first
    /// save names it (ADR-0064 §9: creation has no dialog).
    #[serde(default)]
    pub path: String,
    /// The worktree the note lives in. Same shape rules as
    /// [`DeskRecord::checkout`]: `None` is the primary tree and is not
    /// serialised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout: Option<String>,
    pub rect: DeskRect,
    /// The operator locked this card in place. Same shape rules as
    /// [`DeskRecord::locked`]; a card inside a locked fence is read-only too,
    /// which the shell derives and does not store.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    #[serde(default)]
    pub ts: i64,
}

/// The persisted desk: the records in LAYOUT order (the order decides which
/// record wins a contended session in the shell's `reconcileDesk`).
///
/// `fences` sits AFTER `windows` because TOML emits an array-of-tables at the
/// end of the document: a scalar field declared after `[[windows]]` would land
/// inside the last window's table. `checkouts` is a table and comes LAST for
/// the same reason: `[checkouts]` after `[[fences]]` parses back at top level.
/// `rev` and `generation` are scalars, so they come FIRST.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeskStore {
    /// Raised by every write that changes the desk (ADR-0050 amendment
    /// 2026-10-04, changes, not the desk). A page ignores a desk whose `rev`
    /// is lower than the last one it took, so a slow read never puts an older
    /// desk on screen. `0`, and not serialised, until the first write.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rev: u64,
    /// The epoch ms of the last restore from the desk history (ADR-0050
    /// amendment 2026-10-04, desk history). A page sends the generation it
    /// loaded with each PUT, and an older one is refused: that page still
    /// shows the layout from before the restore. `0`, and not serialised,
    /// until the first restore.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub generation: u64,
    #[serde(default)]
    pub windows: Vec<DeskRecord>,
    #[serde(default)]
    pub fences: Vec<DeskFence>,
    /// The note cards (ADR-0064 §2). Between `fences` and `checkouts` for the
    /// same TOML reason the doc above gives: one more array-of-tables, still
    /// ahead of the `[checkouts]` table.
    #[serde(default)]
    pub notes: Vec<DeskNote>,
    /// The selected checkout per repo ref (ADR-0063 §4; ADR-0050 amendment):
    /// a worktree NAME, stored — not validated per read (a listing per desk
    /// read would be a spawn). Omitted when empty so an old desk and an old
    /// shell keep their exact `{ windows, fences }` shape.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub checkouts: BTreeMap<String, String>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// The daemon-side cap on desk records, checked on each `create`
/// ([`apply`]) and by a restore's prune: a browser does not get to define the
/// size. Raised from 24 by the ADR-0050 amendment of 2026-10-04.
pub const DESK_MAX: usize = 30;

/// The daemon-side cap on note cards (ADR-0064 §2). Its own const for the same
/// reason [`FENCE_MAX`] is: a card is cheap to place and expensive to lose, and
/// 32 open notes is already a stage nobody is reading.
pub const NOTE_MAX: usize = 32;

/// The daemon-side cap on fences. Its own const, not shared with [`DESK_MAX`]
/// (ADR-0051 §10): a fence holds several consoles, so a dozen named regions
/// already out-runs the window cap.
pub const FENCE_MAX: usize = 12;

/// The daemon-side cap on a console name, in `char`s (ADR-0066 §3). The input's
/// `maxlength` can be bypassed, so the store enforces it.
pub const CONSOLE_NAME_MAX: usize = 40;

/// A blank name is no name: it reads as `None`, so the stored name is kept.
fn cap_console_name(r: &mut DeskRecord) {
    if r.console_name
        .as_deref()
        .is_some_and(|n| n.trim().is_empty())
    {
        r.console_name = None;
    }
    if let Some(name) = r.console_name.as_mut() {
        if let Some((cut, _)) = name.char_indices().nth(CONSOLE_NAME_MAX) {
            name.truncate(cut);
        }
    }
}

/// Keep the `max` newest items by `ts`, PRESERVING layout order.
fn keep_newest_by_ts<T>(items: Vec<T>, max: usize, ts: impl Fn(&T) -> i64) -> Vec<T> {
    if items.len() <= max {
        return items;
    }
    let mut by_ts: Vec<usize> = (0..items.len()).collect();
    by_ts.sort_by(|&a, &b| ts(&items[b]).cmp(&ts(&items[a])));
    by_ts.truncate(max);
    let keep: std::collections::HashSet<usize> = by_ts.into_iter().collect();
    items
        .into_iter()
        .enumerate()
        .filter_map(|(i, r)| keep.contains(&i).then_some(r))
        .collect()
}

/// Keep every record in `live` and the newest others by `ts` up to
/// [`DESK_MAX`], PRESERVING layout order. `live` is the records a session of
/// this daemon serves (ADR-0050 amendment 2026-10-04): cutting one strands a
/// running console, and the next load adopts it at the cascade position. When
/// `live` alone is over the cap, all of it stays. Only a restore prunes: it
/// builds a whole desk, and a desk change never deletes a record by age.
pub fn prune(
    records: Vec<DeskRecord>,
    live: &std::collections::HashSet<String>,
) -> Vec<DeskRecord> {
    if records.len() <= DESK_MAX {
        return records;
    }
    let pinned = records.iter().filter(|r| live.contains(&r.id)).count();
    let room = DESK_MAX.saturating_sub(pinned);
    let mut by_ts: Vec<usize> = (0..records.len())
        .filter(|&i| !live.contains(&records[i].id))
        .collect();
    by_ts.sort_by(|&a, &b| records[b].ts.cmp(&records[a].ts));
    by_ts.truncate(room);
    let keep: std::collections::HashSet<usize> = by_ts.into_iter().collect();
    records
        .into_iter()
        .enumerate()
        .filter_map(|(i, r)| (live.contains(&r.id) || keep.contains(&i)).then_some(r))
        .collect()
}

/// Keep the [`FENCE_MAX`] newest fences by `ts`, PRESERVING layout order.
pub fn prune_fences(fences: Vec<DeskFence>) -> Vec<DeskFence> {
    keep_newest_by_ts(fences, FENCE_MAX, |f| f.ts)
}

/// Keep the [`NOTE_MAX`] newest cards by `ts`, PRESERVING layout order.
pub fn prune_notes(notes: Vec<DeskNote>) -> Vec<DeskNote> {
    keep_newest_by_ts(notes, NOTE_MAX, |n| n.ts)
}

/// Whether a rect is one this daemon will persist: every component finite, and
/// the origin on the stage.
///
/// Finite because TOML writes an infinity as `inf` and serializing it back to
/// JSON yields `null` — which the shell would render as `"nullpx"`. This is the
/// BACKSTOP, not the only gate: measured on `serde_json` 1.x, an out-of-range
/// float literal (`1e999`) dies in the `Json` extractor as a SYNTAX error
/// ("number out of range"), which axum answers `400`, and never reaches the
/// route (#340) — same status as this guard's own refusal, different body.
///
/// Non-negative because the stage's origin is pinned at 0,0 and the plane grows
/// right and down only (ADR-0051 §4): a negative `left`/`top` would mean
/// re-anchoring the origin and rewriting every other rect. The shell's drag and
/// resize both clamp at 0, so a negative origin can only arrive from a
/// hand-rolled client.
///
/// WRITE PATH ONLY — [`load_from`] does not filter, because a desk written
/// before this guard must still reopen byte-identical (issue #336).
pub fn rect_is_sane(r: &DeskRect) -> bool {
    r.left.is_finite()
        && r.top.is_finite()
        && r.width.is_finite()
        && r.height.is_finite()
        && r.left >= 0.0
        && r.top >= 0.0
}

#[cfg(test)]
mod tests;
