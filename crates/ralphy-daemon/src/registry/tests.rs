use super::*;

/// `root()` is the explorer's join base, so it must be absolute and PASTEABLE
/// — the `\\?\` prefix `canonicalize` returns on Windows is a Win32 escape
/// hatch no operator wants in a copied path.
#[test]
fn root_is_absolute_and_carries_no_verbatim_prefix() {
    let dir = tempfile::tempdir().unwrap();
    // FORWARD-slashed, because that is the shape `path` actually stores (git's
    // `--show-toplevel` output) and the reason `root()` exists at all. A test
    // fed native separators would never exercise the conversion.
    let entry = RepoEntry {
        path: dir.path().to_string_lossy().replace('\\', "/"),
        ..RepoEntry::default()
    };
    let root = entry.root().expect("a live tempdir canonicalizes");
    assert!(Path::new(&root).is_absolute(), "absolute: {root}");
    assert!(!root.starts_with(r"\\?\"), "no verbatim prefix: {root}");
    assert_eq!(
        std::fs::canonicalize(&root).unwrap(),
        std::fs::canonicalize(dir.path()).unwrap(),
        "still names the same directory"
    );
    // An unreachable repo has no root rather than a fabricated one.
    let gone = RepoEntry {
        path: dir.path().join("nope").to_string_lossy().into_owned(),
        ..RepoEntry::default()
    };
    assert_eq!(gone.root(), None);
}

#[test]
fn upsert_self_heals_same_slug_new_path() {
    let mut store = RegistryStore::default();
    store.upsert("owner/repo", "/old");
    store.upsert("owner/repo", "/new");
    assert_eq!(store.repos.len(), 1, "same slug must not duplicate");
    assert_eq!(store.entry("owner/repo").unwrap().path, "/new");
}

/// The knob is retired (ADR-0063 §3): an old store still loads, the slugs
/// carrying the key are listed for the one startup notice, and the next
/// write drops it. Presence is what is retired, not `true` — `false` lists.
#[test]
fn a_retired_console_worktree_key_loads_and_is_not_written_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("repos.toml");
    std::fs::write(
        &path,
        "[repos.\"owner/repo\"]\npath = \"/old\"\nconsole_worktree = true\n\n[repos.\"owner/plain\"]\npath = \"/plain\"\n",
    )
    .unwrap();

    let store = load_from(&path).unwrap();
    assert_eq!(store.entry("owner/repo").unwrap().path, "/old");
    assert_eq!(store.retired_console_worktree(), vec!["owner/repo"]);

    save_to(&store, &path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        text.matches("console_worktree").count(),
        0,
        "the retired key is never written back: {text}"
    );
    let back = load_from(&path).unwrap();
    assert!(back.retired_console_worktree().is_empty());
    assert_eq!(back.entry("owner/repo").unwrap().path, "/old");

    let mut store = RegistryStore::default();
    store.upsert("owner/off", "/off");
    store
        .repos
        .get_mut("owner/off")
        .expect("the slug was just registered")
        .console_worktree = Some(false);
    assert_eq!(store.retired_console_worktree(), vec!["owner/off"]);
    store.upsert("owner/off", "/moved");
    assert!(
        store.retired_console_worktree().is_empty(),
        "an upsert does not carry the retired key over"
    );
}

#[test]
fn remove_is_idempotent() {
    let mut store = RegistryStore::default();
    store.upsert("owner/repo", "/some");
    assert!(store.remove("owner/repo"), "first remove reports true");
    assert!(!store.remove("owner/repo"), "second remove reports false");
    assert!(store.entry("owner/repo").is_none());
}

#[test]
fn unreachable_entry_retained_and_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("repos.toml");
    let mut store = RegistryStore::default();
    store.upsert("owner/gone", "/no/such/path/exists");
    store.upsert("owner/here", &dir.path().to_string_lossy());
    save_to(&store, &path).unwrap();

    let back = load_from(&path).unwrap();
    assert!(
        back.entry("owner/gone").is_some(),
        "an unreachable entry is retained, never removed"
    );
    assert!(!back.entry("owner/gone").unwrap().reachable());
    assert!(back.entry("owner/here").unwrap().reachable());
}

