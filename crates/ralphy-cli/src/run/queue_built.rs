//! The `queue built` telemetry of a run.

use ralphy_core::{github, GhTracker};
use tracing::warn;

use crate::cli::RunArgs;

/// Emit the ADR-0020/-0021 `queue built` telemetry: enrich it with the per-issue
/// snapshot the runner would judge (`data.issues[]`) and mark the applied assignee
/// scope, terminating in the single stable `emit::queue_built` the notifier /
/// presenter consume. Both resolutions are best-effort telemetry — a `gh` blip warns
/// and emits the legacy/unmarked shape rather than aborting the run. The caller
/// positions this after the header/info-line prints and before the notifier worker
/// start, so the buffered-ring drain order is unchanged.
#[expect(
    clippy::too_many_arguments,
    reason = "the arguments are the separate facts of the queue that the event reports"
)]
pub(super) fn emit_queue_built(
    queue: &[ralphy_core::Issue],
    forced_issues: &[u64],
    human_return_labels: &[String],
    repo_root: &std::path::Path,
    args: &RunArgs,
    assignee: Option<&str>,
    order: &[String],
    stop_before: u64,
    scope: &str,
) {
    let issues_json = {
        let tracker = GhTracker::new(repo_root);
        match ralphy_core::resolve_queue_view(queue, forced_issues, human_return_labels, &tracker) {
            Ok(view) => serde_json::to_string(&view.issues).unwrap_or_default(),
            Err(e) => {
                warn!(
                    error = %e,
                    "resolving the queue snapshot failed — emitting the legacy queue.built shape"
                );
                String::new()
            }
        }
    };
    // The filter is the *applied* one — `None` on an explicit `--issues`/`--only-issue`
    // selection (which bypasses the assignee filter), matching how those paths fetch
    // unfiltered; the concrete login is emitted, never the literal `@me`.
    let applied_assignee = if !args.issues.is_empty() || args.only_issue.is_some() {
        None
    } else {
        assignee.map(str::to_string)
    };
    let assignee_filter: Option<String> = match applied_assignee.as_deref() {
        Some(a) => match github::resolve_login(a, repo_root) {
            Ok(l) => Some(l),
            Err(e) => {
                warn!(
                    error = %e,
                    "resolving @me for the assignee_filter mark failed — \
                     emitting queue.built without the scope mark"
                );
                None
            }
        },
        None => None,
    };
    ralphy_core::emit::queue_built(
        queue.len() as u64,
        &order.join(" -> "),
        stop_before,
        &issues_json,
        assignee_filter.as_deref().unwrap_or(""),
        scope,
    );
}
