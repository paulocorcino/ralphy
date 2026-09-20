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
    pub checkouts: Vec<String>,
}

impl<'de> Deserialize<'de> for DeskUpload {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            windows: Vec<DeskRecord>,
            fences: Vec<DeskFence>,
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
    let mut checkouts = stored.checkouts;
    for gone in &removed.checkouts {
        checkouts.remove(gone);
    }
    checkouts.extend(up.checkouts);
    DeskStore {
        windows,
        fences,
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
mod tests {
    use super::*;

    /// A literal pre-#406 desk: one window table, no `[checkouts]`.
    const OLD_DESK_TOML: &str = "[[windows]]\nid = \"w1\"\nrepo = \"owner/repo\"\nagent = \"claude\"\nkind = \"console\"\nmax = false\nts = 1\n\n[windows.rect]\nleft = 1.0\ntop = 2.0\nwidth = 3.0\nheight = 4.0\n";

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
            daemon_id: None,
            environment: None,
            checkout: None,
            locked: false,
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
            locked: false,
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
        b.repo = "01ARZ3NDEKTSV4RRFFQ69G5FAW/owner/repo".into();
        b.daemon_id = Some("01ARZ3NDEKTSV4RRFFQ69G5FAW".into());
        b.environment = Some("WSL: Ubuntu-22.04".into());
        b.checkout = Some("wt-a".into());
        b.max = true;
        b.locked = true;
        let store = DeskStore {
            windows: vec![a, b],
            fences: vec![],
            checkouts: BTreeMap::new(),
        };
        save_to(&store, &path).unwrap();

        let back = load_from(&path);
        assert_eq!(back, store, "the desk round-trips through desk.toml");
        assert_eq!(back.windows[0].session_id, None);
        assert_eq!(
            back.windows[1].repo,
            "01ARZ3NDEKTSV4RRFFQ69G5FAW/owner/repo"
        );
        assert_eq!(
            back.windows[1].environment.as_deref(),
            Some("WSL: Ubuntu-22.04")
        );
        assert!(back.windows[1].max);
        assert_eq!(back.windows[1].checkout.as_deref(), Some("wt-a"));
        assert_eq!(
            back.windows[0].checkout, None,
            "the primary's record has none"
        );
        assert!(back.windows[1].locked, "the lock survives desk.toml");
        assert!(!back.windows[0].locked);
    }

    /// Lock amendment: `locked` is absent from the wire and from desk.toml
    /// when off — the pre-lock record and fence shapes are byte-identical, so
    /// an older shell reading the desk sees exactly what it always saw.
    #[test]
    fn a_lock_that_is_off_is_not_serialised() {
        let json = serde_json::to_string(&record("w1", 1)).unwrap();
        assert!(!json.contains("locked"), "json={json}");
        let json = serde_json::to_string(&fence("f1", "backend", 1)).unwrap();
        assert!(!json.contains("locked"), "json={json}");
        let mut held = record("w2", 2);
        held.locked = true;
        let json = serde_json::to_string(&held).unwrap();
        assert!(json.contains(r#""locked":true"#), "json={json}");
        let mut held = fence("f2", "planning", 2);
        held.locked = true;
        let json = serde_json::to_string(&held).unwrap();
        assert!(json.contains(r#""locked":true"#), "json={json}");
        let store = DeskStore {
            windows: vec![record("w1", 1)],
            fences: vec![fence("f1", "backend", 1)],
            checkouts: BTreeMap::new(),
        };
        let toml = toml::to_string_pretty(&store).unwrap();
        assert!(!toml.contains("locked"), "toml={toml}");
    }

    /// A shell that sent `locked: null` would have every PUT refused: the field
    /// is a plain `bool`, and this pins that a `null` is NOT read as `false`.
    #[test]
    fn a_null_lock_is_refused_not_read_as_off() {
        let json = r#"{"id":"w1","rect":{"left":0,"top":0,"width":1,"height":1},"locked":null}"#;
        assert!(serde_json::from_str::<DeskRecord>(json).is_err());
        let json = r#"{"id":"f1","rect":{"left":0,"top":0,"width":1,"height":1},"locked":null}"#;
        assert!(serde_json::from_str::<DeskFence>(json).is_err());
    }

    /// #411: a record's `checkout` is absent from the wire when `None` — the
    /// pre-#411 record shape is byte-identical for a console on the primary.
    #[test]
    fn a_primary_records_checkout_is_not_serialised() {
        let json = serde_json::to_string(&record("w1", 1)).unwrap();
        assert!(!json.contains("checkout"), "json={json}");
        let mut linked = record("w2", 2);
        linked.checkout = Some("wt-a".into());
        let json = serde_json::to_string(&linked).unwrap();
        assert!(json.contains(r#""checkout":"wt-a""#), "json={json}");
    }

    /// ADR-0063 §4: the third desk record type survives the TOML round trip
    /// (declared last so `[checkouts]` lands at top level after `[[windows]]`).
    #[test]
    fn round_trip_preserves_checkouts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desk.toml");
        let store = DeskStore {
            windows: vec![record("w1", 1), record("w2", 2)],
            fences: vec![],
            checkouts: BTreeMap::from([
                ("owner/repo".to_string(), "wt-a".to_string()),
                (
                    "01ARZ3NDEKTSV4RRFFQ69G5FAW/owner/repo".to_string(),
                    "wt-b".to_string(),
                ),
            ]),
        };
        save_to(&store, &path).unwrap();

        let back = load_from(&path);
        assert_eq!(back, store, "checkouts round-trip through desk.toml");
        assert_eq!(back.checkouts["owner/repo"], "wt-a");
        assert_eq!(back.windows.len(), 2, "the table did not swallow a window");
    }

    /// A `desk.toml` written before ADR-0063 §4 has no `[checkouts]` and loads
    /// with an empty map — never a parse failure that reads as an empty desk.
    #[test]
    fn old_desk_without_checkouts_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desk.toml");
        std::fs::write(&path, OLD_DESK_TOML).unwrap();
        let store = load_from(&path);
        assert_eq!(store.windows.len(), 1, "the one window loads");
        assert_eq!(store.windows[0].id, "w1");
        assert!(store.checkouts.is_empty());
    }

    /// An empty map is not serialised, so the wire body a shell without
    /// selections sees is exactly the pre-#406 `{"windows":[],"fences":[]}`.
    #[test]
    fn empty_checkouts_are_not_serialised() {
        assert_eq!(
            serde_json::to_string(&DeskStore::default()).unwrap(),
            r#"{"windows":[],"fences":[]}"#
        );
        let toml = toml::to_string_pretty(&DeskStore::default()).unwrap();
        assert!(!toml.contains("checkouts"), "toml={toml}");
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
            checkouts: BTreeMap::new(),
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
                checkouts: BTreeMap::new(),
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
                checkouts: BTreeMap::new(),
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
                checkouts: BTreeMap::new(),
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
            checkouts: BTreeMap::new(),
        };
        save_to(&store, &path).unwrap();

        let back = load_from(&path);
        assert_eq!(back, store, "fences round-trip through desk.toml");
        assert_eq!(back.fences[1].name, "planning");
    }

    #[test]
    fn a_locked_fence_round_trips_through_desk_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desk.toml");
        let mut held = fence("f2", "planning", 20);
        held.locked = true;
        let store = DeskStore {
            windows: vec![],
            fences: vec![fence("f1", "backend", 10), held],
            checkouts: BTreeMap::new(),
        };
        save_to(&store, &path).unwrap();
        let back = load_from(&path);
        assert_eq!(back, store);
        assert!(back.fences[1].locked);
        assert!(!back.fences[0].locked);
    }

    /// The fold moves whole records by `ts`, so a lock rides with the newer
    /// copy: a page whose mirror predates the lock cannot unlock by accident.
    #[test]
    fn merge_carries_the_lock_with_the_newer_record() {
        let mut held = record("a", 20);
        held.locked = true;
        let mut held_fence = fence("f", "backend", 20);
        held_fence.locked = true;
        let stored = DeskStore {
            windows: vec![held],
            fences: vec![held_fence],
            ..Default::default()
        };
        let up = upload(
            vec![record("a", 10)],
            vec![fence("f", "backend", 10)],
            Some(DeskRemoved::default()),
        );
        let out = merge(stored, up);
        assert!(out.windows[0].locked, "a stale unlock does not win");
        assert!(out.fences[0].locked);
        let mut freed = record("a", 30);
        freed.locked = false;
        let stored = DeskStore {
            windows: vec![{
                let mut r = record("a", 20);
                r.locked = true;
                r
            }],
            ..Default::default()
        };
        let out = merge(
            stored,
            upload(vec![freed], vec![], Some(DeskRemoved::default())),
        );
        assert!(!out.windows[0].locked, "a newer unlock does");
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
        // NEGATIVE CONTROL: the entry has to say a fence is DAEMON state and is
        // never bound to a project — the two claims the whole slice rests on. An
        // entry reduced to a bare heading would pass the pins above.
        assert!(
            context.contains("never bound to a project"),
            "the **Fence** entry must keep a fence free-form (#340)"
        );
        assert!(
            !context.contains("a fence belongs to a project"),
            "a fence is never a project's (#340)"
        );
    }

    #[test]
    fn prune_leaves_an_under_cap_desk_untouched() {
        let records: Vec<DeskRecord> = (1..=5).map(|n| record(&format!("w{n}"), n)).collect();
        assert_eq!(prune(records.clone()), records);
    }

    // ---- merge (ADR-0050 amendment 2026-09-20) --------------------------------

    fn upload(
        windows: Vec<DeskRecord>,
        fences: Vec<DeskFence>,
        removed: Option<DeskRemoved>,
    ) -> DeskUpload {
        DeskUpload {
            windows,
            fences,
            checkouts: BTreeMap::new(),
            removed,
        }
    }

    #[test]
    fn merge_keeps_the_newest_copy_of_each_record_and_the_stores_unmentioned_ones() {
        let mut stale = record("a", 20);
        stale.session_id = Some(7); // the daemon's copy, newer: another page recorded the id
        let mut theirs = record("b", 30);
        theirs.session_id = Some(2);
        let stored = DeskStore {
            windows: vec![stale.clone(), record("b", 25), record("d", 1)],
            ..Default::default()
        };
        let mut ours = record("a", 10);
        ours.session_id = None; // this page's stale mirror of `a`
        let up = upload(
            vec![ours, theirs.clone(), record("c", 5)],
            vec![],
            Some(DeskRemoved::default()),
        );
        let out = merge(stored, up);
        assert_eq!(
            out.windows
                .iter()
                .map(|r| (r.id.as_str(), r.ts, r.session_id))
                .collect::<Vec<_>>(),
            vec![
                ("a", 20, Some(7)),
                ("b", 30, Some(2)),
                ("c", 5, Some(7)),
                ("d", 1, Some(7))
            ],
            "newest per id; the upload's order first, the store's unmentioned after"
        );
    }

    #[test]
    fn merge_drops_what_the_upload_retires_even_when_the_store_is_newer() {
        let stored = DeskStore {
            windows: vec![record("closed", 99), record("kept", 1)],
            fences: vec![fence("f-gone", "old", 99), fence("f-kept", "keep", 1)],
            checkouts: BTreeMap::from([
                ("o/r".to_string(), "wt".to_string()),
                ("o/s".to_string(), "wt-s".to_string()),
            ]),
        };
        let mut up = upload(
            vec![],
            vec![],
            Some(DeskRemoved {
                windows: vec!["closed".into()],
                fences: vec!["f-gone".into()],
                checkouts: vec!["o/r".into()],
            }),
        );
        up.checkouts.insert("o/t".into(), "wt-t".into());
        let out = merge(stored, up);
        assert_eq!(
            out.windows
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            vec!["kept"]
        );
        assert_eq!(
            out.fences.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            vec!["f-kept"]
        );
        assert_eq!(
            out.checkouts,
            BTreeMap::from([
                ("o/s".to_string(), "wt-s".to_string()),
                ("o/t".to_string(), "wt-t".to_string()),
            ])
        );
    }

    #[test]
    fn an_upload_without_removed_is_the_wholesale_replace_an_older_shell_means() {
        let stored = DeskStore {
            windows: vec![record("theirs", 99)],
            fences: vec![fence("f", "old", 99)],
            checkouts: BTreeMap::from([("o/r".to_string(), "wt".to_string())]),
        };
        let out = merge(stored, upload(vec![record("mine", 1)], vec![], None));
        assert_eq!(
            out.windows
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            vec!["mine"]
        );
        assert!(out.fences.is_empty());
        assert!(out.checkouts.is_empty());
    }

    #[test]
    fn the_upload_body_takes_removed_and_still_refuses_a_bare_array() {
        let json = r#"{"windows":[],"fences":[],"removed":{"windows":["x"]}}"#;
        let up: DeskUpload = serde_json::from_str(json).expect("the amended shape parses");
        assert_eq!(
            up.removed.expect("removed present").windows,
            vec!["x".to_string()]
        );
        let legacy: DeskUpload = serde_json::from_str(r#"{"windows":[],"fences":[]}"#)
            .expect("the pre-amendment shape parses");
        assert!(legacy.removed.is_none());
        assert!(serde_json::from_str::<DeskUpload>("[[],[]]").is_err());
    }
}
