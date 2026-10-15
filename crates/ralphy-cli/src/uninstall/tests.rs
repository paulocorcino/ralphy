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

#[test]
fn purge_refuses_a_project_folder_and_the_home_folder() {
    let root = tempfile::tempdir().expect("scratch");
    let project = root.path().join("app");
    std::fs::create_dir_all(project.join(".git")).expect("repo");
    let runs = project.join(".ralphy");
    std::fs::create_dir_all(&runs).expect("runs");
    std::fs::write(runs.join("run.log"), "run").expect("run");

    let err = purge(&runs, true).expect_err("a project's .ralphy is refused");
    assert!(err.to_string().contains("of a project"), "{err}");
    assert!(runs.join("run.log").is_file(), "nothing was deleted");

    assert_eq!(
        purge_refusal(root.path(), Some(root.path())),
        Some("it is the home folder")
    );
    assert_eq!(purge_refusal(&root.path().join(".ralphy"), None), None);
}

#[test]
fn the_binary_and_its_parked_copies_go_and_nothing_else() {
    let dir = tempfile::tempdir().expect("scratch");
    let entry = dir.path().join(binary_name());
    for name in [
        binary_name().to_string(),
        format!("{}.old", binary_name()),
        format!("{}.old.2", binary_name()),
        format!("{}.older", binary_name()),
        "ralphy-notes".to_string(),
    ] {
        std::fs::write(dir.path().join(name), "x").expect("seed");
    }

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
