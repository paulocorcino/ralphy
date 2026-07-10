use anyhow::{Context, Result};
use tracing::{info, warn};

use crate::{
    blocked, handoff, knowledge, protocol, references, verify, Issue, IssueTracker, Workspace,
};

/// The scratch file the runner drops in the workspace to hand a failed gate back
/// to the executor (read by the exec charter's repair clause). Vendor-neutral —
/// the runner writes it, any adapter's prompt reads it. Cleared once the gate
/// goes green so it never bleeds into a later run on the same worktree.
const VERIFY_FAILURE_FILE: &str = "verify-failure.md";

/// Write the repair brief for a failed gate so the next `execute()` can read why
/// it failed and fix the root cause. Best-effort: a write failure just means the
/// agent retries blind, which is strictly no worse than not repairing at all.
pub(crate) fn write_verify_failure(
    ws: &Workspace,
    stamp: &str,
    report: &verify::VerifyReport,
    done_signal: &str,
) {
    let path = ws.ralphy_dir().join(VERIFY_FAILURE_FILE);
    if let Err(e) = std::fs::write(&path, verify::repair_brief(stamp, report, done_signal)) {
        warn!(error = %e, "writing the verify-failure repair brief failed");
    }
}

/// Remove the repair brief. Called when the gate passes and at each issue's start
/// so the file only ever reflects the current run's gate state. Absent file is a
/// no-op.
pub(crate) fn clear_verify_failure(ws: &Workspace) {
    let path = ws.ralphy_dir().join(VERIFY_FAILURE_FILE);
    if path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            warn!(error = %e, "removing the stale verify-failure brief failed");
        }
    }
}

/// The scratch file the runner drops in the workspace to hand a protocol-lint
/// violation back to the executor (ADR-0015) — the same vendor-neutral channel
/// as [`VERIFY_FAILURE_FILE`]. Written on the first violation only (one bounce);
/// cleared at each issue's start and once the lint is settled.
const PROTOCOL_FAILURE_FILE: &str = "protocol-failure.md";

/// Write the protocol repair brief so the next `execute()` can read which
/// structural checks failed and complete the charter's protocol. Best-effort:
/// a write failure means the agent retries blind, no worse than not bouncing.
pub(crate) fn write_protocol_failure(
    ws: &Workspace,
    stamp: &str,
    report: &protocol::ProtocolReport,
    done_signal: &str,
) {
    let path = ws.ralphy_dir().join(PROTOCOL_FAILURE_FILE);
    if let Err(e) = std::fs::write(&path, protocol::failure_brief(stamp, report, done_signal)) {
        warn!(error = %e, "writing the protocol-failure repair brief failed");
    }
}

/// Remove the protocol repair brief. Called at each issue's start and once the
/// lint is settled, so a stale brief never steers a later session.
pub(crate) fn clear_protocol_failure(ws: &Workspace) {
    let path = ws.ralphy_dir().join(PROTOCOL_FAILURE_FILE);
    if path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            warn!(error = %e, "removing the stale protocol-failure brief failed");
        }
    }
}

/// A one-line digest of a failed gate for the skip log/artifact: the failing
/// command and why it failed (exit code or timeout).
pub(crate) fn verify_failure_summary(report: &verify::VerifyReport) -> String {
    match report.commands.iter().find(|c| !c.passed()) {
        Some(c) if c.timed_out => format!("`{}` timed out", c.argv.join(" ")),
        Some(c) => format!(
            "`{}` exited {}",
            c.argv.join(" "),
            c.exit_code
                .map(|n| n.to_string())
                .unwrap_or_else(|| "non-zero".into())
        ),
        None => "verify gate failed".into(),
    }
}

/// A one-line digest for a gate that could not spawn its deciding command (#182):
/// name the command and that it could not be spawned, so the skip line reads as a
/// spec/spawn problem (a typo'd binary, a missing tool) — never a test failure the
/// agent could have repaired.
pub(crate) fn verify_spawn_failure_summary(report: &verify::VerifyReport) -> String {
    match report.first_failure() {
        Some(c) => format!(
            "`{}` could not be spawned (program not found)",
            c.argv.join(" ")
        ),
        None => "verify gate command could not be spawned".into(),
    }
}

