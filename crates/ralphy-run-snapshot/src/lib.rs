//! The **run snapshot** channel: a run publishes its state as one versioned
//! JSON document per `runid` under its repo's `.ralphy/runstate/`, and the
//! daemon lists live runs by reading that directory
//! ([ADR-0047](../../../docs/adr/0047-run-state-snapshot-channel.md)).
//!
//! This crate is the shared leaf both sides name: the document type and its
//! version constant, the atomic writer, the RAII removal guard the run holds
//! for its lifetime, and the reader that classifies each document. It is
//! deliberately dependency-free beyond serde — the projection from `RunState`
//! lives in `ralphy-cli`, and the liveness predicate is injected.
//!
//! The document is **state, not a log**: applied by replacement, never
//! replayed, and it says nothing about whether its run is alive. Liveness is
//! derived by the reader from the header `pid` (ADR-0047 §7).

mod document;
mod guard;
mod read;
mod write;

pub use document::{
    snapshot_dir, snapshot_path, stop_path, IssueBlock, PhaseBlock, PlanBlock, PlanStepBlock,
    QueueBlock, RunSnapshot, SleepBlock, SNAPSHOT_VERSION,
};
pub use guard::SnapshotGuard;
pub use read::{list_runs, RunListing, UnreadableRun};
pub use write::write_atomic;
