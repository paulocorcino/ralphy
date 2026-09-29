//! End-to-end coverage of the `ralphy` verbs: each module drives the real
//! `ralphy` binary against isolated temp repos, never the checkout under test.
//! No module sets an env var on the test process, so they share one binary.

mod support;

mod blob;
mod changes;
mod checkout_cwd;
mod checkouts;
mod daemon_cli;
mod hook_status;
mod lock_refusal;
mod mutate;
mod sync;
mod usage_recovery;
mod worktree;
