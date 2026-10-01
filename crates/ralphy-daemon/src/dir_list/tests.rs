use std::path::{Path, MAIN_SEPARATOR};

use super::*;

fn mkdirs(root: &Path, names: &[&str]) {
    for n in names {
        std::fs::create_dir_all(root.join(n)).expect("mkdir");
    }
}

/// The typed text that lists `dir` itself: its path and a trailing separator.
fn inside(dir: &Path) -> String {
    format!("{}{MAIN_SEPARATOR}", dir.display())
}

fn names(listing: &Listing) -> Vec<&str> {
    listing.entries.iter().map(|e| e.name.as_str()).collect()
}

fn none() -> RegistryStore {
    RegistryStore::default()
}

fn registry_of(paths: &[&Path]) -> RegistryStore {
    let mut store = RegistryStore::default();
    for (i, p) in paths.iter().enumerate() {
        store.upsert(&format!("o/r{i}"), &p.to_string_lossy());
    }
    store
}

#[test]
fn lists_one_level_of_directories_and_never_a_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    mkdirs(tmp.path(), &["a/inner", "b"]);
    std::fs::write(tmp.path().join("file.txt"), "x").expect("write");

    let got = list(Some(&inside(tmp.path())), &none(), None).expect("list");

    assert_eq!(names(&got), ["a", "b"]);
    assert_eq!(got.more, 0);
}

#[test]
fn returns_at_most_the_limit_and_counts_the_rest() {
    let tmp = tempfile::tempdir().expect("tempdir");
    for i in 0..LIMIT + 5 {
        std::fs::create_dir(tmp.path().join(format!("d{i:03}"))).expect("mkdir");
    }

    let got = list(Some(&inside(tmp.path())), &none(), None).expect("list");

    assert_eq!(got.entries.len(), LIMIT);
    assert_eq!(got.more, 5);
    assert_eq!(got.entries[0].name, "d000", "sorted before the cut");
}

#[test]
fn filters_by_the_typed_prefix_without_regard_to_case() {
    let tmp = tempfile::tempdir().expect("tempdir");
    mkdirs(tmp.path(), &["Alpha", "alpine", "beta"]);

    let text = format!("{}AL", inside(tmp.path()));
    let got = list(Some(&text), &none(), None).expect("list");

    assert_eq!(names(&got), ["Alpha", "alpine"]);
}

#[test]
fn leaves_out_dot_folders_unless_the_prefix_starts_with_a_dot() {
    let tmp = tempfile::tempdir().expect("tempdir");
    mkdirs(tmp.path(), &[".cache", "src"]);

    let plain = list(Some(&inside(tmp.path())), &none(), None).expect("list");
    assert_eq!(names(&plain), ["src"]);

    let dotted = format!("{}.", inside(tmp.path()));
    let got = list(Some(&dotted), &none(), None).expect("list");
    assert_eq!(names(&got), [".cache"]);
}

#[cfg(windows)]
#[test]
fn leaves_out_a_hidden_folder_unless_its_name_is_typed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    mkdirs(tmp.path(), &["Secret", "src"]);
    let status = std::process::Command::new("attrib")
        .arg("+h")
        .arg(tmp.path().join("Secret"))
        .status()
        .expect("attrib");
    assert!(status.success());

    let plain = list(Some(&inside(tmp.path())), &none(), None).expect("list");
    assert_eq!(names(&plain), ["src"]);

    let typed = format!("{}secret", inside(tmp.path()));
    let got = list(Some(&typed), &none(), None).expect("list");
    assert_eq!(names(&got), ["Secret"]);
}

#[test]
fn marks_a_repo_by_a_git_directory_or_a_git_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    mkdirs(tmp.path(), &["main/.git", "worktree", "plain"]);
    std::fs::write(tmp.path().join("worktree/.git"), "gitdir: elsewhere").expect("write");

    let got = list(Some(&inside(tmp.path())), &none(), None).expect("list");

    let repo: Vec<(&str, bool)> = got
        .entries
        .iter()
        .map(|e| (e.name.as_str(), e.repo))
        .collect();
    assert_eq!(repo, [("main", true), ("plain", false), ("worktree", true)]);
}

#[test]
fn marks_a_folder_the_registry_holds_as_added() {
    let tmp = tempfile::tempdir().expect("tempdir");
    mkdirs(tmp.path(), &["one", "two"]);
    let registry = registry_of(&[&tmp.path().join("two")]);

    let got = list(Some(&inside(tmp.path())), &registry, None).expect("list");

    let added: Vec<(&str, bool)> = got
        .entries
        .iter()
        .map(|e| (e.name.as_str(), e.added))
        .collect();
    assert_eq!(added, [("one", false), ("two", true)]);
}

