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
        console_name: None,
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
        notes: vec![],
        checkouts: BTreeMap::new(),
    };
    save_to(&store, &path).unwrap();

    let back = load_from(&path).expect("desk reads");
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
fn missing_file_reads_as_empty_desk() {
    let dir = tempfile::tempdir().unwrap();
    let store = load_from(&dir.path().join("desk.toml")).expect("a missing desk reads");
    assert!(store.windows.is_empty());
}

#[test]
fn a_corrupt_desk_is_a_failure_not_an_empty_desk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desk.toml");
    std::fs::write(&path, "windows = [\n").unwrap();
    let e = load_from(&path).expect_err("a corrupt desk is an error, not an empty desk");
    assert!(
        format!("{e:#}").contains("parsing desk layout"),
        "the error names the parse: {e:#}"
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
        notes: vec![],
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
            notes: vec![],
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
    assert_eq!(
        load_from(&path).expect("desk reads").windows[0].id,
        "w-keep"
    );
}

#[test]
fn save_leaves_no_temp_file_behind() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desk.toml");
    save_to(
        &DeskStore {
            windows: vec![record("w1", 1)],
            fences: vec![],
            notes: vec![],
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
            notes: vec![],
            checkouts: BTreeMap::new(),
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(load_from(&path).expect("desk reads").windows, vec![legacy]);
}

/// The ubiquitous language is a deliverable of the desk issues: CONTEXT.md
/// defines each desk term. The definitions' wording is the glossary's own.
#[test]
fn context_md_defines_the_desk_terms() {
    let context = include_str!("../../../../CONTEXT.md");
    for term in ["**Stage / viewport**", "**Fence**", "**Note**", "**Card**"] {
        assert!(context.contains(term), "CONTEXT.md must define {term}");
    }
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
fn a_console_name_round_trips_through_the_wire_and_desk_toml() {
    let mut named = record("w1", 1);
    named.console_name = Some("fincal #1 ação 🚀".into());
    let json = serde_json::to_string(&named).unwrap();
    assert!(
        json.contains(r#""consoleName":"fincal #1 ação 🚀""#),
        "json={json}"
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desk.toml");
    let store = DeskStore {
        windows: vec![named, record("w2", 2)],
        ..Default::default()
    };
    save_to(&store, &path).unwrap();
    let back = load_from(&path).expect("desk reads");
    assert_eq!(back, store);
    assert_eq!(
        back.windows[0].console_name.as_deref(),
        Some("fincal #1 ação 🚀")
    );
}

/// ADR-0066 §3: the input's `maxlength` can be bypassed, so the store cuts
/// by `char` on both merge paths. A byte cut would split `é` and panic.
#[test]
fn merge_cuts_a_console_name_to_40_chars() {
    let long = || {
        let mut r = record("a", 5);
        r.console_name = Some("é".repeat(41));
        r
    };
    let out = merge(
        DeskStore::default(),
        upload(vec![long()], vec![], Some(DeskRemoved::default())),
    );
    assert_eq!(out.windows[0].console_name, Some("é".repeat(40)));
    let out = merge(DeskStore::default(), upload(vec![long()], vec![], None));
    assert_eq!(out.windows[0].console_name, Some("é".repeat(40)));
    let mut short = record("b", 5);
    short.console_name = Some("é".repeat(40));
    let out = merge(
        DeskStore::default(),
        upload(vec![short], vec![], Some(DeskRemoved::default())),
    );
    assert_eq!(out.windows[0].console_name, Some("é".repeat(40)));
}

/// ADR-0066 §2: a tab running a shell older than the name uploads records
/// without one and with a newer `ts`. The stored name stays; every other field
/// of the winning record still wins.
#[test]
fn merge_keeps_a_stored_console_name_when_the_winner_has_none() {
    let stored = || {
        let mut r = record("a", 1);
        r.console_name = Some("backend".into());
        DeskStore {
            windows: vec![r],
            ..Default::default()
        }
    };
    let mut old_shell = record("a", 2);
    old_shell.locked = true;
    old_shell.checkout = Some("wt-a".into());
    old_shell.rect.left = 99.0;
    let out = merge(
        stored(),
        upload(vec![old_shell], vec![], Some(DeskRemoved::default())),
    );
    let w = &out.windows[0];
    assert_eq!(w.console_name.as_deref(), Some("backend"));
    assert!(w.locked);
    assert_eq!(w.checkout.as_deref(), Some("wt-a"));
    assert_eq!(w.rect.left, 99.0);
    assert_eq!(w.ts, 2);

    let mut renamed = record("a", 2);
    renamed.console_name = Some("other".into());
    let out = merge(
        stored(),
        upload(vec![renamed], vec![], Some(DeskRemoved::default())),
    );
    assert_eq!(out.windows[0].console_name.as_deref(), Some("other"));

    let mut blank = record("a", 2);
    blank.console_name = Some("   ".into());
    let out = merge(
        stored(),
        upload(vec![blank], vec![], Some(DeskRemoved::default())),
    );
    assert_eq!(
        out.windows[0].console_name.as_deref(),
        Some("backend"),
        "a blank name is no name"
    );

    let out = merge(
        stored(),
        upload(
            vec![],
            vec![],
            Some(DeskRemoved {
                windows: vec!["a".into()],
                ..Default::default()
            }),
        ),
    );
    assert!(out.windows.is_empty(), "a retired record is gone");
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
        notes: Vec::new(),
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
        notes: vec![],
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
            notes: vec![],
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
        notes: vec![],
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
    let legacy: DeskUpload = serde_json::from_str(r#"{"windows":[],"fences":[],"notes":[]}"#)
        .expect("the pre-amendment shape parses");
    assert!(legacy.removed.is_none());
    assert!(serde_json::from_str::<DeskUpload>("[[],[]]").is_err());
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

/// The card record is placement, and the placement survives the file: colour
/// and text are the note's, so what round-trips here is a rect, a lock and the
/// `(checkout, path)` identity.
#[test]
fn a_note_card_round_trips_its_placement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desk.toml");
    let mut held = note("n2", "docs/plan.note", 2);
    held.checkout = Some("wt-a".into());
    held.locked = true;
    let store = DeskStore {
        windows: vec![record("w1", 1)],
        fences: vec![fence("f1", "backend", 1)],
        notes: vec![note("n1", ".ralphy/notes/standup.note", 1), held],
        checkouts: BTreeMap::new(),
    };
    save_to(&store, &path).unwrap();
    assert_eq!(load_from(&path).expect("desk reads"), store);

    // The TOML keeps `[[notes]]` between the fences and the `[checkouts]`
    // table — the ordering rule the store's doc comment states.
    let text = std::fs::read_to_string(&path).unwrap();
    let (fences_at, notes_at) = (
        text.find("[[fences]]").expect("fences table"),
        text.find("[[notes]]").expect("notes table"),
    );
    assert!(fences_at < notes_at, "{text}");
    // `false`/`None` stay off the page, as they do for a window.
    assert!(!text.contains("locked = false"), "{text}");
    assert_eq!(text.matches("checkout = ").count(), 1, "{text}");
}

#[test]
fn prune_notes_keeps_the_newest_cap_in_layout_order() {
    let notes: Vec<DeskNote> = (1..=NOTE_MAX as i64 + 1)
        .map(|n| note(&format!("n{n}"), &format!("a{n}.note"), n))
        .collect();
    let kept: Vec<String> = prune_notes(notes).into_iter().map(|n| n.id).collect();
    let expected: Vec<String> = (2..=NOTE_MAX as i64 + 1).map(|n| format!("n{n}")).collect();
    // The negative control is `n1`: an inverted or unsorted prune keeps it.
    assert_eq!(kept, expected, "the lowest-ts card is evicted");
    assert_eq!(kept.len(), NOTE_MAX);
}

/// Notes fold exactly like windows and fences: the newer `ts` wins per id, a
/// retired id is dropped whatever the store holds, and a card another page
/// owns survives an upload that never mentions it.
#[test]
fn merge_folds_notes_beside_the_other_two_collections() {
    let stored = DeskStore {
        windows: vec![],
        fences: vec![],
        notes: vec![
            note("n-closed", "gone.note", 99),
            note("n-other", "other.note", 5),
            note("n-moved", "moved.note", 1),
        ],
        checkouts: BTreeMap::new(),
    };
    let mut up = upload(
        vec![],
        vec![],
        Some(DeskRemoved {
            windows: vec![],
            fences: vec![],
            notes: vec!["n-closed".into()],
            checkouts: vec![],
        }),
    );
    let mut moved = note("n-moved", "moved.note", 7);
    moved.rect.left = 500.0;
    up.notes = vec![moved, note("n-new", "new.note", 8)];

    let out = merge(stored, up);
    let ids: Vec<&str> = out.notes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, vec!["n-moved", "n-new", "n-other"]);
    assert_eq!(out.notes[0].rect.left, 500.0, "the newer move wins");
}

/// A stale page's copy of a card another page just moved must not win.
#[test]
fn merge_keeps_the_newer_note_when_the_upload_is_stale() {
    let stored = DeskStore {
        notes: vec![note("n1", "a.note", 9)],
        ..DeskStore::default()
    };
    let mut up = upload(vec![], vec![], Some(DeskRemoved::default()));
    let mut stale = note("n1", "a.note", 2);
    stale.locked = true;
    up.notes = vec![stale];
    let out = merge(stored, up);
    assert_eq!(out.notes.len(), 1);
    assert!(!out.notes[0].locked, "the store's newer record won");
}

/// The ordering invariant the third array-of-tables rests on, proved with the
/// two collections that can collide: TOML emits values before tables, so a
/// `notes` field declared AFTER `checkouts` would make `to_string_pretty` fail
/// — and every desk save would fail at runtime, for any operator who has both
/// a note and a selected worktree, with the rest of this file still green.
#[test]
fn a_desk_with_both_a_note_and_a_checkout_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desk.toml");
    let mut held = fence("f2", "planning", 20);
    held.locked = true;
    let store = DeskStore {
        windows: vec![record("w1", 1), record("w2", 2)],
        fences: vec![fence("f1", "backend", 10), held],
        notes: vec![note("n1", ".ralphy/notes/a.note", 1)],
        // ADR-0063 §4: a repo key with a daemon prefix, beside a bare one.
        checkouts: BTreeMap::from([
            ("owner/repo".to_string(), "wt-a".to_string()),
            (
                "01ARZ3NDEKTSV4RRFFQ69G5FAW/owner/repo".to_string(),
                "wt-b".to_string(),
            ),
        ]),
    };
    save_to(&store, &path).expect("a desk with every collection must serialise");
    // Windows, fences (a locked one included), notes and checkouts all round
    // trip, and the `[checkouts]` table did not swallow a window.
    assert_eq!(load_from(&path).expect("desk reads"), store);
    let text = std::fs::read_to_string(&path).unwrap();
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle} in {text}"))
    };
    assert!(at("[[fences]]") < at("[[notes]]"), "{text}");
    assert!(at("[[notes]]") < at("[checkouts]"), "{text}");
}