#[test]
fn head_branch_reads_ref() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    std::fs::write(
        dir.path().join(".git").join("HEAD"),
        "ref: refs/heads/feat/x\n",
    )
    .unwrap();
    let entry = RepoEntry {
        path: dir.path().to_string_lossy().to_string(),
        ..RepoEntry::default()
    };
    assert_eq!(entry.head_branch(), Some("feat/x".to_string()));
}

#[test]
fn head_branch_none_when_detached_or_missing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    std::fs::write(
        dir.path().join(".git").join("HEAD"),
        "1234567890abcdef1234567890abcdef12345678\n",
    )
    .unwrap();
    let entry = RepoEntry {
        path: dir.path().to_string_lossy().to_string(),
        ..RepoEntry::default()
    };
    assert_eq!(entry.head_branch(), None, "detached HEAD yields None");

    let no_git = tempfile::tempdir().unwrap();
    let entry = RepoEntry {
        path: no_git.path().to_string_lossy().to_string(),
        ..RepoEntry::default()
    };
    assert_eq!(entry.head_branch(), None, "missing .git yields None");
}

fn git_init(dir: &Path) {
    std::process::Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "init"])
        .output()
        .expect("git init (CI and the build machine have git)");
}

#[test]
fn dirty_true_on_untracked_false_on_clean() {
    let clean = tempfile::tempdir().unwrap();
    git_init(clean.path());
    let clean_entry = RepoEntry {
        path: clean.path().to_string_lossy().to_string(),
        ..RepoEntry::default()
    };
    assert!(!clean_entry.dirty(), "a freshly git-inited repo is clean");

    let dirty = tempfile::tempdir().unwrap();
    git_init(dirty.path());
    std::fs::write(dirty.path().join("untracked.txt"), "x").unwrap();
    let dirty_entry = RepoEntry {
        path: dirty.path().to_string_lossy().to_string(),
        ..RepoEntry::default()
    };
    assert!(
        dirty_entry.dirty(),
        "an untracked file makes the tree dirty"
    );

    let no_git = tempfile::tempdir().unwrap();
    let entry = RepoEntry {
        path: no_git.path().to_string_lossy().to_string(),
        ..RepoEntry::default()
    };
    assert!(!entry.dirty(), "a non-git dir reports not-dirty");
}

#[test]
fn remote_some_with_origin_none_without() {
    let with = tempfile::tempdir().unwrap();
    git_init(with.path());
    std::process::Command::new("git")
        .args([
            "-C",
            &with.path().to_string_lossy(),
            "remote",
            "add",
            "origin",
            "https://github.com/o/r.git",
        ])
        .output()
        .unwrap();
    let with_entry = RepoEntry {
        path: with.path().to_string_lossy().to_string(),
        ..RepoEntry::default()
    };
    assert_eq!(
        with_entry.remote(),
        Some("https://github.com/o/r.git".to_string())
    );

    let without = tempfile::tempdir().unwrap();
    git_init(without.path());
    let without_entry = RepoEntry {
        path: without.path().to_string_lossy().to_string(),
        ..RepoEntry::default()
    };
    assert_eq!(without_entry.remote(), None, "no origin → None");

    let no_git = tempfile::tempdir().unwrap();
    let entry = RepoEntry {
        path: no_git.path().to_string_lossy().to_string(),
        ..RepoEntry::default()
    };
    assert_eq!(entry.remote(), None, "a non-git dir has no remote");
}

#[test]
fn slug_with_slash_round_trips_through_toml() {
    let mut store = RegistryStore::default();
    store.upsert("owner/repo", "/somewhere");
    let text = toml::to_string_pretty(&store).unwrap();
    let back: RegistryStore = toml::from_str(&text).unwrap();
    assert_eq!(back.entry("owner/repo").unwrap().path, "/somewhere");
}

