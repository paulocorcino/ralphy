//! Queue-loop lifecycle tests over a throwaway git repo, with a `ScriptedAgent`,
//! a `RecordingTracker`, and a `ScriptedClock` standing in for the real adapter,
//! `gh`, and the wall clock. These prove the acceptance criteria the core owns:
//! ascending/deduped order, close-on-green with a branch-pointing comment and no
//! label mutation, stop-at-first-non-green with later issues untouched, dry-run
//! closing nothing, and the deadline blocking the next issue.

#![expect(
    clippy::disallowed_methods,
    reason = "nextest runs each test in its own process, so a test may change its environment"
)]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use ralphy_core::{
    resolve_queue_view, run_queue, Issue, Outcome, QueueStatus, RunClock, StopReason, Usage,
    Workspace,
};

// One test binary (`queue`), one module per queue behaviour. The shared fixtures
// live in `harness`; the modules reach them through `use super::*`.
mod blockers;
mod branches;
mod close_artifacts;
mod events;
mod harness;
mod lifecycle;
mod limits;
mod refused;
mod selection;
mod session_ledger;
mod verify;

use harness::*;
