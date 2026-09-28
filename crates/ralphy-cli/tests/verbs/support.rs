//! Helpers shared by the verb modules: git and `ralphy` spawns, and a live
//! child that holds a repo's run lock. Each module keeps its own `init_repo`,
//! because each suite needs a different fixture.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};

use tempfile::TempDir;

pub(crate) fn run_git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .expect("spawning git");
    assert!(status.success(), "git {args:?} failed");
}

pub(crate) fn git_output(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("spawning git");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub(crate) fn commit(root: &Path, file: &str, body: &str, msg: &str) {
    std::fs::write(root.join(file), body).unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "--quiet", "-m", msg]);
}

pub(crate) fn ralphy(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ralphy"))
        .args(args)
        .output()
        .expect("spawning ralphy")
}

/// Spawn the live child whose pid a run lock names.
pub(crate) fn spawn_lock_holder() -> Child {
    Command::new(env!("CARGO_BIN_EXE_runlock_test_child"))
        .spawn()
        .expect("spawning runlock_test_child")
}

/// Write `repo`'s `.ralphy/run.lock` naming `pid`, so the repo looks like a
/// run holds it while that process lives.
pub(crate) fn write_run_lock(repo: &Path, pid: u32) {
    let lock_dir = repo.join(".ralphy");
    std::fs::create_dir_all(&lock_dir).unwrap();
    std::fs::write(
        lock_dir.join("run.lock"),
        serde_json::json!({
            "pid": pid,
            "started_at": "2026-07-25T10:00:00-03:00",
        })
        .to_string(),
    )
    .unwrap();
}

/// Hold `repo`'s run lock with a live child. The caller passes the child to
/// [`release`].
pub(crate) fn hold_run_lock(repo: &Path) -> Child {
    let child = spawn_lock_holder();
    write_run_lock(repo, child.id());
    child
}

pub(crate) fn release(mut child: Child) {
    if let Err(e) = child.kill() {
        eprintln!("killing the lock holder: {e}");
    }
    if let Err(e) = child.wait() {
        eprintln!("waiting for the lock holder: {e}");
    }
}

/// One row of the held-lock refusal table (`lock_refusal.rs`): a write verb,
/// the fixture it runs against, and the state it must leave untouched.
pub(crate) struct LockRow {
    /// The verb as the refusal names it: `refusing to <verb>`.
    pub(crate) verb: &'static str,
    /// Keeps the row's temp dirs alive until the row is checked.
    pub(crate) _dirs: Vec<TempDir>,
    /// The primary checkout whose `.ralphy/run.lock` is held.
    pub(crate) lock_repo: PathBuf,
    pub(crate) args: Vec<String>,
    /// Reads the state a refused verb must leave byte-identical.
    pub(crate) state: Box<dyn Fn() -> String>,
}
