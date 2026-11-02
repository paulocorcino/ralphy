use super::*;
use crate::scratch::remove_scratch;

#[test]
fn project_slug_from_remote_and_path_fallback() {
    // Both common remote forms normalize to the same `owner/repo` slug.
    assert_eq!(
        slug_from_url("git@github.com:owner/repo.git").as_deref(),
        Some("owner/repo")
    );
    assert_eq!(
        slug_from_url("https://github.com/owner/repo").as_deref(),
        Some("owner/repo")
    );
    // The ssh:// form and a trailing `.git` are handled too.
    assert_eq!(
        slug_from_url("ssh://git@github.com/owner/repo.git").as_deref(),
        Some("owner/repo")
    );

    // A path with no git remote (a non-existent dir → `origin_url` is `None`)
    // falls back to a non-empty `path-<hash>` slug.
    let no_remote = std::env::temp_dir().join("ralphy-no-such-repo-xyz");
    let slug = project_slug(&no_remote);
    assert!(slug.starts_with("path-"), "fallback slug form: {slug}");
    assert!(
        slug.len() > "path-".len(),
        "fallback slug non-empty: {slug}"
    );
}

fn init_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ralphy-git-{}-{}", std::process::id(), name));
    remove_scratch(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]).unwrap();
    git(&dir, &["config", "user.email", "t@example.com"]).unwrap();
    git(&dir, &["config", "user.name", "Test"]).unwrap();
    dir
}

/// `checkout` takes a BRANCH and refuses anything git would read as an
/// option (audit F10): `--pathspec-from-file=.gitignore` restores every
/// listed path from the index, so the negative control is a dirty tracked
/// file that must still be dirty after the refusal. `check-ref-format` is
/// what refuses `a..b`; the leading `-` never reaches git at all.
#[test]
fn checkout_refuses_option_shaped_and_malformed_names() {
    let dir = init_repo("checkout-refuse");
    std::fs::write(dir.join("tracked.txt"), "one").unwrap();
    git(&dir, &["add", "-A"]).unwrap();
    git(&dir, &["commit", "-q", "-m", "one"]).unwrap();
    git(&dir, &["branch", "feat/x"]).unwrap();
    std::fs::write(dir.join("tracked.txt"), "edited").unwrap();

    for bad in [
        "-b",
        "--pathspec-from-file=tracked.txt",
        "--orphan",
        "a..b",
        "",
        "x y",
    ] {
        let err = checkout(&dir, bad).unwrap_err().to_string();
        assert!(err.contains("invalid branch name"), "{bad:?}: {err}");
        assert_eq!(
            std::fs::read_to_string(dir.join("tracked.txt")).unwrap(),
            "edited",
            "{bad:?}: the working tree is untouched"
        );
    }
    // A real branch still switches (the edit rides along — git's behaviour).
    checkout(&dir, "feat/x").unwrap();
    assert_eq!(current_branch(&dir).unwrap(), "feat/x");

    // `validate_commitish` (audit F11): a `-`-leading ref and a name that
    // resolves to nothing are refused; a SHA, a branch and `origin`-style
    // remote names that exist pass.
    let sha = head_sha(&dir).unwrap();
    for bad in ["--detach", "-x", "nope", ""] {
        assert!(validate_commitish(&dir, bad).is_err(), "{bad:?}");
    }
    validate_commitish(&dir, &sha).unwrap();
    validate_commitish(&dir, "main").unwrap();
    validate_commitish(&dir, "feat/x~0").unwrap();
}

/// A run branch cut from a remote branch (`origin/main`) has no upstream: git
/// would otherwise track `origin/main`, and a plain `git push` from the run
/// branch would land on `main`.
#[test]
fn a_branch_cut_from_a_remote_base_tracks_nothing() {
    let origin = init_repo("track-origin");
    std::fs::write(origin.join("README.md"), "hello\n").unwrap();
    git(&origin, &["add", "."]).unwrap();
    git(&origin, &["commit", "-q", "-m", "init"]).unwrap();
    let clone = origin.with_file_name(format!(
        "{}-clone",
        origin.file_name().unwrap().to_string_lossy()
    ));
    remove_scratch(&clone);
    git(
        &origin,
        &[
            "clone",
            "-q",
            &origin.to_string_lossy(),
            &clone.to_string_lossy(),
        ],
    )
    .unwrap();
    // Control: the clone's own `main` tracks `origin/main`, so this git does
    // set an upstream when a branch comes from a remote one.
    assert_eq!(
        git(&clone, &["config", "--get", "branch.main.merge"]).unwrap(),
        "refs/heads/main"
    );

    checkout_new_branch(&clone, "afk/run-t", "origin/main").unwrap();

    assert_eq!(current_branch(&clone).unwrap(), "afk/run-t");
    assert!(
        git(&clone, &["config", "--get", "branch.afk/run-t.merge"]).is_err(),
        "the run branch must have no upstream"
    );
    remove_scratch(&clone);
    remove_scratch(&origin);
}

#[test]
fn commit_all_snapshot_clears_dirty_tree_with_fixed_subject() {
    let dir = init_repo("snapshot");
    // Seed an initial commit so HEAD exists, then dirty the tree.
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(&dir, &["add", "."]).unwrap();
    git(&dir, &["commit", "-q", "-m", "init"]).unwrap();
    std::fs::write(dir.join("README.md"), "changed\n").unwrap();
    std::fs::write(dir.join("new.txt"), "added\n").unwrap();

    commit_all_snapshot(&dir).unwrap();

    let porcelain = git(&dir, &["status", "--porcelain"]).unwrap();
    assert!(
        porcelain.is_empty(),
        "tree must be clean after snapshot, got:\n{porcelain}"
    );
    let subject = git(&dir, &["log", "-1", "--format=%s"]).unwrap();
    assert_eq!(subject, "chore: snapshot before ralphy init");

    remove_scratch(&dir);
}

#[test]
fn local_branches_lists_all_local_branches() {
    let dir = init_repo("local-branches");
    git(&dir, &["commit", "--allow-empty", "-q", "-m", "init"]).unwrap();
    git(&dir, &["branch", "feat/x"]).unwrap();

    let branches = local_branches(&dir).unwrap();
    assert!(
        branches.iter().any(|b| b == "feat/x"),
        "must list feat/x, got: {branches:?}"
    );
    assert!(
        branches.iter().any(|b| b == "main"),
        "must list the default branch, got: {branches:?}"
    );

    remove_scratch(&dir);
}
