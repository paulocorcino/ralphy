use std::collections::HashSet;

use super::*;

mod store;

/// A literal pre-#406 desk: one window table, no `[checkouts]`.
const OLD_DESK_TOML: &str = "[[windows]]\nid = \"w1\"\nrepo = \"owner/repo\"\n\
    agent = \"claude\"\nkind = \"console\"\nmax = false\nts = 1\n\n[windows.rect]\n\
    left = 1.0\ntop = 2.0\nwidth = 3.0\nheight = 4.0\n";

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
        console_name: None,
        done_mark: false,
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

/// A field that is off or empty is absent from the wire and from desk.toml, so
/// the older record, fence and desk shapes are byte-identical and an older
/// shell reading the desk sees exactly what it always saw: the lock amendment,
/// #411's `checkout` on a primary's record, ADR-0066 §1's `consoleName`, and an
/// empty `[checkouts]` map (the pre-#406 wire body). Each "set" row is the
/// control that the key is written once it has a value.
#[test]
fn a_field_that_is_off_or_empty_is_not_serialised() {
    let json = |r: &DeskRecord| serde_json::to_string(r).unwrap();
    let fence_json = |f: &DeskFence| serde_json::to_string(f).unwrap();
    let mut held = record("w2", 2);
    held.locked = true;
    let mut held_fence = fence("f2", "planning", 2);
    held_fence.locked = true;
    let mut linked = record("w2", 2);
    linked.checkout = Some("wt-a".into());
    let mut named = record("w2", 2);
    named.console_name = Some("fincal #1".into());
    let desk = DeskStore {
        rev: 0,
        generation: 0,
        windows: vec![record("w1", 1)],
        fences: vec![fence("f1", "backend", 1)],
        notes: vec![],
        checkouts: BTreeMap::new(),
    };
    let desk_toml = toml::to_string_pretty(&desk).unwrap();
    // (case, serialised text, needle, whether the needle is written)
    let rows = [
        ("record lock off", json(&record("w1", 1)), "locked", false),
        ("record lock on", json(&held), r#""locked":true"#, true),
        (
            "fence lock off",
            fence_json(&fence("f1", "backend", 1)),
            "locked",
            false,
        ),
        (
            "fence lock on",
            fence_json(&held_fence),
            r#""locked":true"#,
            true,
        ),
        (
            "primary checkout",
            json(&record("w1", 1)),
            "checkout",
            false,
        ),
        (
            "linked checkout",
            json(&linked),
            r#""checkout":"wt-a""#,
            true,
        ),
        (
            "no console name",
            json(&record("w1", 1)),
            "consoleName",
            false,
        ),
        (
            "console name",
            json(&named),
            r#""consoleName":"fincal #1""#,
            true,
        ),
        ("desk.toml lock off", desk_toml.clone(), "locked", false),
        (
            "desk.toml no console name",
            desk_toml.clone(),
            "consoleName",
            false,
        ),
        (
            "desk.toml empty checkouts",
            toml::to_string_pretty(&DeskStore::default()).unwrap(),
            "checkouts",
            false,
        ),
    ];
    for (case, text, needle, written) in rows {
        assert_eq!(text.contains(needle), written, "{case}: {text}");
    }
    assert_eq!(
        serde_json::to_string(&DeskStore::default()).unwrap(),
        r#"{"windows":[],"fences":[],"notes":[]}"#,
        "the empty desk's wire body"
    );
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

/// A `desk.toml` written before a collection existed loads with that
/// collection empty — never a parse failure that reads as an empty desk: no
/// `[checkouts]` (ADR-0063 §4), no `fences` (#340), no `notes`, no
/// `consoleName` (ADR-0066 §1). Hand-written on purpose: round-tripping THIS
/// build would emit the new keys and prove nothing.
#[test]
fn a_desk_from_an_older_build_loads() {
    let windows_only = r#"
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
"#;
    // (case, desk.toml, the one window's id)
    let rows = [
        ("pre-#406 desk", OLD_DESK_TOML, "w1"),
        ("pre-#340 windows-only desk", windows_only, "w-legacy"),
    ];
    for (case, text, id) in rows {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desk.toml");
        std::fs::write(&path, text).unwrap();
        let store = load_from(&path).expect("desk reads");
        assert_eq!(store.windows.len(), 1, "{case}: the one window loads");
        assert_eq!(store.windows[0].id, id, "{case}");
        assert_eq!(store.windows[0].console_name, None, "{case}");
        assert!(store.fences.is_empty(), "{case}: no fences");
        assert!(store.notes.is_empty(), "{case}: no notes");
        assert!(store.checkouts.is_empty(), "{case}: no checkouts");
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desk.toml");
    std::fs::write(&path, windows_only).unwrap();
    assert_eq!(
        load_from(&path).expect("desk reads").windows,
        vec![record("w-legacy", 5)]
    );
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
fn prune_keeps_the_cap_newest_by_ts_in_layout_order() {
    let records: Vec<DeskRecord> = (1..=36).map(|n| record(&format!("w{n}"), n)).collect();
    let kept: Vec<String> = prune(records, &HashSet::new())
        .into_iter()
        .map(|r| r.id)
        .collect();
    let expected: Vec<String> = (7..=36).map(|n| format!("w{n}")).collect();
    assert_eq!(kept, expected, "the six lowest-ts records are evicted");
}

#[test]
fn prune_preserves_layout_order_not_ts_order() {
    // Layout order and ts order disagree: the survivors must come back in
    // LAYOUT order (w36 first), not newest-first.
    let records: Vec<DeskRecord> = (1..=36).map(|n| record(&format!("w{n}"), 37 - n)).collect();
    let kept: Vec<String> = prune(records, &HashSet::new())
        .into_iter()
        .map(|r| r.id)
        .collect();
    let expected: Vec<String> = (1..=30).map(|n| format!("w{n}")).collect();
    assert_eq!(kept, expected);
}

/// A record a live session serves is never cut, even the oldest, and even
/// when the live ones alone are over the cap (ADR-0050 amendment 2026-10-04).
#[test]
fn prune_never_cuts_a_record_a_live_session_serves() {
    let records: Vec<DeskRecord> = (1..=36).map(|n| record(&format!("w{n}"), n)).collect();
    let live: HashSet<String> = ["w1".to_owned()].into();
    let kept: Vec<String> = prune(records.clone(), &live)
        .into_iter()
        .map(|r| r.id)
        .collect();
    let mut expected = vec!["w1".to_owned()];
    expected.extend((8..=36).map(|n| format!("w{n}")));
    assert_eq!(kept, expected, "w1 stays; the oldest unpinned make room");

    let all: HashSet<String> = records.iter().map(|r| r.id.clone()).collect();
    assert_eq!(
        prune(records.clone(), &all),
        records,
        "31+ live records all stay"
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

/// The ubiquitous language is a deliverable of the desk issues: CONTEXT.md
/// defines each desk term. The definitions' wording is the glossary's own.
// pin: CONTEXT.md defines the desk terms —
// a glossary has no behavior; only its text shows it.
#[test]
fn context_md_defines_the_desk_terms() {
    let context = include_str!("../../../../CONTEXT.md");
    for term in ["**Stage / viewport**", "**Fence**", "**Note**", "**Card**"] {
        assert!(context.contains(term), "CONTEXT.md must define {term}");
    }
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
fn prune_leaves_an_under_cap_desk_untouched() {
    let records: Vec<DeskRecord> = (1..=5).map(|n| record(&format!("w{n}"), n)).collect();
    assert_eq!(prune(records.clone(), &HashSet::new()), records);
}

// ---- notes (ADR-0064 §2) --------------------------------------------------

fn note(id: &str, path: &str, ts: i64) -> DeskNote {
    DeskNote {
        id: id.into(),
        repo: "owner/repo".into(),
        path: path.into(),
        checkout: None,
        rect: DeskRect {
            left: 80.0,
            top: 120.0,
            width: 240.0,
            height: 180.0,
        },
        locked: false,
        ts,
    }
}

#[test]
fn prune_notes_keeps_the_newest_cap_in_layout_order() {
    let max = i64::try_from(NOTE_MAX).expect("the note cap fits i64");
    let notes: Vec<DeskNote> = (1..=max + 1)
        .map(|n| note(&format!("n{n}"), &format!("a{n}.note"), n))
        .collect();
    let kept: Vec<String> = prune_notes(notes).into_iter().map(|n| n.id).collect();
    let expected: Vec<String> = (2..=max + 1).map(|n| format!("n{n}")).collect();
    // The negative control is `n1`: an inverted or unsorted prune keeps it.
    assert_eq!(kept, expected, "the lowest-ts card is evicted");
    assert_eq!(kept.len(), NOTE_MAX);
}

#[test]
fn a_console_name_is_written_on_the_wire() {
    let mut named = record("w1", 1);
    named.console_name = Some("fincal #1 ação 🚀".into());
    let json = serde_json::to_string(&named).unwrap();
    assert!(
        json.contains(r#""consoleName":"fincal #1 ação 🚀""#),
        "json={json}"
    );
}

/// A desk saved to desk.toml loads back equal. `order` names tables that must
/// appear in that order in the file: TOML writes values before tables, so a
/// `notes` field declared after `checkouts` would fail every save of a desk
/// with both. `absent` and `counts` pin what stays off the page.
struct RoundTrip {
    name: &'static str,
    store: DeskStore,
    order: &'static [&'static str],
    absent: &'static [&'static str],
    counts: &'static [(&'static str, usize)],
}

fn round_trip_rows() -> Vec<RoundTrip> {
    let mut no_session = record("w1", 1);
    no_session.session_id = None;
    let mut full = record("w2", 2);
    full.repo = "01ARZ3NDEKTSV4RRFFQ69G5FAW/owner/repo".into();
    full.daemon_id = Some("01ARZ3NDEKTSV4RRFFQ69G5FAW".into());
    full.environment = Some("WSL: Ubuntu-22.04".into());
    full.checkout = Some("wt-a".into());
    full.max = true;
    full.locked = true;
    let mut named = record("w1", 1);
    named.console_name = Some("fincal #1 ação 🚀".into());
    let mut held_note = note("n2", "docs/plan.note", 2);
    held_note.checkout = Some("wt-a".into());
    held_note.locked = true;
    let mut held_fence = fence("f2", "planning", 20);
    held_fence.locked = true;
    vec![
        RoundTrip {
            name: "records",
            store: DeskStore {
                windows: vec![no_session, full],
                ..Default::default()
            },
            order: &[],
            absent: &[],
            counts: &[],
        },
        RoundTrip {
            name: "console name",
            store: DeskStore {
                windows: vec![named, record("w2", 2)],
                ..Default::default()
            },
            order: &[],
            absent: &[],
            counts: &[],
        },
        RoundTrip {
            name: "note card placement",
            store: DeskStore {
                windows: vec![record("w1", 1)],
                fences: vec![fence("f1", "backend", 1)],
                notes: vec![note("n1", ".ralphy/notes/standup.note", 1), held_note],
                ..Default::default()
            },
            order: &["[[fences]]", "[[notes]]"],
            absent: &["locked = false"],
            counts: &[("checkout = ", 1)],
        },
        RoundTrip {
            name: "a note and a checkout together",
            store: DeskStore {
                windows: vec![record("w1", 1), record("w2", 2)],
                fences: vec![fence("f1", "backend", 10), held_fence],
                notes: vec![note("n1", ".ralphy/notes/a.note", 1)],
                checkouts: BTreeMap::from([
                    ("owner/repo".to_owned(), "wt-a".to_owned()),
                    (
                        "01ARZ3NDEKTSV4RRFFQ69G5FAW/owner/repo".to_owned(),
                        "wt-b".to_owned(),
                    ),
                ]),
                ..Default::default()
            },
            order: &["[[fences]]", "[[notes]]", "[checkouts]"],
            absent: &[],
            counts: &[],
        },
    ]
}

#[test]
fn a_desk_round_trips_through_desk_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desk.toml");
    for row in round_trip_rows() {
        save_to(&row.store, &path).unwrap_or_else(|e| panic!("{}: {e:#}", row.name));
        assert_eq!(
            load_from(&path).expect("desk reads"),
            row.store,
            "{}",
            row.name
        );
        let text = std::fs::read_to_string(&path).unwrap();
        let at = |needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("{}: {needle} in {text}", row.name))
        };
        let places: Vec<usize> = row.order.iter().map(|n| at(n)).collect();
        assert!(
            places.windows(2).all(|w| w[0] < w[1]),
            "{}: {text}",
            row.name
        );
        for needle in row.absent {
            assert!(!text.contains(needle), "{}: {text}", row.name);
        }
        for (needle, n) in row.counts {
            assert_eq!(text.matches(needle).count(), *n, "{}: {text}", row.name);
        }
    }
}
