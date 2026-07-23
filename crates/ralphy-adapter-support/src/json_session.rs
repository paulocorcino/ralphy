//! The one-shot **JSON session runner** shared by every adapter's `diagnose_repo`
//! / `draft_issues` (ADR-0012 stages 2 & 8). Each of those six functions built a
//! vendor command, ran it headless, wrote the combined log, bailed on
//! auth/timeout, then read the artifact and validated it against a core schema —
//! the same mechanical tail every time. [`run_json_session`] owns that tail.
//!
//! It stays vendor- and core-neutral (ADR-0002): the adapter passes the already
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

/// The vendor-specific inputs to [`run_text_session`]: like [`JsonSession`] but
/// with **no `out_path`/`missing_msg`** — the session's deliverable is the log
/// itself, not a separate JSON artifact (Claude's `consolidate_knowledge`).
pub struct TextSession<'a> {
    /// The fully built child command (stdin/stdout/stderr already piped).
    pub cmd: Command,
    /// The prompt piped on the child's stdin.
    pub prompt: &'a str,
    /// The wall-clock budget for the session.
    pub timeout: Duration,
    /// Where the combined stdout+stderr log is written (and referenced in errors).
    pub log_path: &'a Path,
    /// Context for a spawn failure, e.g. "failed to spawn the `claude` CLI (…)".
    pub spawn_err: &'a str,
    /// Message when the auth detector fires, e.g. the adapter's `*_AUTH_ERROR_MSG`.
    pub auth_msg: &'a str,
    /// Message prefix when the session times out, e.g.
    /// "consolidation session hit the wall timeout".
    pub timeout_msg: &'a str,
}

/// Run a one-shot headless session that is expected to write a JSON artifact, and
/// return the adapter's validated core type.
///
/// The mechanical tail, identical across all six `diagnose_repo`/`draft_issues`
/// functions: run the shared spawn→log→auth-bail→timeout-bail body via
/// [`run_text_session`], read the artifact at `out_path`, then hand the raw text to
/// `validate`. The vendor decides what markers signal auth (`auth_error`) and how
/// to parse the artifact (`validate`); this runner owns no schema and produces no
/// `Outcome`.
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

    // Reuse the text session's spawn→persist→auth-bail→timeout-bail body; the JSON
    // session's deliverable is the artifact at `out_path`, so the returned log is
    // intentionally dropped here.
    run_text_session(
        TextSession {
            cmd,
            prompt,
            timeout,
            log_path,
            spawn_err,
            auth_msg,
            timeout_msg,
        },
        auth_error,
    )?;

    let raw = fs::read_to_string(out_path).with_context(|| {
        format!(
            "{} at {} (see {})",
            missing_msg,
            out_path.display(),
            log_path.display()
        )
    })?;
    // A vendor CLI on Windows may write the artifact UTF-8-BOM-prefixed (the
    // PowerShell default); `read_to_string` keeps the BOM as a leading `\u{feff}`,
    // which `serde_json` then rejects with "expected value at line 1 column 1".
    // Strip it before the adapter's parse so a BOM never masquerades as a schema
    // mismatch (mirrors the BOM guard in `github::attachments`).
    validate(strip_bom(&raw))
}

/// Strip a single leading UTF-8 BOM (`\u{feff}`) from a decoded artifact. A BOM is
/// not whitespace, so `trim_start` misses it; left in place it makes `serde_json`
/// fail at "line 1 column 1". Returns the input unchanged when no BOM is present.
///
/// `pub` so an adapter that reads its own artifact outside [`run_json_session`]
/// (Gemini's one-shots, whose ladder is exit-code-first and cannot go through the
/// shared tail) shares this guard rather than growing a second copy of it.
pub fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{feff}').unwrap_or(s)
}

/// [`run_json_session`] with the per-session **init preamble** the six init/triage
/// wrappers each hand-rolled: ensure the `out_path` and `log_path` parent dirs
/// exist, then remove any stale `out_path` so a prior run's artifact can never
/// masquerade as this session's output. Folds that boilerplate into one place;
/// the adapter passes the same `JsonSession`/`is_auth_error`/`validate` as before.
pub fn run_init_session<T>(
    session: JsonSession<'_>,
    is_auth_error: impl Fn(&str) -> bool,
    validate: impl Fn(&str) -> Result<T>,
) -> Result<T> {
    if let Some(parent) = session.out_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    if let Some(parent) = session.log_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let _ = fs::remove_file(session.out_path);
    run_json_session(session, is_auth_error, validate)
}

/// Sibling of [`run_json_session`] for one-shots that produce **no JSON artifact**
/// (Claude's `consolidate_knowledge`): spawn via [`run_headless`], combine
/// stdout+stderr into the log and persist it at `log_path`, `bail!` on an auth
/// failure (`auth_error(&log)`), then `bail!` on a wall timeout — in that exact
/// order — and return the combined log verbatim on success. This is the shared
/// body [`run_json_session`] layers its read+validate onto.
pub fn run_text_session(
    session: TextSession<'_>,
    auth_error: impl Fn(&str) -> bool,
) -> Result<String> {
    let TextSession {
        cmd,
        prompt,
        timeout,
        log_path,
        spawn_err,
        auth_msg,
        timeout_msg,
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
    Ok(log)
}

#[cfg(test)]
mod tests {
    use super::strip_bom;

    #[test]
    fn strip_bom_removes_single_leading_bom() {
        assert_eq!(strip_bom("\u{feff}{\"ok\":true}"), "{\"ok\":true}");
    }

    #[test]
    fn strip_bom_passthrough_without_bom() {
        assert_eq!(strip_bom("{\"ok\":true}"), "{\"ok\":true}");
        // Only a *leading* BOM is stripped; one mid-string is left untouched.
        assert_eq!(strip_bom("{}\u{feff}"), "{}\u{feff}");
    }

    #[test]
    fn bom_prefixed_diagnosis_json_parses_after_strip() {
        // The exact issue #133 shape: a Windows-authored diagnosis.json led by a
        // UTF-8 BOM. Raw parse fails at "line 1 column 1"; stripping fixes it.
        let raw = "\u{feff}{\"repo_kind\":\"existing\"}";
        assert!(
            serde_json::from_str::<serde_json::Value>(raw).is_err(),
            "BOM must break the raw parse (the #133 symptom)"
        );
        let parsed: serde_json::Value = serde_json::from_str(strip_bom(raw)).unwrap();
        assert_eq!(parsed["repo_kind"], "existing");
    }
}
