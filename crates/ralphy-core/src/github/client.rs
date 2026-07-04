//! `gh` command plumbing: spawning the CLI rooted at a repo, and the
//! transient-failure retry wrapper every subsystem call routes through.

use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

use anyhow::{bail, Context, Result};

/// A `gh` command rooted at `repo_root`. Ralphy is a global tool driven with
/// `--repo`, so the process cwd need not be the target repo; `gh` resolves the
/// repository from its working directory, so every GitHub call must be pinned to
/// `repo_root` or it would silently target the wrong repo (or none).
pub(crate) fn gh(repo_root: &Path) -> Command {
    let mut cmd = Command::new("gh");
    cmd.current_dir(repo_root);
    cmd
}

/// Total attempts for a transient-failing `gh` call (1 initial + 3 retries).
pub(crate) const GH_MAX_ATTEMPTS: u32 = 4;

/// Is a `gh` failure a transient GitHub edge / network blip (worth retrying)
/// rather than a real rejection (bad label, missing issue, auth — never retry)?
///
/// GitHub's gateway answers an overloaded request with a 5xx HTML page —
/// e.g. `non-200 OK status code: 504 Gateway Timeout` — which `gh` surfaces on
/// stderr. We match those markers (and the usual transport failures) so a momentary
/// blip is retried instead of aborting a run whose work has already landed.
pub(crate) fn is_transient_gh_failure(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    [
        "502",
        "503",
        "504",
        "bad gateway",
        "gateway timeout",
        "service unavailable",
        "timeout",
        "timed out",
        "connection reset",
        "connection refused",
        "could not resolve host",
        "tls handshake",
        "eof",
    ]
    .iter()
    .any(|m| s.contains(m))
}

/// Run a `gh` invocation (built fresh by `build` each attempt — `Command` is not
/// reusable) and return its captured output, retrying on a transient failure with
/// exponential backoff. `op` labels the call in the final error.
///
/// Every call routed through here is idempotent enough that a retried duplicate is
/// harmless next to losing the run: closing an already-closed issue, re-applying a
/// label, re-setting a body, or (worst case) a duplicate evidence comment after a
/// 504 whose write actually landed. A real rejection is not transient, so it bails
/// on the first attempt — no added latency on genuine errors.
pub(crate) fn gh_output(op: &str, mut build: impl FnMut() -> Command) -> Result<Output> {
    let mut backoff = Duration::from_secs(1);
    for attempt in 1..=GH_MAX_ATTEMPTS {
        let out = build()
            .output()
            .context("failed to spawn `gh` (is the GitHub CLI installed and on PATH?)")?;
        if out.status.success() {
            return Ok(out);
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        if attempt < GH_MAX_ATTEMPTS && is_transient_gh_failure(&stderr) {
            std::thread::sleep(backoff);
            backoff *= 2;
            continue;
        }
        bail!("`{op}` failed: {}", stderr.trim());
    }
    bail!("`{op}` exhausted {GH_MAX_ATTEMPTS} attempts");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_detector_matches_the_observed_504() {
        // The exact gateway response that aborted a run mid-evidence-comment.
        let stderr = r#"failed to run git: non-200 OK status code: 504 Gateway Timeout body: "<!DOCTYPE html>...""#;
        assert!(is_transient_gh_failure(stderr));
    }

    #[test]
    fn transient_detector_matches_other_edge_and_transport_blips() {
        for s in [
            "non-200 OK status code: 502 Bad Gateway",
            "503 Service Unavailable",
            "request timed out",
            "connection reset by peer",
            "could not resolve host: api.github.com",
        ] {
            assert!(is_transient_gh_failure(s), "expected transient: {s}");
        }
    }

    #[test]
    fn transient_detector_rejects_real_rejections() {
        // Real failures must bail on the first attempt — no pointless retries.
        for s in [
            "could not add label: 'needs-split' not found",
            "GraphQL: Could not resolve to an Issue with the number of 9999",
            "gh: Not Found (HTTP 404)",
            "authentication required",
        ] {
            assert!(!is_transient_gh_failure(s), "expected non-transient: {s}");
        }
    }
}
