use super::*;

#[test]
fn discard_restores_a_tracked_file_from_head() {
    let dir = init_repo("discard-tracked");
    std::fs::write(dir.join("never-touched.txt"), "mangled\n").unwrap();

    assert_eq!(
        discard(&dir, &p("never-touched.txt")).unwrap(),
        DiscardOutcome::Discarded {
            restored: 1,
            deleted: 0
        }
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("never-touched.txt")).unwrap(),
        "steady\n",
        "the committed content is back on disk"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn discard_deletes_an_untracked_file() {
    let dir = init_repo("discard-untracked");
    std::fs::write(dir.join("loose.txt"), "never committed\n").unwrap();

    // The counts are the classifier's oracle: an inverted tracked/untracked
    // partition flips them even though the file would still leave disk.
    assert_eq!(
        discard(&dir, &p("loose.txt")).unwrap(),
        DiscardOutcome::Discarded {
            restored: 0,
            deleted: 1
        }
    );
    assert!(!dir.join("loose.txt").exists(), "the file is gone");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn discard_deletes_an_untracked_directory() {
    // `git status --porcelain=v2` reports an untracked DIRECTORY as ONE entry
    // named `newdir/` — so that entry shape is what the panel offers, and
    // `clean -d` is what makes it discardable.
    let dir = init_repo("discard-untracked-dir");
    std::fs::create_dir_all(dir.join("newdir/sub")).unwrap();
    std::fs::write(dir.join("newdir/sub/f.txt"), "deep\n").unwrap();

    assert!(
        changes(&dir).unwrap().iter().any(|c| c.path == "newdir/"),
        "git names the whole directory as one entry, got: {:?}",
        changes(&dir).unwrap()
    );
    assert_eq!(
        discard(&dir, &p("newdir/")).unwrap(),
        DiscardOutcome::Discarded {
            restored: 0,
            deleted: 1
        }
    );
    assert!(!dir.join("newdir").exists(), "the directory is gone");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn discard_leaves_every_other_path_alone() {
    let dir = init_repo("discard-isolation");
    commit_file(&dir, "a.txt", "one\n", "add a");
    commit_file(&dir, "b.txt", "two\n", "add b");
    std::fs::write(dir.join("a.txt"), "edited a\n").unwrap();
    std::fs::write(dir.join("b.txt"), "edited b\n").unwrap();
    std::fs::write(dir.join("loose.txt"), "loose\n").unwrap();

    assert_eq!(
        discard(&dir, &p("a.txt")).unwrap(),
        DiscardOutcome::Discarded {
            restored: 1,
            deleted: 0
        }
    );
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "one\n");
    assert_eq!(
        std::fs::read_to_string(dir.join("b.txt")).unwrap(),
        "edited b\n",
        "the other modified path keeps its edit"
    );
    assert!(
        dir.join("loose.txt").exists(),
        "an untracked bystander is not cleaned"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The negative control for "a staged change is never silently thrown away".
/// This is the test that reds if the implementation ever reaches for
/// `--source=HEAD --staged`.
#[test]
fn discard_keeps_a_staged_change() {
    let dir = init_repo("discard-staged");
    commit_file(&dir, "both.txt", "base\n", "add both");
    std::fs::write(dir.join("both.txt"), "staged\n").unwrap();
    git(&dir, &["add", "both.txt"]).unwrap();
    std::fs::write(dir.join("both.txt"), "worktree only\n").unwrap();

    let mixed = entry(&dir, "both.txt");
    assert!(
        mixed.index_status.is_some() && mixed.worktree_status.is_some(),
        "the fixture must be staged AND edited again, got {mixed:?}"
    );

    assert_eq!(
        discard(&dir, &p("both.txt")).unwrap(),
        DiscardOutcome::Discarded {
            restored: 1,
            deleted: 0
        }
    );
    assert_eq!(
        git(&dir, &["show", ":both.txt"]).unwrap(),
        "staged",
        "the staged blob survives an unstaged discard"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("both.txt")).unwrap(),
        "staged\n",
        "the working tree went back to the INDEX, not to HEAD"
    );
    assert_eq!(staged_paths(&dir), "both.txt", "the path is still staged");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn discard_refuses_a_path_outside_the_change_set() {
    let dir = init_repo("discard-unknown");
    std::fs::write(dir.join("a.txt"), "new\n").unwrap();

    let refused = discard(&dir, &p("never-touched.txt")).unwrap();
    assert_eq!(
        refused,
        DiscardOutcome::NotInChangeSet {
            path: "never-touched.txt".to_string()
        }
    );
    assert_eq!(
        refused.reason().as_deref(),
        Some(
            "cannot discard: never-touched.txt has no change — \
             it may be committed or reverted already"
        )
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("never-touched.txt")).unwrap(),
        "steady\n",
        "a refusal ran no git write"
    );
    assert!(dir.join("a.txt").exists(), "and cleaned nothing either");
    assert_eq!(staged_paths(&dir), "");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn discard_refuses_an_empty_list() {
    let dir = init_repo("discard-empty");
    std::fs::write(dir.join("a.txt"), "new\n").unwrap();

    assert_eq!(discard(&dir, &[]).unwrap(), DiscardOutcome::NoPaths);
    assert!(dir.join("a.txt").exists(), "nothing on disk moved");
    assert_eq!(
        std::fs::read_to_string(dir.join("never-touched.txt")).unwrap(),
        "steady\n"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// REPRODUCED live before the fix: `git --literal-pathspecs restore --worktree
/// -- f.txt` on an unmerged path exits 1 with `error: path 'f.txt' is
/// unmerged`. A conflicted entry has NO index side (`changes.rs`: a `u` record
/// is all worktree work), so the panel's unstaged group renders its discard
/// control — the operator would have confirmed the TRACKED dialog ("anything
/// staged for this file is kept", false here) and been shown git's raw string.
#[test]
fn discard_refuses_a_conflicted_path() {
    let dir = init_repo("discard-conflicted");
    commit_file(&dir, "f.txt", "base\n", "add f");
    // A REAL merge conflict, not a hand-written index: the `u` record is what
    // the change set reads, and only git produces a faithful one.
    git(&dir, &["branch", "other"]).unwrap();
    git(&dir, &["checkout", "--quiet", "other"]).unwrap();
    commit_file(&dir, "f.txt", "theirs\n", "other side");
    git(&dir, &["checkout", "--quiet", "main"]).unwrap();
    commit_file(&dir, "f.txt", "mine\n", "our side");
    let merged = raw(&dir, &["merge", "other"]).unwrap();
    assert!(
        !merged.status.success(),
        "the fixture must actually conflict"
    );

    let conflicted = entry(&dir, "f.txt");
    assert_eq!(
        conflicted.status,
        ChangeStatus::Conflicted,
        "got {conflicted:?}"
    );
    assert!(
        conflicted.index_status.is_none() && conflicted.worktree_status.is_some(),
        "a conflict is all worktree work — which is why the PANEL offers it a \
         discard control, and why refusing it here matters: {conflicted:?}"
    );

    let before = std::fs::read_to_string(dir.join("f.txt")).unwrap();
    let refused = discard(&dir, &p("f.txt")).unwrap();
    assert_eq!(
        refused,
        DiscardOutcome::Conflicted {
            path: "f.txt".to_string()
        }
    );
    assert_eq!(
        refused.reason().as_deref(),
        Some("cannot discard: f.txt has an unresolved merge conflict — resolve it first"),
        "the operator reads this module's prose, never git's `path … is unmerged`"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("f.txt")).unwrap(),
        before,
        "the refusal ran no git write"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The cross-path invariant the doc comment sells, as an ORACLE: a list whose
/// LAST element is unknown must run NEITHER write — not the restore, and above
/// all not the unrecoverable delete of the good untracked path in the middle.
/// A per-path loop refactor stays green on every other test here and reds on
/// this one.
#[test]
fn discard_refuses_a_mixed_batch_before_either_write() {
    let dir = init_repo("discard-mixed-refusal");
    commit_file(&dir, "good.txt", "committed\n", "add good");
    std::fs::write(dir.join("good.txt"), "edited\n").unwrap();
    std::fs::write(dir.join("loose.txt"), "loose\n").unwrap();

    let refused = discard(
        &dir,
        &[
            "good.txt".to_string(),
            "loose.txt".to_string(),
            "never-touched.txt".to_string(),
        ],
    )
    .unwrap();
    assert_eq!(
        refused,
        DiscardOutcome::NotInChangeSet {
            path: "never-touched.txt".to_string()
        }
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("good.txt")).unwrap(),
        "edited\n",
        "the restore half never ran"
    );
    assert!(
        dir.join("loose.txt").exists(),
        "the DELETE half never ran — the unrecoverable act is what this pins"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A rename's ORIGINAL path names no working-tree content, so it is not
/// discardable — the same asymmetry `stage` documents, and the doc comment on
/// `discard` claims it. Without this test that claim is prose only.
#[test]
fn discard_refuses_a_renames_original_path() {
    let dir = init_repo("discard-rename");
    commit_file(&dir, "old.txt", "l1\nl2\nl3\nl4\n", "add old");
    git(&dir, &["mv", "old.txt", "renamed.txt"]).unwrap();

    let refused = discard(&dir, &p("old.txt")).unwrap();
    assert_eq!(
        refused,
        DiscardOutcome::NotInChangeSet {
            path: "old.txt".to_string()
        },
        "the change set names the rename by its NEW path only"
    );
    assert!(dir.join("renamed.txt").exists(), "nothing was written");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A path removed from the WORKING TREE only (`rm f.txt`, no `git rm`) is the
/// case discard genuinely restores — the positive control that keeps the
/// `NothingInTheWorkingTree` refusal from over-reaching.
#[test]
fn discard_restores_a_worktree_deletion() {
    let dir = init_repo("discard-worktree-delete");
    commit_file(&dir, "gone.txt", "still here\n", "add gone");
    std::fs::remove_file(dir.join("gone.txt")).unwrap();
    assert_eq!(
        entry(&dir, "gone.txt").worktree_status,
        Some(ChangeStatus::Deleted),
        "the fixture must be a WORKTREE deletion, not a staged one"
    );

    assert_eq!(
        discard(&dir, &p("gone.txt")).unwrap(),
        DiscardOutcome::Discarded {
            restored: 1,
            deleted: 0
        }
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("gone.txt")).unwrap(),
        "still here\n",
        "the file is back"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// REPRODUCED live before the fix: `git --literal-pathspecs restore --worktree
/// -- f.txt` on a STAGED deletion exits 1 with `error: pathspec 'f.txt' did not
/// match any file(s) known to git` — the path is in neither the index nor the
/// working tree, so there is nothing to restore from.
#[test]
fn discard_refuses_a_staged_deletion() {
    let dir = init_repo("discard-staged-delete");
    commit_file(&dir, "f.txt", "base\n", "add f");
    git(&dir, &["rm", "--quiet", "f.txt"]).unwrap();
    let staged_delete = entry(&dir, "f.txt");
    assert!(
        staged_delete.index_status.is_some() && staged_delete.worktree_status.is_none(),
        "the fixture must have an index side and NO worktree side, got {staged_delete:?}"
    );

    let refused = discard(&dir, &p("f.txt")).unwrap();
    assert_eq!(
        refused,
        DiscardOutcome::NothingInTheWorkingTree {
            path: "f.txt".to_string()
        }
    );
    assert_eq!(
        refused.reason().as_deref(),
        Some("cannot discard: f.txt has no working-tree change — unstage it instead")
    );
    assert_eq!(
        staged_paths(&dir),
        "f.txt",
        "a refusal leaves the staged deletion exactly as it was"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
