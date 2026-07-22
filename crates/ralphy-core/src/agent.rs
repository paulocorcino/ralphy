//! The agent contract. This signature is the one hard-to-reverse commitment of
//! the rewrite (see docs/adr/0002): it is PTY-free and names no vendor. Execution
//! mode, the PTY, completion detection, and complexity routing all live behind
//! it, inside an adapter.

use anyhow::Result;

use crate::{Execution, Issue, Plan, Workspace};

/// One agent CLI vendor, behind the core's boundary. The core asks it to plan or
/// execute an issue and receives a domain result — it never learns *how* the
/// agent is driven.
pub trait Agent {
    /// The adapter's self-reported vendor label, stamped onto each ledger line
    /// (ADR-0008 D6). An opaque string to the core: it only carries the value
    /// through and never branches on it, so the vendor-agnostic boundary
    /// (ADR-0002) holds.
    fn name(&self) -> &'static str;

    /// The read-time harvest-tax floor (issue #270): the input tokens this vendor's
    /// CLI injects on *each* invocation by auto-discovering foreign skills, or `None`
    /// for a vendor that does not harvest. Like [`name`](Self::name), an opaque
    /// number the core only carries through — it is never branched on — so the
    /// vendor-agnostic boundary (ADR-0002/0004) holds. The CLI turns it into a
    /// read-time estimate view (`floor × invocation_count`), the analog of USD
    /// (ADR-0008 D8); it is never stored in the ledger or on the wire.
    fn harvest_floor(&self) -> Option<u64> {
        None
    }

    /// Read the issue and the repo, decide feasibility, and write the plan
    /// artifact into the workspace. The returned [`Plan`] points at it.
    fn plan(&self, issue: &Issue, ws: &Workspace) -> Result<Plan>;

    /// Carry out the plan, committing onto the workspace's current branch.
    /// Returns the domain [`Outcome`] paired with the phase's token [`crate::Usage`]
    /// in an [`Execution`] (ADR-0008 D4).
    fn execute(&self, plan: &Plan, ws: &Workspace) -> Result<Execution>;
}
