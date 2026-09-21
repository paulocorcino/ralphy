use super::*;
use crate::git::git;
use std::path::PathBuf;

fn entry(path: &str) -> Entry {
    Entry {
        path: path.to_string(),
        ..Entry::default()
    }
}

#[test]
fn parse_porcelain_reads_blank_line_separated_records() {
    let text = "worktree C:/r\nHEAD abc\nbranch refs/heads/main\n\n\
worktree C:/r/.ralphy/worktrees/wt-a\nHEAD abc\nbranch refs/heads/wt-a\n\n\
worktree C:/r/other\nHEAD abc\ndetached\n\n\
worktree C:/r/.ralphy/worktrees/gone\nHEAD abc\nbranch refs/heads/gone\n\
prunable gitdir file points to non-existent location\n\n\
worktree C:/r/.ralphy/worktrees/held\nHEAD abc\nbranch refs/heads/held\nlocked operator says so\n\n\
worktree C:/r/.ralphy/worktrees/bare-lock\nHEAD abc\nbranch refs/heads/bare-lock\nlocked\n\n";
    let entries = parse_porcelain(text);
    assert_eq!(entries.len(), 6, "six records: {entries:?}");
    assert!(
        entries[4].locked && entries[5].locked,
        "locked with and without a reason"
    );
    assert!(!entries[1].locked, "an unlocked record stays unlocked");
    assert_eq!(entries[0].path, "C:/r");
    assert_eq!(entries[0].branch, "main");
    assert_eq!(entries[1].branch, "wt-a");
    assert!(entries[2].detached && entries[2].branch.is_empty());
    assert!(!entries[2].prunable);
    assert!(entries[3].prunable);
    assert_eq!(entries[3].branch, "gone");

    // A last record without its closing blank line, and CRLF, both close.
    let tail = parse_porcelain("worktree C:/r\r\nHEAD abc\r\n\r\nworktree C:/r/x\r\nHEAD abc");
    assert_eq!(tail.len(), 2);
    assert_eq!(tail[1].path, "C:/r/x");
}

#[test]
fn select_workbench_keeps_only_single_segment_children() {
    let mut gone = entry("C:/r/.ralphy/worktrees/gone");
    gone.prunable = true;
    let entries = vec![
        entry("C:/r"),
        entry("C:/r/.ralphy/worktrees/wt-a"),
        entry("C:/r/.ralphy/worktrees/nested/x"),
        entry("C:/r/other"),
        gone,
        entry("C:/r/.ralphy/worktrees/"),
    ];
    let names: Vec<String> = select_workbench(&entries, "C:/r")
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, vec!["wt-a".to_string()]);
}

