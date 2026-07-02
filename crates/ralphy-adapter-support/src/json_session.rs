//! The one-shot **JSON session runner** shared by every adapter's `diagnose_repo`
//! / `draft_issues` (ADR-0012 stages 2 & 8). Each of those six functions built a
//! vendor command, ran it headless, wrote the combined log, bailed on
//! auth/timeout, then read the artifact and validated it against a core schema —
//! the same mechanical tail every time. [`run_json_session`] owns that tail.
//!
//! It stays vendor- and core-neutral (ADR-0004): the adapter passes the already
//! built [`Command`], its own auth detector, the exact error wording, and a
//! `validate` closure that parses the raw artifact into whatever core type it
//! returns. The `serde_json` deserialization and the `ralphy-core` schema types
//! live in the adapter's closure, so this crate needs neither.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::run_headless;

/// The vendor-specific inputs to [`run_json_session`]: the built command, the
/// prompt, paths, and the exact error wording. The `(see <log>)` suffix is
/// appended by the runner, so `auth_msg`/`timeout_msg`/`missing_msg` carry only
/// the prefix.
pub struct JsonSession<'a> {
    /// The fully built child command (stdin/stdout/stderr already piped).
    pub cmd: Command,
    /// The prompt piped on the child's stdin.
    pub prompt: &'a str,
    /// The wall-clock budget for the session.
    pub timeout: Duration,
    /// Where the combined stdout+stderr log is written (and referenced in errors).
    pub log_path: &'a Path,
    /// The JSON artifact the session is expected to write.
    pub out_path: &'a Path,
    /// Context for a spawn failure, e.g. "failed to spawn the `claude` CLI (…)".
    pub spawn_err: &'a str,
    /// Message when the auth detector fires, e.g. the adapter's `*_AUTH_ERROR_MSG`.
    pub auth_msg: &'a str,
    /// Message prefix when the session times out, e.g.
    /// "diagnosis session hit the wall timeout".
    pub timeout_msg: &'a str,
    /// Message prefix when no artifact was written, e.g.
    /// "diagnosis session left no report".
    pub missing_msg: &'a str,
}

/// Run a one-shot headless session that is expected to write a JSON artifact, and
/// return the adapter's validated core type.
///
/// The mechanical tail, identical across all six `diagnose_repo`/`draft_issues`
/// functions: spawn via [`run_headless`], combine stdout+stderr into the log and
/// persist it, `bail!` on an auth failure (`auth_error(&log)`), `bail!` on a wall
/// timeout, read the artifact at `out_path`, then hand the raw text to `validate`.
/// The vendor decides what markers signal auth (`auth_error`) and how to parse the
/// artifact (`validate`); this runner owns no schema and produces no `Outcome`.
pub fn run_json_session<T>(
    session: JsonSession<'_>,
    auth_error: impl Fn(&str) -> bool,
    validate: impl Fn(&str) -> Result<T>,
) -> Result<T> {
    let JsonSession {
        cmd,
        prompt,
        timeout,
        log_path,
        out_path,
        spawn_err,
        auth_msg,
        timeout_msg,
        missing_msg,
    } = session;

    let r = run_headless(cmd, prompt, timeout).with_context(|| spawn_err.to_string())?;
    let mut log = r.stdout;
    log.push_str(&r.stderr);
    let _ = fs::write(log_path, &log);

    if auth_error(&log) {
        bail!("{} (see {})", auth_msg, log_path.display());
    }
    if r.timed_out {
        bail!("{} (see {})", timeout_msg, log_path.display());
    }

    let raw = fs::read_to_string(out_path).with_context(|| {
        format!(
            "{} at {} (see {})",
            missing_msg,
            out_path.display(),
            log_path.display()
        )
    })?;
    validate(&raw)
}
