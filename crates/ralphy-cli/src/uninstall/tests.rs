use super::*;

#[test]
fn purge_deletes_the_store_and_nothing_beside_it() {
    let home = tempfile::tempdir().expect("scratch");
    let store = home.path().join(".ralphy");
    std::fs::create_dir_all(store.join("peers")).expect("store");
    std::fs::write(store.join("daemon.log"), "log").expect("log");
    std::fs::write(store.join("peers").join("a.toml"), "peer").expect("peer");
    std::fs::write(home.path().join("notes.txt"), "keep").expect("sibling");

    purge(&store, true).expect("purge");

    assert!(!store.exists());
    assert!(
        home.path().join("notes.txt").is_file(),
        "a file beside the store stays"
    );
}

/// A folder with one file only the daemon writes.
fn store_at(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).expect("store");
    std::fs::write(dir.join("daemon.toml"), "name = \"x\"").expect("marker");
    dir.to_path_buf()
}

#[test]
fn purge_refuses_every_folder_that_is_not_ralphys_own() {
    let root = tempfile::tempdir().expect("scratch");
    let root = std::fs::canonicalize(root.path()).expect("canonical");
    let home = root.join("home");
    let default_store = store_at(&home.join(".ralphy"));
    let project = store_at(&root.join("code").join("app"));
    std::fs::create_dir_all(project.join(".git")).expect("repo");
    let runs = store_at(&project.join(".ralphy"));
    let plain = root.join("photos");
    std::fs::create_dir_all(&plain).expect("plain");
    std::fs::write(plain.join("a.jpg"), "x").expect("photo");

    let refusal = |store: &Path| purge_refusal(store, Some(&home), Some(&default_store));
    assert_eq!(
        refusal(&home),
        Some("it is the home folder or a folder above it")
    );
    assert_eq!(
        refusal(&root),
        Some("it is the home folder or a folder above it")
    );
    assert_eq!(refusal(&project), Some("it is a project"));
    assert_eq!(
        refusal(&runs),
        Some("it is the `.ralphy` folder of a project")
    );
    assert_eq!(
        refusal(&plain),
        Some("it holds none of the files the daemon writes")
    );
    assert_eq!(refusal(Path::new("store")), Some("it is a relative path"));
    assert_eq!(refusal(&default_store), None);

    // A home folder kept in git (dotfiles) still lets its own store go.
    std::fs::create_dir_all(home.join(".git")).expect("dotfiles");
    assert_eq!(refusal(&default_store), None);

    let err = purge(&runs, true).expect_err("a project's .ralphy is refused");
    assert!(err.to_string().contains("of a project"), "{err}");
    assert!(runs.join("daemon.toml").is_file(), "nothing was deleted");
}

/// Windows compares paths without case, so a home spelled in other letters
/// is still the home folder.
#[cfg(windows)]
#[test]
fn purge_compares_windows_paths_without_case() {
    let home = Path::new(r"C:\Users\Me");
    assert_eq!(
        purge_refusal(Path::new(r"c:\users"), Some(home), None),
        Some("it is the home folder or a folder above it")
    );
}

#[test]
fn a_plain_uninstall_keeps_the_store() {
    let home = tempfile::tempdir().expect("scratch");
    let store = store_at(&home.path().join(".ralphy"));
    keep_or_purge(&store, false, true).expect("keep");
    assert!(store.join("daemon.toml").is_file());
}

#[test]
fn the_binary_and_its_parked_copies_go_and_nothing_else() {
    let dir = tempfile::tempdir().expect("scratch");
    let entry = dir.path().join(binary_name());
    for name in [
        format!("{}.old.2", binary_name()),
        format!("{}.older", binary_name()),
        "ralphy-notes".to_string(),
    ] {
        std::fs::write(dir.path().join(name), "x").expect("seed");
    }
    // `.old` is named by install's own parking, so the two cannot drift.
    std::fs::write(&entry, "x").expect("seed");
    crate::install::park_existing(&entry).expect("park");
    std::fs::write(&entry, "x").expect("seed");

    remove_binary(&entry).expect("remove");

    let mut left: Vec<String> = std::fs::read_dir(dir.path())
        .expect("read")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(
        left,
        [
            "ralphy-notes".to_string(),
            format!("{}.older", binary_name())
        ]
    );
}

#[test]
fn a_size_reads_in_kilobytes_or_megabytes() {
    assert_eq!(readable_size(0), "0 KB");
    assert_eq!(readable_size(1500), "2 KB");
    assert_eq!(readable_size(5 * 1024 * 1024 + 1024 * 512), "5.5 MB");
}

/// A link in the store to a file outside it: the purge removes the link, never
/// what it points at.
#[cfg(unix)]
#[test]
fn purge_does_not_follow_a_link_out_of_the_store() {
    let home = tempfile::tempdir().expect("scratch");
    let outside = home.path().join("keep.txt");
    std::fs::write(&outside, "keep").expect("outside");
    let store = store_at(&home.path().join(".ralphy"));
    std::os::unix::fs::symlink(&outside, store.join("link")).expect("link");
    std::os::unix::fs::symlink(home.path(), store.join("home-link")).expect("dir link");

    purge(&store, true).expect("purge");

    assert!(!store.exists());
    assert_eq!(
        std::fs::read_to_string(&outside).expect("still there"),
        "keep"
    );
}
