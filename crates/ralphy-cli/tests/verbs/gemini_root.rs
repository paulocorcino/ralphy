//! End-to-end coverage for `ralphy gemini prepare-root`: the real binary, run
//! the way the daemon runs it before a Gemini console (ADR-0040 Amendment 3) —
//! no arguments, with its working directory in the repo.

use std::process::Command;

use super::support::run_git;

#[test]
fn prepare_root_writes_the_policy_at_the_repo_top_and_prints_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    run_git(root, &["init", "--quiet"]);
    // From a subfolder: the root goes to the repo's top, where the daemon's
    // gate looks, never into the folder the command happened to start in.
    let sub = root.join("src");
    std::fs::create_dir(&sub).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_ralphy"))
        .args(["gemini", "prepare-root"])
        .current_dir(&sub)
        .output()
        .expect("spawning ralphy");

    assert!(
        out.status.success(),
        "prepare-root must succeed; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.is_empty(), "nothing on stdout: {:?}", out.stdout);
    assert!(root
        .join(".ralphy")
        .join("gemini-home")
        .join(".gemini")
        .join("ralphy-policy.toml")
        .is_file());
    assert!(!sub.join(".ralphy").exists(), "no root in the subfolder");
}
