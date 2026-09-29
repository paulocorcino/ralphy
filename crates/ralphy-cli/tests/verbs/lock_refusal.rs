//! Every write verb refuses under a held run lock before it writes (ADR-0036
//! §6): one live lock-holder child, one row per verb. Each row builds its own
//! fixture and captures the state its verb would move; the verb then runs
//! against the held lock and must exit non-zero, name itself in a
//! `refusing to <verb>` line on stderr, and leave that state byte-identical.
//! A guard placed after the write still exits non-zero; the state check is
//! what catches it.
//!
//! Precise about what the rows do NOT prove: a guard placed after a read-only
//! git call would still pass, and one such call is deliberate — the
//! `rev-parse --show-toplevel` that LOCATES the lock has to run first
//! (`crates/ralphy-cli/src/changes.rs`'s module header states the same).

use super::support::{self, LockRow};

/// Kills the lock-holder child when the test ends, also when a row panics.
struct Holder(std::process::Child);

impl Drop for Holder {
    fn drop(&mut self) {
        if let Err(e) = self.0.kill() {
            eprintln!("killing the lock holder: {e}");
        }
        if let Err(e) = self.0.wait() {
            eprintln!("waiting for the lock holder: {e}");
        }
    }
}

#[test]
fn every_write_verb_refuses_under_a_held_run_lock_before_it_writes() {
    let rows: Vec<LockRow> = [
        super::worktree::lock_rows(),
        super::sync::lock_rows(),
        super::checkouts::lock_rows(),
        super::mutate::lock_rows(),
    ]
    .into_iter()
    .flatten()
    .collect();
    assert_eq!(rows.len(), 11, "one row per guarded write verb");

    let holder = Holder(support::spawn_lock_holder());
    for row in rows {
        let verb = row.verb;
        let before = (row.state)();
        support::write_run_lock(&row.lock_repo, holder.0.id());
        let args: Vec<&str> = row.args.iter().map(String::as_str).collect();
        let out = support::ralphy(&args);

        assert!(
            !out.status.success(),
            "{verb}: must refuse under a held run.lock"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(&format!("refusing to {verb}")),
            "{verb}: the refusal names the verb, got: {stderr}"
        );
        assert_eq!(
            (row.state)(),
            before,
            "{verb}: the guard runs BEFORE any write, but the state moved"
        );
    }
}
