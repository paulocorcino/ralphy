use super::*;

fn write(root: &Path, rel: &str, bytes: usize) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("a file has a parent")).unwrap();
    std::fs::write(path, vec![b'x'; bytes]).unwrap();
}

fn measure(root: &Path, rel: &str) -> Result<Size, SizeError> {
    size(root, rel, SizeBudget::default(), &Slot::new())
}

#[test]
fn it_counts_files_folders_and_bytes() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.txt", 3);
    write(dir.path(), "sub/b.txt", 5);
    write(dir.path(), "sub/deeper/c.txt", 7);

    assert_eq!(
        measure(dir.path(), "").unwrap(),
        Size {
            bytes: 15,
            files: 3,
            dirs: 2,
            ignored_bytes: 0,
            truncated: false,
        }
    );
    // A folder below the root counts only what is inside it.
    let sub = measure(dir.path(), "sub").unwrap();
    assert_eq!((sub.bytes, sub.files, sub.dirs), (12, 2, 1));
}

#[test]
fn it_counts_what_the_tree_does_not_list() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/a.rs", 1);
    for hidden in super::super::HARD_EXCLUDE {
        write(dir.path(), &format!("{hidden}/blob"), 100);
    }

    let got = measure(dir.path(), "").unwrap();

    // The control is `src/a.rs`: 1 byte. The rest is what the listing drops.
    assert_eq!((got.bytes, got.files), (301, 4));
}

#[test]
fn ignored_bytes_follow_the_rules_of_the_tree() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".gitignore"), "build/\n*.log\n").unwrap();
    write(dir.path(), "build/out.bin", 100);
    // Git never looks inside an ignored folder, so its own rules do not count.
    std::fs::write(dir.path().join("build/.gitignore"), "!out.bin\n").unwrap();
    write(dir.path(), "run.log", 10);
    write(dir.path(), "kept.txt", 1);
    // A deeper `.gitignore` takes a file back; the pattern above still holds
    // for the file next to it.
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub/.gitignore"), "!keep.log\n").unwrap();
    write(dir.path(), "sub/keep.log", 7);
    write(dir.path(), "sub/other.log", 4);

    let got = measure(dir.path(), "").unwrap();

    // build/out.bin 100 + build/.gitignore 9 (it is inside an ignored folder
    // too) + run.log 10 + sub/other.log 4. `sub/keep.log` is taken back.
    assert_eq!(got.ignored_bytes, 123);
    assert!(got.bytes > got.ignored_bytes, "the rest is not ignored");
}

#[test]
fn a_folder_below_the_root_is_judged_from_the_root_rules() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".gitignore"), "sub/ign/\n").unwrap();
    write(dir.path(), "sub/ign/a", 10);
    write(dir.path(), "sub/b", 3);

    let sub = measure(dir.path(), "sub").unwrap();
    let ign = measure(dir.path(), "sub/ign").unwrap();

    assert_eq!(sub.ignored_bytes, 10);
    // The folder itself is ignored: everything in it is.
    assert_eq!((ign.bytes, ign.ignored_bytes), (10, 10));
}

#[test]
fn a_link_counts_as_one_item_and_is_not_followed() {
    let outside = tempfile::tempdir().unwrap();
    write(outside.path(), "big.bin", 5_000);
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.txt", 1);
    let link = dir.path().join("link");
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(outside.path(), &link);
    #[cfg(not(windows))]
    let linked = std::os::unix::fs::symlink(outside.path(), &link);
    if let Err(e) = linked {
        // A Windows host without the symlink privilege cannot stage the link;
        // the Linux leg of CI does. Say so rather than pass vacuously.
        eprintln!("SKIPPED: cannot create a directory symlink here: {e}");
        return;
    }

    let got = measure(dir.path(), "").unwrap();

    assert!(got.bytes < 5_000, "the target was followed: {got:?}");
    // The control: the link itself is an item beside `a.txt`.
    assert_eq!(got.files + got.dirs, 2, "{got:?}");
}

#[test]
fn the_entry_budget_stops_the_walk_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    for n in 0..20 {
        write(dir.path(), &format!("f{n}.txt"), 1);
    }
    let budget = SizeBudget {
        max_entries: 5,
        ..SizeBudget::default()
    };

    let cut = size(dir.path(), "", budget, &Slot::new()).unwrap();

    assert!(cut.truncated);
    assert_eq!(cut.files + cut.dirs, 5);
    // The control: the default budget reads all of it.
    let whole = measure(dir.path(), "").unwrap();
    assert!(!whole.truncated);
    assert_eq!(whole.files, 20);
}

#[test]
fn the_deadline_stops_the_walk_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.txt", 1);
    let budget = SizeBudget {
        deadline: Duration::ZERO,
        ..SizeBudget::default()
    };

    let cut = size(dir.path(), "", budget, &Slot::new()).unwrap();

    assert!(cut.truncated);
    assert_eq!(cut.files, 0);
}

#[test]
fn a_second_walk_is_busy_and_the_slot_is_given_back() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.txt", 1);
    let slot = Slot::new();

    let held = slot.take().expect("a free slot is taken");
    assert_eq!(
        size(dir.path(), "", SizeBudget::default(), &slot),
        Err(SizeError::Busy)
    );
    drop(held);

    // Free again: a walk runs, and gives the slot back at its end.
    assert!(size(dir.path(), "", SizeBudget::default(), &slot).is_ok());
    // A refusal gives it back too.
    assert_eq!(
        size(dir.path(), "nope", SizeBudget::default(), &slot),
        Err(SizeError::NotFound)
    );
    assert!(size(dir.path(), "", SizeBudget::default(), &slot).is_ok());
}

#[test]
fn a_miss_a_file_and_an_escape_are_not_found() {
    let outer = tempfile::tempdir().unwrap();
    write(outer.path(), "secret/key", 1);
    let root = outer.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    write(&root, "a.txt", 1);

    assert_eq!(measure(&root, "nope"), Err(SizeError::NotFound));
    // A file has a stat, not a size.
    assert_eq!(measure(&root, "a.txt"), Err(SizeError::NotFound));
    // `secret` is a real folder outside the root: only confinement refuses it.
    assert!(outer.path().join("secret").is_dir());
    assert_eq!(measure(&root, "../secret"), Err(SizeError::NotFound));
}

#[test]
fn refusal_reasons_are_the_wire_vocabulary() {
    assert_eq!(SizeError::Busy.reason(), "busy");
    assert_eq!(SizeError::NotFound.reason(), "not found");
}
