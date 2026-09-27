//! End-to-end coverage for `ralphy branch switch|create` (ADR-0036 §6, issue
//! #189): drives the real `ralphy` binary against an isolated temp git repo,
//! never the checkout under test. `label set` needs `gh` (not in
//! `environment.md`) so only the guarded-refusal path — reached before any
//! forge call — is covered here.
//!
//! The held-lock refusals of `branch switch`, `config set` and `config unset`
//! are rows of `lock_refusal.rs`; [`lock_rows`] builds them.

use std::process::Command;

use super::support::{git_output, ralphy, run_git, LockRow};

/// `git init` a fresh temp repo with a born HEAD (an empty initial commit),
/// so branch creation/switch has a commit-ish to work from.
fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
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

    let status = Command::new(env!("CARGO_BIN_EXE_ralphy"))
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

    let out = Command::new(env!("CARGO_BIN_EXE_ralphy"))
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
