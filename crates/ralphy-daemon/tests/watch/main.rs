//! The subscriptions of the `tree` channel of the page socket, over the wire:
//! the file-tree, HEAD, run snapshot and peer filesystem watches. No module
//! sets an env var, so they share one test binary.

#[path = "../support/golden.rs"]
mod golden;

#[path = "../support/page.rs"]
pub mod page;

mod fleet_watch;
mod head_watch;
mod runs_watch;
mod tree_watch;
