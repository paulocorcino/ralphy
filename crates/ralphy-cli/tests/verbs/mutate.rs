//! End-to-end coverage for `ralphy branch switch|create` (ADR-0036 §6, issue
//! #189): drives the real `ralphy` binary against an isolated temp git repo,
//! never the checkout under test. `label set` needs `gh` (not in
//! `environment.md`) so only the guarded-refusal path — reached before any
//! forge call — is covered here.
//!
//! The held-lock refusals of `branch switch`, `config set` and `config unset`
//! are rows of `lock_refusal.rs`; [`lock_rows`] builds them.

use super::support::{git_output, ralphy, run_git, LockRow};

/// `git init` a fresh temp repo with a born HEAD (an empty initial commit),
/// so branch creation/switch has a commit-ish to work from.
fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("creating a temporary directory");
    let root = dir.path();
    run_git(root, &["init", "--quiet"]);
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "user.name", "Test"]);
    run_git(root, &["commit", "--allow-empty", "--quiet", "-m", "init"]);
    dir
}

#[test]
fn branch_create_makes_branch_when_lock_free() {
    let repo = init_repo();

    let status = crate::support::ralphy_command()
        .args([
            "branch",
            "create",
            "feature-x",
            "--repo",
            &repo.path().to_string_lossy(),
        ])
        .status()
        .expect("spawning ralphy");
    assert!(
        status.success(),
        "branch create must succeed when lock is free"
    );

    let head = git_output(repo.path(), &["rev-parse", "--abbrev-ref", "HEAD"]);
    assert_eq!(head, "feature-x");
}

#[test]
fn branch_list_reports_current_and_branches() {
    let repo = init_repo();
    run_git(repo.path(), &["branch", "other"]);
    let current = git_output(repo.path(), &["rev-parse", "--abbrev-ref", "HEAD"]);

    let out = crate::support::ralphy_command()
        .args([
            "branch",
            "list",
            "--format",
            "json",
            "--repo",
            &repo.path().to_string_lossy(),
        ])
        .output()
        .expect("spawning ralphy");
    assert!(out.status.success(), "branch list must succeed");

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("branch list emits JSON");
    assert_eq!(
        v["current"], current,
        "current must be the checked-out branch"
    );
    let branches: Vec<String> = v["branches"]
        .as_array()
        .expect("branches array")
        .iter()
        .map(|b| b.as_str().unwrap().to_string())
        .collect();
    assert!(
        branches.contains(&current),
        "branches must contain the current branch, got: {branches:?}"
    );
    assert!(
        branches.iter().any(|b| b == "other"),
        "branches must contain the created branch, got: {branches:?}"
    );
    let worktrees = v["worktrees"].as_object().expect("worktrees object");
    assert!(
        !worktrees.contains_key("other"),
        "a branch no worktree holds has no holder, got: {worktrees:?}"
    );
}

/// `ralphy branch list --format json` against `repo`, parsed.
fn branch_list_json(repo: &std::path::Path) -> serde_json::Value {
    let out = ralphy(&[
        "branch",
        "list",
        "--format",
        "json",
        "--repo",
        &repo.to_string_lossy(),
    ]);
    assert!(out.status.success(), "branch list must succeed");
    serde_json::from_slice(&out.stdout).expect("branch list emits JSON")
}

#[test]
fn branch_list_names_the_worktree_that_holds_a_branch() {
    let repo = init_repo();
    let other = tempfile::tempdir().unwrap();
    let held = other.path().join("held");
    run_git(
        repo.path(),
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "busy",
            &held.to_string_lossy(),
        ],
    );

    let v = branch_list_json(repo.path());
    let path = v["worktrees"]["busy"]
        .as_str()
        .unwrap_or_else(|| panic!("busy must name its worktree, got: {v}"));
    assert_eq!(
        std::fs::canonicalize(path).unwrap(),
        std::fs::canonicalize(&held).unwrap(),
        "the holder must be the worktree that has the branch checked out"
    );
}

#[test]
fn branch_switch_refused_by_local_changes_leads_with_gits_reason() {
    let repo = init_repo();
    let root = repo.path();
    std::fs::write(root.join("f.txt"), "one\n").unwrap();
    run_git(root, &["add", "f.txt"]);
    run_git(root, &["commit", "--quiet", "-m", "one"]);
    run_git(root, &["branch", "other"]);
    std::fs::write(root.join("f.txt"), "two\n").unwrap();
    run_git(root, &["commit", "--quiet", "-am", "two"]);
    std::fs::write(root.join("f.txt"), "three\n").unwrap();

    let out = ralphy(&[
        "branch",
        "switch",
        "other",
        "--repo",
        &root.to_string_lossy(),
    ]);
    assert!(!out.status.success(), "git must refuse the switch");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let reason = stderr
        .split_once("cannot switch branch: ")
        .map(|(_, rest)| rest)
        .unwrap_or_else(|| panic!("the error must lead with the refusal, got: {stderr}"));
    assert!(
        reason.contains("local changes"),
        "git's reason must follow the prefix, got: {stderr}"
    );
}

/// The held-lock rows of `branch switch` (its state: the checked-out branch)
/// and `config set`/`config unset` (their state: the settings bytes). `other`
/// exists, so the switch refusal is the lock's, not a missing branch's. The
/// config fixture is seeded by a lock-free `config set`, which proves the
/// verb can write.
pub(super) fn lock_rows() -> Vec<LockRow> {
    let switch = init_repo();
    run_git(switch.path(), &["branch", "other"]);
    let switch_root = switch.path().to_path_buf();
    let mut rows = vec![LockRow {
        verb: "branch switch",
        lock_repo: switch_root.clone(),
        args: vec![
            "branch".to_string(),
            "switch".to_string(),
            "other".to_string(),
            "--repo".to_string(),
            switch_root.to_string_lossy().to_string(),
        ],
        state: Box::new(move || git_output(&switch_root, &["rev-parse", "--abbrev-ref", "HEAD"])),
        _dirs: vec![switch],
    }];
    for (verb, args) in [
        ("config set", &["set", "base_branch", "other"][..]),
        ("config unset", &["unset", "base_branch"][..]),
    ] {
        let repo = init_repo();
        let root = repo.path().to_path_buf();
        let seed = ralphy(&[
            "config",
            "--repo",
            &root.to_string_lossy(),
            "set",
            "base_branch",
            "seeded",
        ]);
        assert!(
            seed.status.success(),
            "config set must succeed when the lock is free: {}",
            String::from_utf8_lossy(&seed.stderr)
        );
        let settings = root.join(".ralphy").join("settings.json");
        assert!(settings.is_file(), "the free config set wrote settings");
        let mut full = vec![
            "config".to_string(),
            "--repo".to_string(),
            root.to_string_lossy().to_string(),
        ];
        full.extend(args.iter().map(|a| a.to_string()));
        rows.push(LockRow {
            verb,
            lock_repo: root,
            args: full,
            state: Box::new(move || {
                format!(
                    "{:?}",
                    std::fs::read(&settings).expect("settings still readable")
                )
            }),
            _dirs: vec![repo],
        });
    }
    rows
}
