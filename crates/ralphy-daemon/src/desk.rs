//! The desk store (ADR-0050): which consoles were open and where each window
//! sat, persisted as `desk.toml` beside `repos.toml` in the global daemon store.
//!
//! The desk is DAEMON state, not browser state — a workbench session survives
//! the browser, so its window must too. Modelled on `registry`: pure sync,
//! path-explicit, tests pass a temp path and never touch the process env.
//!
//! The record shape mirrors what the shell already writes (wb-console.js
//! `persistWin`), spelled `camelCase` on the wire and in the file so one
//! spelling holds end to end.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

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
/// `session_id` is a volatile attribute (a restarted daemon hands out ids from 1
/// again), which is why it is nullable and never the key.
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
/// Identity is `(checkout, path)`; `id` is the shell's stable handle for the
/// DOM node, the same role it plays for a window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeskNote {
    pub id: String,
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeskStore {
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

/// The `PUT /api/desk` body. Strict where [`DeskStore`] is lenient — a body
/// that is not this exact shape must be a refusal, never an empty desk that
/// replaces the operator's layout. The FILE type stays lenient so a `desk.toml`
/// written by a newer daemon degrades per-field instead of to nothing.
///
/// MAP-ONLY, and that is the guard. `#[derive(Deserialize)]` calls
/// `deserialize_struct`, which `serde_json` satisfies from a JSON SEQUENCE as
/// well as from an object: the pre-#340 bare array a stale browser tab PUTs for
/// a desk it thinks is empty then lands as a valid upload and wipes the
/// operator's fences with a `200`. Measured (#340): with per-field
/// `#[serde(default)]`, `[]` did it; with both fields required, `[[],[]]` still
/// did — the two elements satisfy the two fields POSITIONALLY.
/// `deny_unknown_fields` covers neither. Only calling `deserialize_map` does.
#[derive(Debug)]
pub struct DeskUpload {
    pub windows: Vec<DeskRecord>,
    pub fences: Vec<DeskFence>,
    pub notes: Vec<DeskNote>,
    pub checkouts: BTreeMap<String, String>,
    /// What this page DELETED since its last read (ADR-0050 amendment
    /// 2026-09-20). Its presence is the protocol switch: an upload carrying it
    /// is folded into the stored desk by [`merge`] — the other pages' records
    /// survive — and one without it (a shell older than the amendment) is the
    /// wholesale replace it always was, since that shell cannot say what it
    /// deleted and a merge would resurrect every close.
    pub removed: Option<DeskRemoved>,
}

/// The ids an upload retires, per record type. A record absent from an
/// upload is not thereby deleted — the page may simply not have read it yet —
/// so deletion has to be said.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeskRemoved {
    #[serde(default)]
    pub windows: Vec<String>,
    #[serde(default)]
    pub fences: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub checkouts: Vec<String>,
}

impl<'de> Deserialize<'de> for DeskUpload {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            windows: Vec<DeskRecord>,
            fences: Vec<DeskFence>,
            // Optional on the wire: a shell older than ADR-0064 sends none.
            #[serde(default)]
            notes: Vec<DeskNote>,
            // Optional on the wire: a shell older than ADR-0063 §4 sends none.
            #[serde(default)]
            checkouts: BTreeMap<String, String>,
            // Optional on the wire, and its absence MEANS something — see
            // `DeskUpload::removed`.
            #[serde(default)]
            removed: Option<DeskRemoved>,
        }

        struct MapOnly;
        impl<'de> serde::de::Visitor<'de> for MapOnly {
            type Value = DeskUpload;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a desk upload object with `windows` and `fences`")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<Self::Value, A::Error> {
                let fields =
                    Fields::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                Ok(DeskUpload {
                    windows: fields.windows,
                    fences: fields.fences,
                    notes: fields.notes,
                    checkouts: fields.checkouts,
                    removed: fields.removed,
                })
            }
        }

        d.deserialize_map(MapOnly)
    }
}

/// The daemon-side cap on desk records. Enforced here rather than trusting the
/// uploaded array — a browser upload does not get to define the size.
pub const DESK_MAX: usize = 24;

/// The daemon-side cap on note cards (ADR-0064 §2). Its own const for the same
/// reason [`FENCE_MAX`] is: a card is cheap to place and expensive to lose, and
/// 32 open notes is already a stage nobody is reading.
pub const NOTE_MAX: usize = 32;

/// The daemon-side cap on fences. Its own const, not shared with [`DESK_MAX`]
/// (ADR-0051 §10): a fence holds several consoles, so a dozen named regions
/// already out-runs the 24-window cap.
pub const FENCE_MAX: usize = 12;

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

/// Keep the [`DESK_MAX`] newest records by `ts`, PRESERVING layout order. Live
/// windows cannot be pinned here — the daemon does not know which windows are on
/// screen — so the shell pins them before uploading and this is the backstop.
pub fn prune(records: Vec<DeskRecord>) -> Vec<DeskRecord> {
    keep_newest_by_ts(records, DESK_MAX, |r| r.ts)
}

