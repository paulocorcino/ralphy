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
    #[serde(default)]
    pub ts: i64,
}

/// The persisted desk: the records in LAYOUT order (the order decides which
/// record wins a contended session in the shell's `reconcileDesk`).
///
/// `fences` sits AFTER `windows` because TOML emits an array-of-tables at the
/// end of the document: a scalar field declared after `[[windows]]` would land
/// inside the last window's table.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeskStore {
    #[serde(default)]
    pub windows: Vec<DeskRecord>,
    #[serde(default)]
    pub fences: Vec<DeskFence>,
}

/// The `PUT /api/desk` body. Strict where [`DeskStore`] is lenient — a body
/// that is not this exact shape must be a refusal, never an empty desk that
/// replaces the operator's layout. The FILE type stays lenient so a `desk.toml`
/// written by a newer daemon degrades per-field instead of to nothing.
///
/// BOTH fields are REQUIRED, and that is the whole guard: serde deserializes a
/// struct from a JSON SEQUENCE as well as from a map, so with
/// `#[serde(default)]` on both the pre-#340 bare array `[]` — what a stale
/// browser tab PUTs for a desk it thinks is empty — parses as a valid empty
/// upload and wipes the operator's fences with a `200` (measured, #340).
/// `deny_unknown_fields` does not cover that path.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeskUpload {
    pub windows: Vec<DeskRecord>,
    pub fences: Vec<DeskFence>,
}

/// The daemon-side cap on desk records. Enforced here rather than trusting the
/// uploaded array — a browser upload does not get to define the size.
pub const DESK_MAX: usize = 24;

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
/// float literal (`1e999`) dies in the `Json` extractor as `422` ("number out of
/// range") and never reaches the route (#340).
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
mod tests {
    use super::*;

    fn record(id: &str, ts: i64) -> DeskRecord {
        DeskRecord {
            id: id.into(),
            repo: "owner/repo".into(),
            agent: "claude".into(),
            kind: "console".into(),
            rect: DeskRect {
                left: 10.0,
                top: 20.0,
                width: 640.0,
                height: 480.0,
            },
            max: false,
            session_id: Some(7),
            ts,
        }
    }

    fn fence(id: &str, name: &str, ts: i64) -> DeskFence {
        DeskFence {
            id: id.into(),
            name: name.into(),
            rect: DeskRect {
                left: 40.0,
                top: 40.0,
                width: 720.0,
                height: 460.0,
            },
            ts,
        }
    }

    #[test]
    fn round_trip_preserves_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desk.toml");
        let mut a = record("w1", 1);
        a.session_id = None;
        let mut b = record("w2", 2);
        b.max = true;
        let store = DeskStore {
            windows: vec![a, b],
            fences: vec![],
        };
        save_to(&store, &path).unwrap();

