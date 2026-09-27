//! The `/ws/tree` subscriptions over the wire: the file-tree, HEAD, run
//! snapshot and peer filesystem watches. No module sets an env var, so they
//! share one test binary.

mod fleet_watch;
mod head_watch;
mod runs_watch;
mod tree_watch;
