//! Small facts `run_cmd` derives from the repo and the settings.

use ralphy_core::Workspace;
use tracing::{info, warn};

/// The run's plan path relative to the repo root, with forward slashes — the form
/// the browser hands straight to the confined `file.read` verb (ADR-0047 §5), which
/// rejects an absolute path and a backslash-separated one alike.
pub(super) fn repo_relative_plan_path(repo_root: &std::path::Path, ws: &Workspace) -> String {
    let plan = ws.plan_path();
    plan.strip_prefix(repo_root)
        .unwrap_or(&plan)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The verify gate's time budget in minutes: the persisted `verify.timeout_minutes`,
/// else [`ralphy_core::VERIFY_GATE_FALLBACK_MINUTES`].
///
/// Split out of the `QueueConfig` literal so the "the gate never inherits the
/// per-issue cap" rule is a testable unit rather than an inline expression
/// (docs/adr/0038). Unlike the per-issue cap, `0` is not a sentinel here — a
/// gate with no timeout is not a thing the runner can enforce.
pub(super) fn resolve_verify_timeout_minutes(persisted: Option<u64>) -> u64 {
    persisted
        .filter(|m| *m > 0)
        .unwrap_or(ralphy_core::VERIFY_GATE_FALLBACK_MINUTES)
}

/// Keep the newest `runs.keep` run folders and remove the older ones. A refused
/// delete is a warning: pruning never fails a run.
pub(super) fn prune_old_run_folders(
    ws: &Workspace,
    settings: &ralphy_core::Settings,
    run_dir: &std::path::Path,
    dry_run: bool,
) {
    let keep = settings.runs.keep.unwrap_or(ralphy_core::DEFAULT_RUNS_KEEP);
    let pruned =
        ralphy_core::run_dirs::prune_run_dirs(&ws.runs_dir(), keep, run_dir, dry_run, |p| {
            std::fs::remove_dir_all(p)
        });
    if !pruned.removed.is_empty() {
        let verb = if dry_run { "would remove" } else { "removed" };
        info!(
            "{verb} {} old run folders (keeping the last {keep}; change with runs.keep)",
            pruned.removed.len()
        );
    }
    for (path, e) in &pruned.failed {
        warn!(path = %path.display(), error = %e, "could not remove an old run folder");
    }
}
