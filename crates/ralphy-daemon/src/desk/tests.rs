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
    let context = include_str!("../../../../CONTEXT.md");
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
    let context = include_str!("../../../../CONTEXT.md");
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