fn tmp(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ralphy-checkouts-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn configure(dir: &Path) {
    git(dir, &["config", "user.email", "t@example.com"]).unwrap();
    git(dir, &["config", "user.name", "Test"]).unwrap();
    git(dir, &["config", "core.autocrlf", "false"]).unwrap();
}

fn commit_file(dir: &Path, file: &str, body: &str, msg: &str) {
    std::fs::write(dir.join(file), body).unwrap();
    git(dir, &["add", "."]).unwrap();
    git(dir, &["commit", "-q", "-m", msg]).unwrap();
}

fn add_worktree(root: &Path, name: &str, path: &str) {
    git(
        root,
        &["worktree", "add", "--no-track", "-b", name, path, "main"],
    )
    .unwrap();
}

/// ONE fixture for the whole git-backed leg (the suite is spawn-bound):
/// `wt-a` dirty with a recorded base, `wt-b` clean with none, `wt-gone`
/// locked and then deleted from disk, and a worktree elsewhere that must
/// not be reported.
#[test]
fn list_reports_only_the_workbench_worktrees() {
    let root = tmp("list");
    let elsewhere = tmp("list-elsewhere");
    let _ = std::fs::remove_dir_all(&elsewhere);
    git(&root, &["init", "-q", "-b", "main"]).unwrap();
    configure(&root);
    commit_file(&root, "README.md", "hello\n", "init");
    commit_file(&root, ".gitignore", ".ralphy/\n", "ignore the run dir");
    std::fs::create_dir_all(root.join(WORKTREES_DIR)).unwrap();
    add_worktree(&root, "wt-a", &format!("{WORKTREES_DIR}/wt-a"));
    git(&root, &["config", "branch.wt-a.base", "main"]).unwrap();
    add_worktree(&root, "wt-b", &format!("{WORKTREES_DIR}/wt-b"));
    add_worktree(&root, "elsewhere", &elsewhere.to_string_lossy());
    // A locked worktree whose directory is gone: git still lists it, never
    // as `prunable`, and `git status` inside it would fail the listing.
    add_worktree(&root, "wt-gone", &format!("{WORKTREES_DIR}/wt-gone"));
    git(
        &root,
        &["worktree", "lock", &format!("{WORKTREES_DIR}/wt-gone")],
    )
    .unwrap();
    std::fs::remove_dir_all(root.join(WORKTREES_DIR).join("wt-gone")).unwrap();
    let wt_a_path = root.join(WORKTREES_DIR).join("wt-a");
    std::fs::write(wt_a_path.join("scratch.txt"), "dirty\n").unwrap();

    let listing = list(&root).unwrap();
    let names: Vec<&str> = listing.worktrees.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, vec!["wt-a", "wt-b"], "listing: {listing:?}");
    let a = &listing.worktrees[0];
    assert_eq!(a.branch, "wt-a");
    assert_eq!(a.base, "main");
    assert!(a.dirty, "wt-a holds an untracked scratch.txt");
    assert!(
        a.path.ends_with("/.ralphy/worktrees/wt-a"),
        "path in git's spelling: {}",
        a.path
    );
    let b = &listing.worktrees[1];
    assert_eq!(b.branch, "wt-b");
    assert_eq!(b.base, "", "no branch.wt-b.base was recorded");
    assert!(!b.dirty);
    assert!(!listing.worktrees.iter().any(|w| w.name == "elsewhere"));
    assert_eq!(
        listing.primary,
        git(&root, &["rev-parse", "--show-toplevel"]).unwrap()
    );

    // From inside a worktree git lists the same trees, main tree first.
    assert_eq!(list(&wt_a_path).unwrap(), listing);

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&elsewhere);
}