#[test]
fn slugs_for_path_matches_canonical_and_string_forms() {
    let dir = tempfile::tempdir().unwrap();
    let native = dir.path().to_string_lossy().into_owned();
    // The forward-slashed spelling git's `--show-toplevel` stores, plus a
    // trailing separator — both must still name the live directory.
    let slashed = format!("{}/", native.replace('\\', "/"));
    let mut store = RegistryStore::default();
    store.upsert("path-abc", &slashed);
    store.upsert("owner/repo", &native);
    store.upsert("owner/other", "/elsewhere");
    assert_eq!(
        store.slugs_for_path(dir.path()),
        vec!["owner/repo".to_string(), "path-abc".to_string()],
        "both keys name the same live directory, in key order"
    );

    // Unreachable on both sides: the strings decide, normalized.
    let mut gone = RegistryStore::default();
    gone.upsert("path-gone", "C:/no/such/dir/");
    assert_eq!(
        gone.slugs_for_path(Path::new(r"C:\no\such\dir")),
        vec!["path-gone".to_string()]
    );
    assert!(gone.slugs_for_path(Path::new("/no/such/other")).is_empty());
}

#[test]
fn rekey_moves_entry_and_records_former_slug() {
    let mut store = RegistryStore::default();
    store.upsert("path-abc", "/repo");
    assert!(store.rekey("path-abc", "owner/repo"));
    assert!(store.entry("path-abc").is_none(), "the old key is gone");
    let entry = store.entry("owner/repo").expect("moved under the new key");
    assert_eq!(entry.path, "/repo");
    assert_eq!(entry.former_slugs, vec!["path-abc".to_string()]);
    assert_eq!(
        store.former_slug_map().get("path-abc").map(String::as_str),
        Some("owner/repo")
    );

    assert!(
        !store.rekey("path-abc", "owner/repo"),
        "absent source moves nothing"
    );
    assert!(
        !store.rekey("owner/repo", "owner/repo"),
        "same key moves nothing"
    );
}

#[test]
fn rekey_into_an_existing_slug_merges_and_never_lists_itself() {
    let mut store = RegistryStore::default();
    store.upsert("path-abc", "/repo-old-spelling");
    store.upsert("owner/repo", "/repo");
    store
        .repos
        .get_mut("path-abc")
        .unwrap()
        .former_slugs
        .push("path-older".into());
    assert!(store.rekey("path-abc", "owner/repo"));
    assert_eq!(store.repos.len(), 1);
    let entry = store.entry("owner/repo").unwrap();
    assert_eq!(
        entry.path, "/repo",
        "the existing target keeps its own path"
    );
    assert_eq!(
        entry.former_slugs,
        vec!["path-older".to_string(), "path-abc".to_string()],
        "history is absorbed, oldest first"
    );

    // A forge rename A → B: B lists A, never B itself.
    let mut renamed = RegistryStore::default();
    renamed.upsert("old/name", "/repo");
    renamed.rekey("old/name", "new/name");
    let entry = renamed.entry("new/name").unwrap();
    assert!(!entry.former_slugs.contains(&"new/name".to_string()));
    assert_eq!(entry.former_slugs, vec!["old/name".to_string()]);
}

#[test]
fn upsert_preserves_former_slugs() {
    let mut store = RegistryStore::default();
    store.upsert("path-abc", "/repo");
    store.rekey("path-abc", "owner/repo");
    // The passive registration on every `ralphy run` is an upsert of the same
    // key: it must not wipe the rename history the desk routes read.
    store.upsert("owner/repo", "/repo");
    assert_eq!(
        store.entry("owner/repo").unwrap().former_slugs,
        vec!["path-abc".to_string()]
    );
}

#[test]
fn former_slugs_round_trip_and_are_omitted_when_empty() {
    let mut store = RegistryStore::default();
    store.upsert("owner/plain", "/plain");
    store.upsert("path-abc", "/repo");
    store.rekey("path-abc", "owner/repo");
    let text = toml::to_string_pretty(&store).unwrap();
    assert_eq!(
        text.matches("former_slugs").count(),
        1,
        "only the migrated entry carries the key: {text}"
    );
    let back: RegistryStore = toml::from_str(&text).unwrap();
    assert_eq!(back, store);
    // An older store without the key still loads.
    let old: RegistryStore = toml::from_str("[repos.\"o/r\"]\npath = \"/x\"\n").unwrap();
    assert!(old.entry("o/r").unwrap().former_slugs.is_empty());
}
