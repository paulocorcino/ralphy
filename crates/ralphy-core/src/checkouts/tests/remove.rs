use super::*;

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
        other @ (RemoveError::NotFound { .. }
        | RemoveError::Locked { .. }
        | RemoveError::Dirty { .. }
        | RemoveError::RemoveFailed { .. }) => panic!("expected BranchKept, got {other:?}"),
    }
    assert!(text.contains("branch 'wt-keep' kept"), "{text}");
    assert!(!wt_keep.exists(), "the directory is gone");
    assert!(branch_exists(&root, "wt-keep"), "the branch is kept");

    // (5) switched to another branch, its own branch already deleted: the
    // directory goes, the delete succeeds, the other branch is not touched.
    add(&root, "wt-switch", None).unwrap();
    let wt_switch = root.join(WORKTREES_DIR).join("wt-switch");
    git(&wt_switch, &["switch", "-q", "-c", "other"]).unwrap();
    git(&root, &["branch", "-D", "wt-switch"]).unwrap();
    remove(&root, "wt-switch").unwrap();
    assert!(!wt_switch.exists(), "the directory is gone");
    assert!(
        branch_exists(&root, "other"),
        "the switched-to branch is kept"
    );

    // (6) unknown names, the primary included.
    for name in ["nope", "primary"] {
        let (err, _) = remove_error(&root, name);
        assert_eq!(err, RemoveError::NotFound { name: name.into() });
    }

    // (7) nothing left to list.
    assert!(list(&root).unwrap().worktrees.is_empty());

    remove_scratch(&root);
}

#[test]
fn core_errors_keep_their_text_remove_error() {
    let n = || "wt-a".to_string();
    let cases = [
        (
            RemoveError::NotFound { name: n() },
            "no workbench worktree named 'wt-a'",
        ),
        (
            RemoveError::Locked { name: n() },
            "worktree 'wt-a' is locked: unlock it first",
        ),
        (
            RemoveError::Dirty { name: n() },
            "worktree 'wt-a' has uncommitted changes: commit or discard them first",
        ),
        (
            RemoveError::RemoveFailed {
                name: n(),
                detail: "boom".into(),
            },
            "removing worktree 'wt-a' failed: boom",
        ),
        (
            RemoveError::BranchKept {
                name: n(),
                why: "not merged".into(),
            },
            "removed worktree 'wt-a'; branch 'wt-a' kept: not merged",
        ),
    ];
    for (err, text) in cases {
        assert_eq!(err.to_string(), text);
    }
}