        let back = load_from(&path);
        assert_eq!(back, store, "the desk round-trips through desk.toml");
        assert_eq!(back.windows[0].session_id, None);
        assert!(back.windows[1].max);
    }

    #[test]
    fn wire_key_is_camel_case_session_id() {
        let json = serde_json::to_string(&record("w1", 3)).unwrap();
        assert!(
            json.contains("\"sessionId\":7"),
            "the shell writes `sessionId`; got {json}"
        );
    }

    #[test]
    fn missing_file_reads_as_empty_desk() {
        let dir = tempfile::tempdir().unwrap();
        let store = load_from(&dir.path().join("desk.toml"));
        assert!(store.windows.is_empty());
    }

    #[test]
    fn corrupt_file_reads_as_empty_desk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desk.toml");
        std::fs::write(&path, "not a toml { ][").unwrap();
        let store = load_from(&path);
        assert!(
            store.windows.is_empty(),
            "a corrupt desk reads empty and does not panic"
        );
    }

    #[test]
    fn prune_keeps_24_newest_by_ts_in_layout_order() {
        let records: Vec<DeskRecord> = (1..=30).map(|n| record(&format!("w{n}"), n)).collect();
        let kept: Vec<String> = prune(records).into_iter().map(|r| r.id).collect();
        let expected: Vec<String> = (7..=30).map(|n| format!("w{n}")).collect();
        assert_eq!(kept, expected, "the six lowest-ts records are evicted");
    }

    #[test]
    fn prune_preserves_layout_order_not_ts_order() {
        // Layout order and ts order disagree: the survivors must come back in
        // LAYOUT order (w30 first), not newest-first.
        let records: Vec<DeskRecord> = (1..=30).map(|n| record(&format!("w{n}"), 31 - n)).collect();
        let kept: Vec<String> = prune(records).into_iter().map(|r| r.id).collect();
        let expected: Vec<String> = (1..=24).map(|n| format!("w{n}")).collect();
        assert_eq!(kept, expected);
    }

    #[test]
    fn a_failed_save_leaves_the_previous_desk_intact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desk.toml");
        let good = DeskStore {
            windows: vec![record("w-keep", 1)],
            fences: vec![],
        };
        save_to(&good, &path).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();

        // A path whose PARENT is a regular file: `create_dir_all` fails on both
        // Windows and unix, so the error return is exercised portably.
        let blocked = path.join("nested").join("desk.toml");
        let err = save_to(
            &DeskStore {
                windows: vec![record("w-lost", 2)],
                fences: vec![],
            },
            &blocked,
        )
        .expect_err("writing under a regular file must fail");
        assert!(
            format!("{err:#}").contains("creating"),
            "the context chain names the step: {err:#}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "the good desk is byte-identical after a failed save"
        );
        assert_eq!(load_from(&path).windows[0].id, "w-keep");
    }

    #[test]
    fn save_leaves_no_temp_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desk.toml");
        save_to(
            &DeskStore {
                windows: vec![record("w1", 1)],
                fences: vec![],
            },
            &path,
        )
        .unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "desk.toml")
            .collect();
        assert!(
            leftovers.is_empty(),
            "the rename consumed the temp: {leftovers:?}"
        );
    }

    #[test]
    fn a_non_finite_rect_is_not_sane() {
        let mut r = record("w1", 1);
        assert!(rect_is_sane(&r.rect));
        r.rect.left = f64::INFINITY;
        assert!(!rect_is_sane(&r.rect));
        r.rect.left = f64::NAN;
        assert!(!rect_is_sane(&r.rect));
    }

    #[test]
    fn rect_is_sane_rejects_a_negative_origin() {
        let mut r = record("w1", 1);
        r.rect.left = -1.0;
        assert!(!rect_is_sane(&r.rect), "a negative left is off the stage");
        // The boundary itself is ON the stage — the origin is pinned AT 0,0, not
        // past it, so a window flush against the corner must still persist.
        r.rect.left = 0.0;
        assert!(rect_is_sane(&r.rect), "left = 0 is the pinned origin");
        r.rect.top = -1.0;
        assert!(!rect_is_sane(&r.rect), "a negative top is off the stage");
        r.rect.top = 0.0;
        assert!(rect_is_sane(&r.rect));
    }

    #[test]
    fn load_from_does_not_filter_a_legacy_negative_rect() {
        // The guard is WRITE-path only: a desk written before it must reopen
        // byte-identical, not be silently pruned to nothing (issue #336).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desk.toml");
        let mut legacy = record("w-legacy", 1);
        legacy.rect.left = -40.0;
        std::fs::write(
            &path,
            toml::to_string_pretty(&DeskStore {
                windows: vec![legacy.clone()],
                fences: vec![],
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(load_from(&path).windows, vec![legacy]);
    }

    /// The ubiquitous language is a deliverable of this issue, not a courtesy —
    /// and `Desk layout` carried a claim ADR-0050 had already superseded. Pinned
    /// here so a doc edit that drops either is a red test, not a silent drift.
    /// Every needle sits on ONE source line of CONTEXT.md: a pin spanning a hard
    /// wrap is a false red.
    #[test]
    fn context_md_names_the_stage_and_the_viewport() {
        let context = include_str!("../../../CONTEXT.md");
        for pin in ["**Stage / viewport**", "overflow:auto", "bring into view"] {
            assert!(
                context.contains(pin),
                "CONTEXT.md must define {pin} (#336, #337)"
            );
        }
        assert!(
            context.contains("The daemon's record of"),
            "the desk lives in the daemon (ADR-0050), not the browser (#336)"
        );
        assert!(
            !context.contains("The browser's record of"),
            "the pre-ADR-0050 `Desk layout` wording must be corrected (#336)"
        );
    }

    /// A `desk.toml` written before #340 has no `fences` key at all — it must
    /// keep loading verbatim, with the fence list empty rather than the whole
    /// desk degrading to `default()`. Hand-written on purpose: round-tripping
    /// THIS build would emit the new key and prove nothing.
    #[test]
    fn a_windows_only_desk_loads_with_no_fences() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desk.toml");
        let legacy = record("w-legacy", 5);
        std::fs::write(
            &path,
            r#"
[[windows]]
id = "w-legacy"
repo = "owner/repo"
agent = "claude"
kind = "console"
max = false
sessionId = 7
ts = 5

[windows.rect]
left = 10.0
top = 20.0
width = 640.0
height = 480.0
"#,
        )
        .unwrap();
        let store = load_from(&path);
        assert_eq!(store.windows, vec![legacy]);
        assert!(store.fences.is_empty(), "a pre-#340 desk has no fences");
    }

    #[test]
    fn fences_round_trip_through_desk_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desk.toml");
        let store = DeskStore {
            windows: vec![record("w1", 1)],
            fences: vec![fence("f1", "backend", 10), fence("f2", "planning", 20)],
        };
        save_to(&store, &path).unwrap();

        let back = load_from(&path);
        assert_eq!(back, store, "fences round-trip through desk.toml");
        assert_eq!(back.fences[1].name, "planning");
    }

    #[test]
    fn prune_fences_keeps_the_12_newest_by_ts() {
        let fences: Vec<DeskFence> = (1..=13)
            .map(|n| fence(&format!("f{n}"), "region", n))
            .collect();
        let kept: Vec<String> = prune_fences(fences).into_iter().map(|f| f.id).collect();
        let expected: Vec<String> = (2..=13).map(|n| format!("f{n}")).collect();
        // The negative control is `f1`: an inverted or unsorted prune keeps it.
        assert_eq!(kept, expected, "the lowest-ts fence is evicted");
    }

    #[test]
    fn a_fence_rect_is_sane_on_the_same_rule_as_a_window() {
        let mut f = fence("f1", "backend", 1);
        assert!(rect_is_sane(&f.rect));
        f.rect.left = f64::INFINITY;
        assert!(!rect_is_sane(&f.rect));
        f.rect.left = f64::NAN;
        assert!(!rect_is_sane(&f.rect));
        f.rect.left = 0.0;
        assert!(rect_is_sane(&f.rect), "left = 0 is the pinned origin");
        f.rect.top = -1.0;
        assert!(!rect_is_sane(&f.rect), "a negative top is off the stage");
        f.rect.top = 0.0;
        assert!(rect_is_sane(&f.rect));
    }

    /// Every needle sits on ONE source line of CONTEXT.md: a pin spanning a hard
    /// wrap is a false red.
    #[test]
    fn context_md_names_the_fence() {
        let context = include_str!("../../../CONTEXT.md");
        for pin in ["**Fence**", "floor tier"] {
            assert!(context.contains(pin), "CONTEXT.md must define {pin} (#340)");
        }
    }

    #[test]
    fn prune_leaves_an_under_cap_desk_untouched() {
        let records: Vec<DeskRecord> = (1..=5).map(|n| record(&format!("w{n}"), n)).collect();
        assert_eq!(prune(records.clone()), records);
    }
}
