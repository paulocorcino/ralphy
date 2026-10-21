use super::*;

#[test]
fn move_aside_never_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desk.toml");
    let earlier = dir.path().join("desk.toml.unreadable-2026-01-02");
    std::fs::write(&earlier, "first").unwrap();
    std::fs::write(&path, "second").unwrap();
    let moved = move_aside(&path, "2026-01-02").expect("the rename succeeds");
    assert!(
        moved.to_string_lossy().ends_with("unreadable-2026-01-02-2"),
        "a taken name gets a counter: {}",
        moved.display()
    );
    assert_eq!(std::fs::read_to_string(&earlier).unwrap(), "first");
    assert_eq!(std::fs::read_to_string(&moved).unwrap(), "second");
    assert!(!path.exists(), "the desk itself moved");
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
fn a_failed_save_leaves_the_previous_desk_intact() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desk.toml");
    let good = DeskStore {
        rev: 0,
        generation: 0,
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
            rev: 0,
            generation: 0,
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
            rev: 0,
            generation: 0,
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
            rev: 0,
            generation: 0,
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