/// Keep the [`FENCE_MAX`] newest fences by `ts`, PRESERVING layout order.
pub fn prune_fences(fences: Vec<DeskFence>) -> Vec<DeskFence> {
    keep_newest_by_ts(fences, FENCE_MAX, |f| f.ts)
}

/// Keep the [`NOTE_MAX`] newest cards by `ts`, PRESERVING layout order.
pub fn prune_notes(notes: Vec<DeskNote>) -> Vec<DeskNote> {
    keep_newest_by_ts(notes, NOTE_MAX, |n| n.ts)
}

/// Fold an upload into the stored desk (ADR-0050 amendment 2026-09-20). Three
/// pages on one desk each hold a mirror only as fresh as their last read, so
/// the store — the one place that sees every write — is where the union is
/// taken. Per id the NEWER `ts` wins (a page's own mutation is newer by
/// construction; its stale copy of another page's record is not; a tie goes
/// to the upload, which is the one that just happened); an id the upload
/// retires is dropped whatever the store holds; a stored record the upload
/// does not mention survives. Order is the upload's — the page's own layout
/// order, which decides a contended session in the shell — with the store's
/// unmentioned records after it. Checkouts have no `ts`: the upload's entry
/// wins per ref, a retired ref is dropped, the rest of the store's stay.
///
/// An upload WITHOUT `removed` is a shell that predates the amendment. It
/// cannot say what it deleted, so it is the wholesale replace it always was.
pub fn merge(stored: DeskStore, up: DeskUpload) -> DeskStore {
    let Some(removed) = up.removed else {
        return DeskStore {
            windows: up.windows,
            fences: up.fences,
            notes: up.notes,
            checkouts: up.checkouts,
        };
    };
    let windows = fold_by_id(
        stored.windows,
        up.windows,
        &removed.windows,
        |r| r.id.as_str(),
        |r| r.ts,
    );
    let fences = fold_by_id(
        stored.fences,
        up.fences,
        &removed.fences,
        |f| f.id.as_str(),
        |f| f.ts,
    );
    let notes = fold_by_id(
        stored.notes,
        up.notes,
        &removed.notes,
        |n| n.id.as_str(),
        |n| n.ts,
    );
    let mut checkouts = stored.checkouts;
    for gone in &removed.checkouts {
        checkouts.remove(gone);
    }
    checkouts.extend(up.checkouts);
    DeskStore {
        windows,
        fences,
        notes,
        checkouts,
    }
}

fn fold_by_id<T>(
    stored: Vec<T>,
    uploaded: Vec<T>,
    removed: &[String],
    id: impl Fn(&T) -> &str,
    ts: impl Fn(&T) -> i64,
) -> Vec<T> {
    let gone: std::collections::HashSet<&str> = removed.iter().map(String::as_str).collect();
    let mut theirs: std::collections::HashMap<String, T> = stored
        .into_iter()
        .filter(|r| !gone.contains(id(r)))
        .map(|r| (id(&r).to_string(), r))
        .collect();
    let mut out: Vec<T> = Vec::with_capacity(uploaded.len() + theirs.len());
    for ours in uploaded {
        if gone.contains(id(&ours)) {
            continue;
        }
        match theirs.remove(id(&ours)) {
            Some(stored) if ts(&stored) > ts(&ours) => out.push(stored),
            _ => out.push(ours),
        }
    }
    // The store's unmentioned records, in the store's own order.
    let mut rest: Vec<T> = theirs.into_values().collect();
    rest.sort_by_key(|r| ts(r));
    out.extend(rest);
    out
}

/// Load the desk from `path`. A missing file AND a corrupt one both read as an
/// empty desk — deliberately diverging from [`crate::registry::load_from`],
/// which returns a `Result`: an unreadable layout costs a cascaded stage, not a
/// daemon, so this must never give a caller a startup failure to propagate.
pub fn load_from(path: &Path) -> DeskStore {
    match std::fs::read_to_string(path) {
        Ok(text) => match toml::from_str(&text) {
            Ok(store) => store,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "unreadable desk layout — starting from an empty desk");
                DeskStore::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DeskStore::default(),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "could not read desk layout — starting from an empty desk");
            DeskStore::default()
        }
    }
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

/// Write the desk to `path` owner-only, creating the parent directory.
///
/// ATOMIC: written to a sibling temp file and renamed over the target, because
/// this is written on every drag, resize and close. A truncated in-place write
/// would read back as an empty desk ([`load_from`] maps a parse error to
/// `default()`), losing the layout silently instead of noisily.
pub fn save_to(store: &DeskStore, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let text = toml::to_string_pretty(store).context("serializing desk layout")?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    crate::registry::set_owner_only(&tmp)?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("replacing {} with {}", path.display(), tmp.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests;