/// ONE fixture for carry-over: `.env` (ignored file) and `.vscode/`
/// (ignored dir) copied, `node_modules` (ignored dir) linked, and one of
/// each refusal — a tracked file, a missing path, a file in `share` —
/// each a warning, none a failure; the new worktree stays clean.
#[test]
fn carry_over_copies_and_links_only_gitignored_paths_and_warns_on_the_rest() {
    let root = tmp("carry");
    git(&root, &["init", "-q", "-b", "main"]).unwrap();
    configure(&root);
    commit_file(
        &root,
        "README.md",
        "hello
",
        "init",
    );
    commit_file(
        &root,
        ".gitignore",
        ".ralphy/
.env
.vscode/
node_modules/
",
        "ignore",
    );
    std::fs::write(
        root.join(".env"),
        "SECRET=1
",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".vscode")).unwrap();
    std::fs::write(
        root.join(".vscode").join("settings.json"),
        "{}
",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("node_modules").join("pkg")).unwrap();
    std::fs::write(
        root.join("node_modules").join("pkg").join("index.js"),
        "1
",
    )
    .unwrap();

    let c = add(&root, "wt-c", None).unwrap();
    let dest = Path::new(&c.path);
    let settings = crate::settings::WorktreeSettings {
        copy: vec![
            ".env".into(),
            ".vscode/".into(),
            "README.md".into(),
            "missing".into(),
        ],
        share: vec!["node_modules".into(), ".env".into()],
    };
    let warnings = carry_over(&root, dest, &settings);
    assert_eq!(warnings.len(), 3, "{warnings:?}");
    assert!(
        warnings[0].starts_with("worktree.copy: README.md: is not gitignored"),
        "{warnings:?}"
    );
    assert!(
        warnings[1].starts_with("worktree.copy: missing: does not exist"),
        "{warnings:?}"
    );
    assert!(
        warnings[2].starts_with("worktree.share: .env: is not a directory"),
        "{warnings:?}"
    );

    assert_eq!(
        std::fs::read_to_string(dest.join(".env")).unwrap(),
        "SECRET=1
"
    );
    assert!(
        !dest
            .join(".env")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink(),
        "copy is a real file"
    );
    assert_eq!(
        std::fs::read_to_string(dest.join(".vscode").join("settings.json")).unwrap(),
        "{}
"
    );
    let shared = dest.join("node_modules");
    assert_eq!(
        std::fs::read_to_string(shared.join("pkg").join("index.js")).unwrap(),
        "1
",
        "the link resolves into the primary's tree"
    );
    let meta = shared.symlink_metadata().unwrap();
    assert!(
        meta.file_type().is_symlink() || (cfg!(windows) && meta.file_type().is_dir()),
        "share is a link (junction on Windows), not a copy"
    );
    // A junction reports as a directory reparse point; prove it is not a
    // copy by writing through it.
    std::fs::write(
        shared.join("pkg").join("added.js"),
        "2
",
    )
    .unwrap();
    assert!(root
        .join("node_modules")
        .join("pkg")
        .join("added.js")
        .exists());
    // Everything carried is ignored in the worktree too (same committed
    // .gitignore), so the new tree is still clean.
    let listing = list(&root).unwrap();
    assert!(!listing.worktrees[0].dirty, "{listing:?}");

    // A second run: every target already exists → warnings, nothing
    // overwritten.
    let again = carry_over(&root, dest, &settings);
    assert!(
        again
            .iter()
            .filter(|w| w.contains("already exists"))
            .count()
            == 3,
        "{again:?}"
    );

    // THE gate this whole mode hangs on: removing the worktree must not
    // follow the share into the primary. Measured without the unlink,
    // `git worktree remove` on Windows deleted `root/node_modules/pkg`.
    remove(&root, "wt-c").unwrap();
    assert!(!dest.exists(), "the worktree is gone");
    assert_eq!(
        std::fs::read_to_string(root.join("node_modules").join("pkg").join("index.js")).unwrap(),
        "1\n",
        "the primary's shared directory survives the worktree's removal"
    );
    assert!(root
        .join("node_modules")
        .join("pkg")
        .join("added.js")
        .exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// ONE fixture: `wt-c` from the default base, `wt-d` from an explicit
/// one, then the four refusals against the same repo, each checked to
/// have left nothing behind.
#[test]
fn add_creates_a_worktree_and_refuses_each_bad_input() {
    let root = tmp("add");
    git(&root, &["init", "-q", "-b", "main"]).unwrap();
    configure(&root);
    commit_file(&root, "README.md", "hello\n", "init");
    commit_file(&root, ".gitignore", ".ralphy/\n", "ignore the run dir");
    git(&root, &["branch", "taken"]).unwrap();

    let c = add(&root, "wt-c", None).unwrap();
    assert_eq!(c.name, "wt-c");
    assert_eq!(c.branch, "wt-c");
    assert_eq!(c.base, "main");
    assert!(!c.dirty);
    assert!(
        c.path.ends_with("/.ralphy/worktrees/wt-c"),
        "path in git's spelling: {}",
        c.path
    );
    let wt_c_dir = root.join(WORKTREES_DIR).join("wt-c");
    assert!(wt_c_dir.is_dir());
    assert_eq!(git(&root, &["config", "branch.wt-c.base"]).unwrap(), "main");
    assert_eq!(
        git(&wt_c_dir, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap(),
        "wt-c"
    );

    let d = add(&root, "wt-d", Some("taken")).unwrap();
    assert_eq!(d.base, "taken");

    // A leftover directory at the location (no git registration): the
    // fifth gate, before git could leave an orphaned branch behind.
    std::fs::create_dir_all(root.join(WORKTREES_DIR).join("left")).unwrap();
    std::fs::write(root.join(WORKTREES_DIR).join("left").join("x"), "x").unwrap();
    let refusals = [
        ("a..b", "not a valid branch name"),
        ("a/b", "must be a single path segment"),
        ("taken", "already exists"),
        ("main", "is checked out at"),
        ("left", "a directory already exists at"),
    ];
    for (name, needle) in refusals {
        let err = add(&root, name, None).unwrap_err().to_string();
        assert!(err.contains(needle), "{name}: {err}");
        assert!(
            name == "left" || !root.join(WORKTREES_DIR).join(name).exists(),
            "{name}: nothing written on refusal"
        );
    }
    let err = add(&root, "wt-h", Some("HEAD")).unwrap_err().to_string();
    assert!(err.contains("is not a branch"), "{err}");
    for name in ["a..b", "a/b", "left", "wt-h"] {
        let probe = raw(
            &root,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{name}"),
            ],
        )
        .unwrap();
        assert!(!probe.status.success(), "{name}: no branch was created");
    }

    let listing = list(&root).unwrap();
    let names: Vec<&str> = listing.worktrees.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, vec!["wt-c", "wt-d"], "listing: {listing:?}");
    assert_eq!(listing.worktrees[0].base, "main");
    assert_eq!(listing.worktrees[1].base, "taken");

    let _ = std::fs::remove_dir_all(&root);
}

fn branch_exists(root: &Path, name: &str) -> bool {
    raw(
        root,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ],
    )
    .unwrap()
    .status
    .success()
}

fn remove_error(root: &Path, name: &str) -> (RemoveError, String) {
    let err = remove(root, name).unwrap_err();
    let text = err.to_string();
    let domain = err
        .downcast_ref::<RemoveError>()
        .unwrap_or_else(|| panic!("{name}: not a RemoveError: {err:?}"))
        .clone();
    (domain, text)
}

/// ONE fixture, the gates in ADR-0063 §1's order: `wt-lock` is locked AND
/// dirty and answers `Locked` (the ordering oracle), then unlocked answers
/// `Dirty`, then cleaned answers `Ok` with directory and branch gone;
/// `wt-keep` carries a commit beyond its base and answers `BranchKept` with
/// the directory gone and the branch present; an unknown name and the
/// primary itself answer `NotFound`.
#[test]
fn remove_applies_the_gates_in_order() {
    let root = tmp("remove");
    git(&root, &["init", "-q", "-b", "main"]).unwrap();
    configure(&root);
    commit_file(&root, "README.md", "hello\n", "init");
    commit_file(&root, ".gitignore", ".ralphy/\n", "ignore the run dir");

    add(&root, "wt-lock", None).unwrap();
    let wt_lock = root.join(WORKTREES_DIR).join("wt-lock");
    let rel_lock = format!("{WORKTREES_DIR}/wt-lock");
    git(&root, &["worktree", "lock", "--reason", "held", &rel_lock]).unwrap();
    std::fs::write(wt_lock.join("scratch.txt"), "dirty\n").unwrap();

    // (1) locked AND dirty: the lock gate answers first.
    let (err, text) = remove_error(&root, "wt-lock");
    assert_eq!(
        err,
        RemoveError::Locked {
            name: "wt-lock".into()
        }
    );
    assert!(text.contains("is locked"), "{text}");
    assert!(wt_lock.is_dir(), "a refusal keeps the directory");

    // (2) unlocked but dirty.
    git(&root, &["worktree", "unlock", &rel_lock]).unwrap();
    let (err, text) = remove_error(&root, "wt-lock");
    assert_eq!(
        err,
        RemoveError::Dirty {
            name: "wt-lock".into()
        }
    );
    assert!(text.contains("has uncommitted changes"), "{text}");
    assert!(wt_lock.is_dir(), "a refusal keeps the directory");

    // (3) clean: directory and branch go.
    std::fs::remove_file(wt_lock.join("scratch.txt")).unwrap();
    remove(&root, "wt-lock").unwrap();
    assert!(!wt_lock.exists(), "the directory is gone");
    assert!(!branch_exists(&root, "wt-lock"), "the branch is deleted");
    assert_eq!(
        base_of(&root, "wt-lock").unwrap(),
        "",
        "no branch.wt-lock.base survives the branch"
    );

    // (4) a commit beyond the base: the directory goes, the branch stays.
    add(&root, "wt-keep", None).unwrap();
    let wt_keep = root.join(WORKTREES_DIR).join("wt-keep");
    configure(&wt_keep);
    commit_file(&wt_keep, "feature.txt", "x\n", "beyond");
    let (err, text) = remove_error(&root, "wt-keep");
    match &err {
        RemoveError::BranchKept { name, why } => {
            assert_eq!(name, "wt-keep");
            assert!(why.contains("not on main"), "why: {why}");
        }
        other => panic!("expected BranchKept, got {other:?}"),
    }
    assert!(text.contains("branch 'wt-keep' kept"), "{text}");
    assert!(!wt_keep.exists(), "the directory is gone");
    assert!(branch_exists(&root, "wt-keep"), "the branch is kept");

    // (5) unknown names, the primary included.
    for name in ["nope", "primary"] {
        let (err, _) = remove_error(&root, name);
        assert_eq!(err, RemoveError::NotFound { name: name.into() });
    }

    // (6) nothing left to list.
    assert!(list(&root).unwrap().worktrees.is_empty());

    let _ = std::fs::remove_dir_all(&root);
}