#[test]
fn describes_the_listed_folder_with_its_repo_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    mkdirs(tmp.path(), &["proj/.git", "proj/src/deep", "loose"]);
    let proj = tmp.path().join("proj");
    let registry = registry_of(&[&proj]);

    let sub = list(Some(&inside(&proj.join("src"))), &registry, None).expect("list");
    let root = std::fs::canonicalize(&proj).expect("canon");
    assert_eq!(sub.dir.root.as_deref(), Some(plain(&root).as_str()));
    assert!(sub.dir.added, "the root of the listed folder is registered");

    let loose = list(Some(&inside(&tmp.path().join("loose"))), &registry, None).expect("list");
    assert_eq!(loose.dir.root, None);
    assert!(!loose.dir.added);
}

#[test]
fn start_is_the_parent_shared_by_most_projects() {
    let tmp = tempfile::tempdir().expect("tempdir");
    mkdirs(tmp.path(), &["dev/a", "dev/b", "other/c"]);
    let registry = registry_of(&[
        &tmp.path().join("dev/a"),
        &tmp.path().join("dev/b"),
        &tmp.path().join("other/c"),
    ]);

    let got = list(None, &registry, None).expect("list");

    let dev = std::fs::canonicalize(tmp.path().join("dev")).expect("canon");
    assert_eq!(got.start.as_deref(), Some(plain(&dev).as_str()));
    assert_eq!(names(&got), ["a", "b"], "the start folder is listed too");
}

#[test]
fn start_is_home_when_nothing_is_registered() {
    let home = tempfile::tempdir().expect("tempdir");
    mkdirs(home.path(), &["code"]);

    let got = list(None, &none(), Some(home.path())).expect("list");

    assert_eq!(
        got.start.as_deref(),
        Some(home.path().to_string_lossy().as_ref())
    );
    assert_eq!(names(&got), ["code"]);
}

#[test]
fn expands_a_leading_tilde_to_home() {
    let home = tempfile::tempdir().expect("tempdir");
    mkdirs(home.path(), &["code", "docs"]);

    let got = list(
        Some(&format!("~{MAIN_SEPARATOR}co")),
        &none(),
        Some(home.path()),
    )
    .expect("list");

    assert_eq!(names(&got), ["code"]);
}

#[test]
fn refuses_a_relative_path() {
    assert_eq!(
        list(Some("some/where/"), &none(), None),
        Err(Refusal::Relative)
    );
}

#[test]
fn refuses_a_long_path_or_a_control_character() {
    let long = format!("/{}", "a".repeat(MAX_PATH_BYTES));
    assert_eq!(list(Some(&long), &none(), None), Err(Refusal::TooLong));
    assert_eq!(
        list(Some("/tmp/\u{7}/"), &none(), None),
        Err(Refusal::ControlCharacter)
    );
}

#[test]
fn a_missing_folder_is_refused_as_not_found() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let text = inside(&tmp.path().join("missing"));
    assert_eq!(list(Some(&text), &none(), None), Err(Refusal::NotFound));
}

#[cfg(windows)]
#[test]
fn refuses_a_unc_path() {
    for text in [
        r"\\server\share\",
        r"\\?\UNC\server\share\",
        r"//server/share/",
    ] {
        assert_eq!(
            list(Some(text), &none(), None),
            Err(Refusal::Network),
            "{text}"
        );
    }
    assert!(is_network_path(Path::new(r"\\wsl.localhost\Ubuntu\home")));
    assert!(!is_network_path(Path::new(r"C:\Dev")));
}

#[cfg(windows)]
#[test]
fn an_empty_path_lists_the_drives() {
    let got = list(Some(""), &none(), None).expect("list");
    let system = std::env::var("SystemDrive").expect("SystemDrive");
    assert!(names(&got).contains(&system.as_str()), "{:?}", names(&got));
}

#[cfg(windows)]
#[test]
fn a_bare_drive_letter_lists_that_drive() {
    let system = std::env::var("SystemDrive").expect("SystemDrive");
    let got = list(Some(&system), &none(), None).expect("list");
    assert_eq!(got.dir.path, format!("{system}\\"));
    assert!(names(&got).contains(&"Windows"), "{:?}", names(&got));
}

#[cfg(unix)]
#[test]
fn follows_a_symlink_to_a_folder() {
    let tmp = tempfile::tempdir().expect("tempdir");
    mkdirs(tmp.path(), &["real"]);
    std::os::unix::fs::symlink(tmp.path().join("real"), tmp.path().join("link")).expect("link");

    let got = list(Some(&inside(tmp.path())), &none(), None).expect("list");

    assert_eq!(names(&got), ["link", "real"]);
}

#[cfg(unix)]
#[test]
fn an_unreadable_folder_is_an_entry_error() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().expect("tempdir");
    mkdirs(tmp.path(), &["locked", "open"]);
    let locked = tmp.path().join("locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    // Root reads any folder; the case does not exist for it.
    let readable_anyway = std::fs::read_dir(&locked).is_ok();

    let got = list(Some(&inside(tmp.path())), &none(), None);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).expect("chmod back");
    if readable_anyway {
        return;
    }

    let got = got.expect("the listing itself succeeds");
    assert_eq!(names(&got), ["locked", "open"]);
    assert_eq!(
        got.entries[0].error.as_deref(),
        Some(Refusal::Unreadable.message())
    );
    assert_eq!(got.entries[1].error, None);
}