/// Refresh `.ralphy/handoffs.md` for the issue about to be planned: collect the
/// handoff comments its closed blockers left, render them, and write the file —
/// or remove a stale one when there is nothing to feed. Best-effort: a fetch
/// failure logs a warning and skips that blocker, never stopping the run.
pub(crate) fn write_handoffs(
    ws: &Workspace,
    number: u64,
    closed_blockers: &[u64],
    tracker: &dyn IssueTracker,
) {
    let mut entries: Vec<(u64, String)> = Vec::new();
    for &n in closed_blockers {
        match tracker.handoff_comment(n) {
            Ok(Some(h)) => entries.push((n, h)),
            Ok(None) => {}
            Err(e) => warn!(number, blocker = n, error = %e, "fetching handoff failed — skipping"),
        }
    }
    let path = ws.handoffs_path();
    match handoff::render_handoffs_file(&entries) {
        Some(content) => {
            if let Err(e) = std::fs::write(&path, content) {
                warn!(number, error = %e, "writing .ralphy/handoffs.md failed");
            } else {
                info!(
                    number,
                    handoffs = entries.len(),
                    "handoffs collected for planner"
                );
            }
        }
        None => {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Refresh `.ralphy/references.md` for the issue about to be planned: fetch the
/// source (title, state, body) of every issue the body references — its
/// `## Blocked by` and `## Parent` sections plus any inline `#N` mention — and
/// render them, or remove a stale file when the issue names none. The planner
/// reads this so a `#N` reference reaches it as the referenced issue's actual
/// spec, not a paraphrase it might restate as fact in a child issue. Best-effort
/// and depth-1: a fetch failure (a cross-repo or deleted ref, say) logs a warning
/// and skips that ref, and the fetched bodies' own references are not followed
/// transitively.
pub(crate) fn write_references(ws: &Workspace, issue: &Issue, tracker: &dyn IssueTracker) {
    let refs = blocked::referenced_issues(&issue.body, issue.number);
    let mut entries: Vec<references::Reference> = Vec::new();
    for n in refs {
        match tracker.reference(n) {
            Ok(Some(r)) => entries.push(r),
            Ok(None) => {}
            Err(e) => {
                warn!(number = issue.number, reference = n, error = %e, "fetching referenced issue failed — skipping")
            }
        }
    }
    let path = ws.references_path();
    match references::render_references_file(&entries) {
        Some(content) => {
            if let Err(e) = std::fs::write(&path, content) {
                warn!(number = issue.number, error = %e, "writing .ralphy/references.md failed");
            } else {
                info!(
                    number = issue.number,
                    references = entries.len(),
                    "references collected for planner"
                );
            }
        }
        None => {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Persist the durable knowledge a green close leaves behind: the environment
/// facts and working commands extracted from the plan's `## Handoff`, written
/// to `.ralphy/knowledge/issue-<N>.md`. The folder accumulates across issues
/// and runs (never cleared), so any future session — sibling or dependent —
/// can grep it instead of re-deriving an environment procedure. Best-effort:
/// a write failure logs a warning, never stopping the run.
pub(crate) fn write_knowledge(ws: &Workspace, issue: &Issue, stamp: &str, note: &str) {
    let dir = ws.knowledge_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn!(number = issue.number, error = %e, "creating .ralphy/knowledge failed");
        return;
    }
    let content = format!(
        "# Knowledge from #{}: {}\n\nExtracted {} (run {}) from the session's \
         handoff at close. Leads, not truths — verify before relying on one.\n\n{}\n",
        issue.number,
        issue.title,
        chrono::Local::now().format("%Y-%m-%d"),
        stamp,
        note.trim_end(),
    );
    let path = ws.knowledge_path(issue.number);
    if let Err(e) = std::fs::write(&path, content) {
        warn!(number = issue.number, error = %e, "writing knowledge note failed");
    } else {
        info!(number = issue.number, path = %path.display(), "knowledge note written");
    }
}

/// Append the close's `**Knowledge used**` citations to the hit-rate log at
/// `.ralphy/knowledge/citations.jsonl` — the input the consolidation curator
/// prunes never-cited `KNOWLEDGE.md` bullets against. An empty list (an honest
/// `none`) is recorded too: it is the denominator of the pruning window.
/// Best-effort like `write_knowledge`: a failure warns, never stops the run.
pub(crate) fn record_citations(ws: &Workspace, issue: &Issue, stamp: &str, citations: Vec<String>) {
    let entry = knowledge::CitationEntry {
        issue: issue.number,
        stamp: stamp.to_string(),
        date: chrono::Local::now().format("%Y-%m-%d").to_string(),
        citations,
    };
    if let Err(e) = knowledge::append_citation(ws, &entry) {
        warn!(number = issue.number, error = %e, "appending citation entry failed");
    } else {
        info!(
            number = issue.number,
            citations = entry.citations.len(),
            "knowledge citations recorded"
        );
    }
}

/// Write the issue the planner reads to `.ralphy/issue.json`.
pub(crate) fn write_issue_json(ws: &Workspace, issue: &Issue) -> Result<()> {
    std::fs::create_dir_all(ws.ralphy_dir())?;
    let json = serde_json::to_string_pretty(issue).context("serializing issue to JSON")?;
    std::fs::write(ws.issue_json_path(), json).context("writing .ralphy/issue.json")?;
    Ok(())
}
