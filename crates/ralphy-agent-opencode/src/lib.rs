//! The OpenCode CLI adapter: drives `opencode run` behind the core [`Agent`]
//! contract. Everything OpenCode-specific — the binary, the model/variant flags,
//! the headless invocation, the line-delimited-JSON event stream, and the
//! signal→[`Outcome`] mapping — is confined here. See docs/adr/0005.
//!
//! Like the Codex adapter (and unlike Claude's live PTY session), OpenCode needs
//! no interactive session: `plan` and `execute` both run headless `opencode run`
//! with the prompt piped on stdin, and completion is detected from the
//! `RALPHY_DONE_EXIT`/`RALPHY_BLOCKED_EXIT` sentinels parsed out of the JSON
//! `text` parts, a JSON `error` event, the process exit code, and a HEAD-diff
//! commit check — mapped onto the same core [`Outcome`].
//!
//! Skills materialization (ADR-0005 D7) is implemented here: before every `plan`
//! and `execute` call the embedded skills tree is extracted to `<repo>/.ralphy/skills`
//! and the absolute path is injected as `OPENCODE_CONFIG_CONTENT` so OpenCode's
//! `skills.paths` config key points at it. Auth-error (D6) is implemented:
//! `is_opencode_auth_error` detects `ProviderAuthError` in the combined log and an
//! actionable bail fires in both `plan` and `execute` before any generic
//! classification. Usage-limit (D9) is implemented: `parse_opencode_limit` scans
//! the JSON stream for a 429/`APIError` or documented rate-limit string and
//! `classify_opencode_outcome` upgrades `Timeout`/`Stuck` to `Outcome::Limit` when
//! one is seen; `--stop-on-limit` is forced for OpenCode in `main.rs`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use include_dir::{include_dir, Dir};
use ralphy_adapter_support::{resolve_program, run_headless};
use ralphy_core::{git, plan, Agent, Issue, Outcome, Plan, Workspace};
use tracing::info;

/// The skills subtree, embedded at build time so the binary is self-contained.
/// OpenCode discovers skills via `skills.paths` in its config; we extract this
/// tree to `.ralphy/skills` and inject the path via `OPENCODE_CONFIG_CONTENT`
/// before every plan and execute call (ADR-0005 D7, mirrors Codex adapter).
static SKILLS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/plugin/skills");

/// Materialize the embedded skills into `<repo>/.ralphy/skills/` so OpenCode can
/// discover them via the injected `skills.paths` config. Clears any prior copy,
/// re-extracts fresh, and writes `<repo>/.ralphy/.gitignore` (`*`) to keep the
/// materialized tree out of executor commits. Returns the `.ralphy/skills` path.
fn materialize_opencode_skills(ws: &Workspace) -> Result<PathBuf> {
    let ralphy_dir = ws.ralphy_dir();
    let skills_dir = ralphy_dir.join("skills");
    ralphy_adapter_support::materialize_assets(&SKILLS, &skills_dir, Some(&ralphy_dir))?;
    Ok(skills_dir)
}

/// Build the JSON string injected as `OPENCODE_CONFIG_CONTENT` so OpenCode's
/// `skills.paths` points at the materialized skills container. The path is
/// canonicalized for robustness; on failure the original path is used as-is.
fn opencode_skills_config(skills_dir: &Path) -> String {
    let abs = skills_dir
        .canonicalize()
        .unwrap_or_else(|_| skills_dir.to_path_buf());
    serde_json::json!({
        "skills": {
            "paths": [abs]
        }
    })
    .to_string()
}

/// The OpenCode planning prompt, embedded so the binary is self-contained as a
/// global tool. A variant of `prompt.plan.md` with the `## Execution model` tier
/// line removed (OpenCode drops complexity routing, ADR-0005 D3/D8a) and the
/// reviewer step committed to the **inline `reviewer` skill** — auto-discovered
/// via `skills.paths`, **not** a subagent. Headless custom-subagent dispatch is
/// blocked upstream (`opencode#29616`/`#20059`: Task tool `subagent_type` is
/// hardcoded to `explore`/`general`), so the inline skill is the only working
/// headless mechanism (ADR-0005 D8). Single source of truth lives at
/// `assets/prompts/`.
const PROMPT_PLAN_OPENCODE: &str = include_str!("../../../assets/prompts/prompt.plan.opencode.md");

/// The actionable message shown when `is_opencode_auth_error` fires — tells the
/// operator exactly what to do to recover (run `opencode auth login`).
const OPENCODE_AUTH_ERROR_MSG: &str =
    "OpenCode is not authenticated (ProviderAuthError) — run `opencode auth login` and retry";

/// Return `true` when `text` (the combined stdout+stderr log) shows an OpenCode
/// authentication failure. A signed-out `opencode run` emits a `ProviderAuthError`
/// SDK error event (ADR-0005 D6). Keying on the case-insensitive substring
/// `providerautherror` is specific enough to avoid false positives from our own
/// prompt text mentioning `opencode auth login`.
fn is_opencode_auth_error(text: &str) -> bool {
    text.to_ascii_lowercase().contains("providerautherror")
}

/// The vendor-neutral execution charter, piped to `opencode run` on stdin. Shared
/// verbatim with the Claude and Codex paths — it already names the
/// `RALPHY_DONE_EXIT` / `RALPHY_BLOCKED_EXIT` sentinels and is not Claude-specific.
const PROMPT_EXECUTE: &str = include_str!("../../../assets/prompts/prompt.execute.md");

/// Drives the `opencode` CLI. `model` is the operator override (omitted entirely
/// when `None`, deferring to OpenCode's own resolution, ADR-0005 D4); `variant`
/// is the operator's optional effort knob, passed through only when set (D3);
/// `run_dir` is where the captured logs live; `max_minutes_per_issue` is the
/// per-issue wall budget, clamped to `run_deadline` when the run carries a global
/// deadline.
pub struct OpenCodeAgent {
    model: Option<String>,
    variant: Option<String>,
    run_dir: PathBuf,
    max_minutes_per_issue: u64,
    run_deadline: Option<Instant>,
}

impl OpenCodeAgent {
    pub fn new(model: Option<String>, run_dir: PathBuf) -> Self {
        Self {
            model,
            variant: None,
            run_dir,
            max_minutes_per_issue: 45,
            run_deadline: None,
        }
    }

    /// Set the operator's optional `--variant` knob (ADR-0005 D3). Passed through
    /// to OpenCode only when present; omitted otherwise so the adapter never
    /// sends a value the provider rejects.
    pub fn with_variant(mut self, variant: Option<String>) -> Self {
        self.variant = variant;
        self
    }

    /// Set the run's global wall-clock deadline (from `--deadline-hours`). Each
    /// issue's budget is then clamped to it, so an issue started just under the
    /// global limit can't overrun by a whole per-issue window (mirrors
    /// `CodexAgent::with_run_deadline`).
    pub fn with_run_deadline(mut self, run_deadline: Option<Instant>) -> Self {
        self.run_deadline = run_deadline;
        self
    }

    /// The deadline for the current issue: the per-issue budget, clamped to the
    /// run's global deadline when one is set.
    fn issue_deadline(&self) -> Instant {
        let per_issue = Instant::now() + Duration::from_secs(self.max_minutes_per_issue * 60);
        match self.run_deadline {
            Some(rd) => per_issue.min(rd),
            None => per_issue,
        }
    }
}

/// Build the headless `opencode run` command both `plan` and `execute` go through
/// — the single point that fixes the invocation, always passes
/// `--dangerously-skip-permissions` (the headless-hang guard, ADR-0005 D5) and
/// `--format json`, omits `-m` unless the operator set one (D4), passes
/// `--variant` only when set (D3), injects `OPENCODE_CONFIG_CONTENT` with the
/// skills path (D7), runs in the repo root, and defensively removes both
/// `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` so an inherited key can't switch
/// the run to metered API billing (D6). The prompt is written on stdin.
fn build_opencode_command(
    model: Option<&str>,
    variant: Option<&str>,
    root: &Path,
    skills_config: &str,
) -> Command {
    // Resolve `opencode` to its real path: on Windows it ships as an npm `.cmd`
    // shim with no `.exe`, which a bare `Command::new("opencode")` cannot find.
    let mut cmd = Command::new(resolve_program("opencode"));
    cmd.arg("run")
        .arg("--format")
        .arg("json")
        .arg("--dangerously-skip-permissions");
    if let Some(m) = model {
        cmd.arg("-m").arg(m);
    }
    if let Some(v) = variant {
        cmd.arg("--variant").arg(v);
    }
    cmd.current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("OPENCODE_CONFIG_CONTENT", skills_config)
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY");
    cmd
}

/// The payload object a single event's fields live in. opencode 1.16.2 wraps
/// every event in an envelope `{type, timestamp, sessionID, part:{…}}` and puts
/// the real fields (`text`, `tool`, `reason`, …) under `part`; the older/SDK
/// shape this adapter was first written against is flat (the fields sit at the
/// top level). Returning `part` when present and the value itself otherwise lets
/// every parser read fields from one place and stay correct under both shapes
/// (ADR-0005 D2 — the exact event JSON, observed live against opencode 1.16.2).
fn event_payload(val: &serde_json::Value) -> &serde_json::Value {
    val.get("part").unwrap_or(val)
}

/// Whether an event (envelope `val`) is an error event under any observed shape:
/// the flat `type:"error"`, a `part:{type:"error"}`, or the opencode 1.16.2
/// envelope that carries a top-level `error` object (`{type:"error", error:{…}}`).
fn is_error_event(val: &serde_json::Value) -> bool {
    let top = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let inner = event_payload(val)
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    top == "error" || inner == "error" || val.get("error").is_some()
}

/// The object an error event's `name`/`statusCode`/`message`/`retryAfter` fields
/// live in. opencode 1.16.2 nests them as `error.data` under a top-level `error`
/// object (`{type:"error", error:{name, data:{message, …}}}`, captured live); the
/// flat/SDK shape puts them directly on the payload. Returns the most specific
/// object available so the limit matcher and reset parser read fields from one
/// place under either shape (ADR-0005 D6/D9 — exact error JSON, observed live).
fn error_detail(val: &serde_json::Value) -> &serde_json::Value {
    if let Some(err) = val.get("error") {
        return err.get("data").unwrap_or(err);
    }
    event_payload(val)
}

/// The error event's `name`, reading `error.name` (opencode 1.16.2) before the
/// flat `name` on the payload.
fn error_name(val: &serde_json::Value) -> &str {
    val.get("error")
        .and_then(|e| e.get("name"))
        .or_else(|| event_payload(val).get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// Parse OpenCode's `--format json` line-delimited event stream: concatenate the
/// assistant `text` parts into the returned string (the source the sentinel scan
/// reads) and set the bool when any line is an `error` event. Tolerant of
/// unparseable lines (skipped). Reads the text from the event's `part` payload
/// (opencode 1.16.2) and falls back to the top level (flat shape), so the
/// sentinel scan sees the assistant's real output under both envelopes.
fn parse_opencode_events(stdout: &str) -> (String, bool) {
    let mut text = String::new();
    let mut saw_error = false;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue; // tolerate non-JSON noise on the stream
        };
        if is_error_event(&val) {
            saw_error = true;
        }
        let payload = event_payload(&val);
        let is_text = val.get("type").and_then(|v| v.as_str()) == Some("text")
            || payload.get("type").and_then(|v| v.as_str()) == Some("text");
        if is_text {
            if let Some(t) = payload.get("text").and_then(|v| v.as_str()) {
                text.push_str(t);
                text.push('\n');
            }
        }
    }
    (text, saw_error)
}

/// Extract a reset hint from an OpenCode error event or message text (best-effort).
/// Looks for a `retryAfter` field value, or a `Retry-After` / "try again" substring
/// in the message. Returns `None` when absent (D9: reset hint is not guaranteed).
fn parse_opencode_reset_hint(event: &serde_json::Value) -> Option<String> {
    // retryAfter field on the event object.
    if let Some(v) = event.get("retryAfter") {
        let s = match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        if !s.is_empty() && s != "null" {
            return Some(s);
        }
    }
    // "try again" or "Retry-After" in the message text.
    if let Some(msg) = event.get("message").and_then(|v| v.as_str()) {
        let lower = msg.to_ascii_lowercase();
        // "retry-after: <value>"
        if let Some(pos) = lower.find("retry-after:") {
            let rest = msg[pos + "retry-after:".len()..].trim();
            let hint = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches(',');
            if !hint.is_empty() {
                return Some(hint.to_string());
            }
        }
        // "try again at <value>" or "try again in <value>"
        for prefix in &["try again at ", "try again in "] {
            if let Some(pos) = lower.find(prefix) {
                let rest = msg[pos + prefix.len()..].trim();
                let hint: String = rest
                    .chars()
                    .take_while(|c| *c != '.' && *c != '\n')
                    .collect();
                let hint = hint.trim().to_string();
                if !hint.is_empty() {
                    return Some(hint);
                }
            }
        }
    }
    None
}

/// Scan the line-delimited JSON event stream for a usage-limit signal (ADR-0005 D9).
///
/// Returns:
/// - `Some(Some(hint))` — a limit event was seen and carries a reset hint.
/// - `Some(None)` — a limit event was seen but no reset hint was found.
/// - `None` — no limit event was seen.
///
/// Detects three documented shapes:
/// 1. `name:"APIError"` + `statusCode:429` (the SDK's rate-limit error).
/// 2. Literal rate-limit strings from OpenCode's `retryable()` function
///    (`retry.ts`): "rate_limit_error", "rate limit exceeded", "too many requests",
///    "quota exceeded".
/// 3. Zen provider `*UsageLimitError` name suffix.
fn parse_opencode_limit(stdout: &str) -> Option<Option<String>> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        if !is_error_event(&val) {
            continue;
        }
        // Read the error fields from wherever this shape carries them: `error.data`
        // (opencode 1.16.2), `error`, or the flat payload (`part`/top level).
        let name = error_name(&val);
        let detail = error_detail(&val);
        let status = detail
            .get("statusCode")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let msg = detail
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        let is_limit = (name == "APIError" && status == 429)
            || name.ends_with("UsageLimitError")
            || msg.contains("rate_limit_error")
            || msg.contains("rate limit exceeded")
            || msg.contains("too many requests")
            || msg.contains("quota exceeded");

        if is_limit {
            return Some(parse_opencode_reset_hint(detail));
        }
    }
    None
}

/// Map an execution call's end state onto a core [`Outcome`] (ADR-0005 D2): the
/// wall timeout wins, but a `limit` event (D9) upgrades `Timeout` to
/// `Outcome::Limit(reset)` and the `Stuck` fallthrough to `Outcome::Limit` when
/// present; a `RALPHY_BLOCKED_EXIT <reason>` sentinel is `Blocked(reason)`; a
/// clean exit that committed, saw no `error` event, and emitted `RALPHY_DONE_EXIT`
/// is `Done`; anything else is `Stuck`. The HEAD-diff `committed` check is the
/// progress guard the Claude headless loop and the Codex adapter already use:
/// OpenCode makes internal snapshots, not git commits, so a `Done` claim with no
/// commit is distrusted and downgraded.
fn classify_opencode_outcome(
    exited_cleanly: bool,
    timed_out: bool,
    committed: bool,
    text: &str,
    saw_error: bool,
    limit: Option<Option<String>>,
) -> Outcome {
    if timed_out {
        return limit.map(Outcome::Limit).unwrap_or(Outcome::Timeout);
    }
    if let Some(reason) = ralphy_adapter_support::blocked_reason(text) {
        return Outcome::Blocked(reason);
    }
    if exited_cleanly && committed && !saw_error && ralphy_adapter_support::done_sentinel(text) {
        return Outcome::Done;
    }
    if let Some(reset) = limit {
        return Outcome::Limit(reset);
    }
    Outcome::Stuck
}

impl OpenCodeAgent {
    /// Spawn a single `opencode run` call, piping `prompt` on stdin and draining
    /// stdout/stderr via reader threads to avoid pipe-buffer deadlock (mirrors
    /// `CodexAgent::run_codex`). Polls `try_wait` until `timeout`; kills the child
    /// on expiry. Returns `(exited_cleanly, timed_out, stdout_text, log)` — stdout
    /// is the JSON event stream the caller parses; `log` is the combined
    /// stdout+stderr written to `run_dir/opencode.log` and used by the auth
    /// detector (auth failures often print only to stderr).
    fn run_opencode(
        &self,
        cmd: Command,
        prompt: &str,
        timeout: Duration,
    ) -> Result<(bool, bool, String, String)> {
        // Delegate the OS-level spawn/drain/poll/kill/collect plumbing to the
        // shared headless runner; `exited_cleanly` is a *successful* exit (the
        // status is `None` exactly when the child was killed on the wall timeout).
        let r = run_headless(cmd, prompt, timeout)
            .context("failed to spawn the `opencode` CLI (is it installed and on PATH?)")?;

        let stdout_text = r.stdout;
        // The combined log keeps stderr too — the JSON stream lives on stdout, but
        // a crash or auth failure often only prints to stderr.
        let mut log = stdout_text.clone();
        log.push_str(&r.stderr);
        let _ = fs::write(self.run_dir.join("opencode.log"), &log);

        let exited_cleanly = r.exit.map(|s| s.success()).unwrap_or(false);
        Ok((exited_cleanly, r.timed_out, stdout_text, log))
    }
}

impl Agent for OpenCodeAgent {
    fn plan(&self, _issue: &Issue, ws: &Workspace) -> Result<Plan> {
        fs::create_dir_all(ws.ralphy_dir()).ok();
        fs::create_dir_all(&self.run_dir).ok();
        let skills_dir = materialize_opencode_skills(ws)?;
        let skills_config = opencode_skills_config(&skills_dir);

        let plan_path = ws.plan_path();
        // Plan fresh every run; never reuse a stale artifact.
        let _ = fs::remove_file(&plan_path);

        let cmd = build_opencode_command(
            self.model.as_deref(),
            self.variant.as_deref(),
            ws.repo_root(),
            &skills_config,
        );
        let timeout = self
            .issue_deadline()
            .saturating_duration_since(Instant::now());
        info!(model = ?self.model, variant = ?self.variant, "planning with opencode run");
        let (_, _, _, log) = self.run_opencode(cmd, PROMPT_PLAN_OPENCODE, timeout)?;

        if !plan_path.exists() {
            if is_opencode_auth_error(&log) {
                bail!(
                    "{OPENCODE_AUTH_ERROR_MSG} (see {})",
                    self.run_dir.join("opencode.log").display()
                );
            }
            bail!(
                "opencode produced no plan at {} (see {})",
                plan_path.display(),
                self.run_dir.join("opencode.log").display()
            );
        }
        let md = fs::read_to_string(&plan_path).context("reading the written plan.md")?;
        Ok(Plan {
            open_steps: plan::count_open_steps(&md),
            // OpenCode drops complexity routing (ADR-0005 D3), so there is no tier.
            recommended_model: None,
            path: plan_path,
        })
    }

    fn execute(&self, _plan: &Plan, ws: &Workspace) -> Result<Outcome> {
        fs::create_dir_all(&self.run_dir).ok();
        fs::create_dir_all(ws.ralphy_dir()).ok();
        let skills_dir = materialize_opencode_skills(ws)?;
        let skills_config = opencode_skills_config(&skills_dir);

        // HEAD before/after bounds the work this call committed (progress guard).
        let before_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        let timeout = self
            .issue_deadline()
            .saturating_duration_since(Instant::now());
        let cmd = build_opencode_command(
            self.model.as_deref(),
            self.variant.as_deref(),
            ws.repo_root(),
            &skills_config,
        );
        info!(model = ?self.model, variant = ?self.variant, "executing with opencode run");
        let (exited_cleanly, timed_out, stdout_text, log) =
            self.run_opencode(cmd, PROMPT_EXECUTE, timeout)?;

        // A signed-out account never makes progress: stop the run with an
        // actionable message rather than letting it fall through to Stuck/Timeout.
        if is_opencode_auth_error(&log) {
            bail!(
                "{OPENCODE_AUTH_ERROR_MSG} (see {})",
                self.run_dir.join("opencode.log").display()
            );
        }

        let after_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        let committed = before_sha != after_sha;
        let (text, saw_error) = parse_opencode_events(&stdout_text);
        let limit = parse_opencode_limit(&stdout_text);

        let outcome = classify_opencode_outcome(
            exited_cleanly,
            timed_out,
            committed,
            &text,
            saw_error,
            limit,
        );
        info!(
            ?outcome,
            exited_cleanly, timed_out, committed, saw_error, "opencode execution ended"
        );
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ── is_opencode_auth_error ──────────────────────────────────────────────

    #[test]
    fn is_opencode_auth_error_matches_captured_provider_auth_error() {
        // Representative captured log from a signed-out `opencode run`: the SDK
        // emits a `ProviderAuthError` event name in the JSON error event and may
        // also print it to stderr. Either occurrence triggers the detector.
        let json_event =
            r#"{"type":"error","name":"ProviderAuthError","message":"Not authenticated"}"#;
        assert!(
            is_opencode_auth_error(json_event),
            "must match a ProviderAuthError JSON event"
        );

        // Mixed log with stderr text (case-insensitive check).
        let mixed_log = "some init output\nError: ProviderAuthError: not signed in\n";
        assert!(
            is_opencode_auth_error(mixed_log),
            "must match ProviderAuthError in stderr text"
        );

        // Upper-cased variant — to_ascii_lowercase makes it case-insensitive.
        assert!(
            is_opencode_auth_error("PROVIDERAUTHERROR"),
            "must be case-insensitive"
        );
    }

    #[test]
    fn is_opencode_auth_error_ignores_unrelated_text() {
        assert!(
            !is_opencode_auth_error("all steps green\nRALPHY_DONE_EXIT\n"),
            "must not match a clean DONE sentinel"
        );
        assert!(
            !is_opencode_auth_error("timeout waiting for response"),
            "must not match an unrelated error"
        );
        assert!(!is_opencode_auth_error(""), "must not match empty text");
    }

    #[test]
    fn is_opencode_auth_error_takes_precedence_over_done_sentinel() {
        // A log that carries both a ProviderAuthError and a RALPHY_DONE_EXIT
        // sentinel must still be detected as an auth error — the auth signal wins.
        let log = "some work\n\
                   {\"type\":\"error\",\"name\":\"ProviderAuthError\",\"message\":\"signed out\"}\n\
                   RALPHY_DONE_EXIT\n";
        assert!(
            is_opencode_auth_error(log),
            "auth error must win over a co-present DONE sentinel"
        );
    }

    // ── classify_opencode_outcome ───────────────────────────────────────────

    #[test]
    fn classify_done_on_clean_exit_commit_and_sentinel() {
        let text = "all steps green\nRALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_opencode_outcome(true, false, true, text, false, None),
            Outcome::Done
        );
    }

    #[test]
    fn classify_stuck_on_no_commit() {
        // A DONE claim with no new commit is distrusted (HEAD-diff progress guard).
        let text = "RALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_opencode_outcome(true, false, false, text, false, None),
            Outcome::Stuck
        );
    }

    #[test]
    fn classify_blocked_on_blocked_sentinel() {
        let text = "did some work\nRALPHY_BLOCKED_EXIT missing upstream crate\n";
        assert_eq!(
            classify_opencode_outcome(true, false, true, text, false, None),
            Outcome::Blocked("missing upstream crate".into())
        );
    }

    #[test]
    fn classify_stuck_on_non_zero_exit() {
        // A non-zero exit is Stuck even when the output carries a DONE sentinel.
        let text = "RALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_opencode_outcome(false, false, true, text, false, None),
            Outcome::Stuck
        );
    }

    #[test]
    fn classify_stuck_on_error_event() {
        // A JSON `error` event downgrades an otherwise-clean DONE claim to Stuck.
        let text = "RALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_opencode_outcome(true, false, true, text, true, None),
            Outcome::Stuck
        );
    }

    #[test]
    fn classify_stuck_on_no_sentinel() {
        assert_eq!(
            classify_opencode_outcome(true, false, true, "quiet exit, no sentinel", false, None),
            Outcome::Stuck
        );
    }

    #[test]
    fn classify_timeout_wins() {
        // The wall timeout wins over everything, including a DONE sentinel.
        let text = "RALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_opencode_outcome(false, true, false, text, false, None),
            Outcome::Timeout
        );
    }

    #[test]
    fn classify_timeout_upgrades_to_limit_when_seen() {
        // A timed-out run with a limit event is upgraded to Limit(reset) (D9).
        let text = "some output\n";
        assert_eq!(
            classify_opencode_outcome(
                false,
                true,
                false,
                text,
                false,
                Some(Some("2026-06-10T18:00:00Z".into()))
            ),
            Outcome::Limit(Some("2026-06-10T18:00:00Z".into()))
        );
    }

    #[test]
    fn classify_timeout_stays_timeout_without_limit() {
        // No limit event means a hung run stays Timeout.
        let text = "some output\n";
        assert_eq!(
            classify_opencode_outcome(false, true, false, text, false, None),
            Outcome::Timeout
        );
    }

    #[test]
    fn classify_stuck_upgrades_to_limit_when_seen() {
        // A Stuck outcome is upgraded to Limit when a limit event was seen.
        let text = "no sentinel\n";
        assert_eq!(
            classify_opencode_outcome(true, false, true, text, false, Some(None)),
            Outcome::Limit(None)
        );
    }

    // ── parse_opencode_limit ─────────────────────────────────────────────────

    #[test]
    fn parse_limit_apierror_429_with_reset_hint() {
        // Representative captured JSON: APIError + statusCode:429 + retryAfter field.
        let stream = r#"{"type":"text","text":"working"}
{"type":"error","name":"APIError","statusCode":429,"message":"rate limited","retryAfter":"2026-06-10T18:00:00Z"}
"#;
        assert_eq!(
            parse_opencode_limit(stream),
            Some(Some("2026-06-10T18:00:00Z".into()))
        );
    }

    #[test]
    fn parse_limit_apierror_429_without_reset_hint() {
        // APIError + 429 but no reset hint → Some(None).
        let stream = r#"{"type":"error","name":"APIError","statusCode":429,"message":"too many requests"}
"#;
        assert_eq!(parse_opencode_limit(stream), Some(None));
    }

    #[test]
    fn parse_limit_retryable_literal_string() {
        // Documented retryable() literal: "rate limit exceeded".
        let stream = r#"{"type":"error","name":"APIError","statusCode":429,"message":"Rate limit exceeded. Try again at 2026-06-10T19:00:00Z"}
"#;
        // Should detect as limit and extract a reset hint from the message.
        let result = parse_opencode_limit(stream);
        assert!(result.is_some(), "must detect as limit: {result:?}");
        // The reset hint is extracted from "try again at <value>".
        assert_eq!(result, Some(Some("2026-06-10T19:00:00Z".into())));
    }

    #[test]
    fn parse_limit_zen_usage_limit_error() {
        // Zen provider emits a *UsageLimitError name.
        let stream = r#"{"type":"error","name":"KimiUsageLimitError","message":"usage limit reached"}
"#;
        assert!(
            parse_opencode_limit(stream).is_some(),
            "must detect Zen *UsageLimitError"
        );
    }

    #[test]
    fn parse_limit_ignores_real_unknown_error_envelope() {
        // The exact error event captured live from opencode 1.16.2: a transient
        // backend failure, NOT a usage limit. It must not be misread as a limit.
        let stream = r#"{"type":"error","timestamp":1781088576836,"sessionID":"ses_x","error":{"name":"UnknownError","data":{"message":"Unexpected server error. Check server logs for details.","ref":"err_7391de1e"}}}"#;
        assert_eq!(
            parse_opencode_limit(stream),
            None,
            "an UnknownError backend blip is not a usage limit"
        );
        // But it IS an error event (downgrades a Done claim to Stuck on execute).
        let (_t, saw_error) = parse_opencode_events(stream);
        assert!(saw_error, "the real error envelope must flag saw_error");
    }

    #[test]
    fn parse_limit_detects_429_in_real_error_data_envelope() {
        // A 429 carried in the opencode 1.16.2 envelope: name + statusCode +
        // retryAfter live under `error.data`, not at the top level.
        let stream = r#"{"type":"error","sessionID":"s","error":{"name":"APIError","data":{"statusCode":429,"message":"rate limit exceeded","retryAfter":"2026-06-10T20:00:00Z"}}}"#;
        assert_eq!(
            parse_opencode_limit(stream),
            Some(Some("2026-06-10T20:00:00Z".into())),
            "must read name/statusCode/retryAfter from error.data"
        );
    }

    #[test]
    fn parse_limit_non_limit_status_500() {
        // A 500 error must not be classified as a limit.
        let stream = r#"{"type":"error","name":"APIError","statusCode":500,"message":"internal server error"}
"#;
        assert_eq!(parse_opencode_limit(stream), None);
    }

    #[test]
    fn parse_limit_clean_stream_no_limit() {
        // A clean stream with no error events yields None.
        let stream = r#"{"type":"text","text":"working on it"}
{"type":"text","text":"RALPHY_DONE_EXIT"}
{"type":"step_finish","reason":"stop"}
"#;
        assert_eq!(parse_opencode_limit(stream), None);
    }

    // ── build_opencode_command ──────────────────────────────────────────────

    fn argv(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn build_command_omits_model_when_none() {
        let cmd = build_opencode_command(None, None, Path::new("/repo"), "{}");
        // The program is `resolve_program("opencode")`: a full path (e.g.
        // `opencode.cmd` on Windows) when found on PATH, else the bare name. Either
        // way the file stem is `opencode`.
        let program = PathBuf::from(cmd.get_program());
        assert_eq!(
            program.file_stem().and_then(|s| s.to_str()),
            Some("opencode"),
            "program: {program:?}"
        );
        let args = argv(&cmd);
        assert!(args.contains(&"run".to_string()), "argv: {args:?}");
        // No -m flag is passed; OpenCode resolves its own model (ADR-0005 D4).
        assert!(!args.contains(&"-m".to_string()), "argv: {args:?}");
        // --variant is absent unless the operator set one (D3).
        assert!(!args.contains(&"--variant".to_string()), "argv: {args:?}");
        // Always-on flags.
        assert!(
            args.contains(&"--dangerously-skip-permissions".to_string()),
            "argv: {args:?}"
        );
        assert!(args.contains(&"--format".to_string()), "argv: {args:?}");
        assert!(args.contains(&"json".to_string()), "argv: {args:?}");
    }

    #[test]
    fn build_command_includes_model_when_some() {
        let cmd = build_opencode_command(
            Some("anthropic/claude-sonnet-4-6"),
            None,
            Path::new("/repo"),
            "{}",
        );
        let args = argv(&cmd);
        assert!(args.contains(&"-m".to_string()), "argv: {args:?}");
        assert!(
            args.contains(&"anthropic/claude-sonnet-4-6".to_string()),
            "argv: {args:?}"
        );
    }

    #[test]
    fn build_command_includes_variant_only_when_some() {
        let without = build_opencode_command(None, None, Path::new("/repo"), "{}");
        assert!(!argv(&without).contains(&"--variant".to_string()));

        let with = build_opencode_command(None, Some("high"), Path::new("/repo"), "{}");
        let args = argv(&with);
        assert!(args.contains(&"--variant".to_string()), "argv: {args:?}");
        assert!(args.contains(&"high".to_string()), "argv: {args:?}");
    }

    #[test]
    fn build_command_removes_both_api_keys() {
        let cmd = build_opencode_command(None, None, Path::new("/repo"), "{}");
        let anthropic_removed = cmd
            .get_envs()
            .any(|(k, v)| k == "ANTHROPIC_API_KEY" && v.is_none());
        let openai_removed = cmd
            .get_envs()
            .any(|(k, v)| k == "OPENAI_API_KEY" && v.is_none());
        assert!(
            anthropic_removed,
            "ANTHROPIC_API_KEY should be removed on the child"
        );
        assert!(
            openai_removed,
            "OPENAI_API_KEY should be removed on the child"
        );
    }

    #[test]
    fn build_command_injects_skills_config() {
        let cfg = r#"{"skills":{"paths":["/some/skills"]}}"#;
        let cmd = build_opencode_command(None, None, Path::new("/repo"), cfg);
        let injected = cmd
            .get_envs()
            .find(|(k, _)| *k == "OPENCODE_CONFIG_CONTENT")
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().into_owned());
        assert_eq!(injected.as_deref(), Some(cfg));
    }

    // ── parse_opencode_events ────────────────────────────────────────────────

    #[test]
    fn parse_extracts_text_parts() {
        let stream = "{\"type\":\"step_start\",\"snapshot\":\"abc\"}\n\
                      {\"type\":\"text\",\"text\":\"working on it\"}\n\
                      {\"type\":\"text\",\"text\":\"RALPHY_DONE_EXIT\"}\n\
                      {\"type\":\"step_finish\",\"reason\":\"stop\"}\n";
        let (text, saw_error) = parse_opencode_events(stream);
        assert!(text.contains("working on it"), "text: {text:?}");
        assert!(text.contains("RALPHY_DONE_EXIT"), "text: {text:?}");
        assert!(!saw_error);
    }

    #[test]
    fn parse_flags_error_event() {
        let stream = "{\"type\":\"text\",\"text\":\"trying\"}\n\
                      {\"type\":\"error\",\"name\":\"APIError\",\"statusCode\":500}\n";
        let (text, saw_error) = parse_opencode_events(stream);
        assert!(text.contains("trying"));
        assert!(saw_error, "an error event must set the flag");
    }

    #[test]
    fn parse_extracts_text_from_nested_part_envelope() {
        // The real opencode 1.16.2 `--format json` shape, captured live: every
        // event is wrapped `{type, timestamp, sessionID, part:{…}}` and the text
        // lives under `part.text`. The sentinel scan must see it through the
        // envelope, or every execute run misclassifies as Stuck.
        let stream = concat!(
            r#"{"type":"step_start","sessionID":"s","part":{"type":"step-start","snapshot":"abc"}}"#,
            "\n",
            r#"{"type":"tool_use","sessionID":"s","part":{"type":"tool","tool":"read","callID":"c1"}}"#,
            "\n",
            r#"{"type":"text","sessionID":"s","part":{"type":"text","text":"did the work\nRALPHY_DONE_EXIT"}}"#,
            "\n",
            r#"{"type":"step_finish","sessionID":"s","part":{"type":"step-finish","reason":"stop"}}"#,
            "\n",
        );
        let (text, saw_error) = parse_opencode_events(stream);
        assert!(
            text.contains("RALPHY_DONE_EXIT"),
            "must extract the sentinel from part.text: {text:?}"
        );
        assert!(
            ralphy_adapter_support::done_sentinel(&text),
            "done_sentinel must fire on the extracted text"
        );
        // A `tool` part must not be mistaken for text or an error.
        assert!(!saw_error, "a tool_use envelope is not an error");
    }

    #[test]
    fn parse_flags_error_event_in_nested_part() {
        // An error carried inside the `part` envelope must still set saw_error.
        let stream = r#"{"type":"error","sessionID":"s","part":{"type":"error","name":"APIError","statusCode":500}}"#;
        let (_text, saw_error) = parse_opencode_events(stream);
        assert!(saw_error, "a nested error part must flag saw_error");
    }

    #[test]
    fn parse_limit_detects_429_in_nested_part() {
        // The limit scan must read name/statusCode/retryAfter from `part` too.
        let stream = concat!(
            r#"{"type":"error","sessionID":"s","part":{"type":"error","name":"APIError","statusCode":429,"message":"rate limited","retryAfter":"2026-06-10T18:00:00Z"}}"#,
            "\n",
        );
        assert_eq!(
            parse_opencode_limit(stream),
            Some(Some("2026-06-10T18:00:00Z".into())),
            "must detect a 429 nested under part and extract the reset"
        );
    }

    #[test]
    fn parse_tolerates_unparseable_lines() {
        let stream = "not json at all\n\
                      {\"type\":\"text\",\"text\":\"kept\"}\n\
                      \n";
        let (text, saw_error) = parse_opencode_events(stream);
        assert_eq!(text.trim(), "kept");
        assert!(!saw_error);
    }

    // ── prompt asset ─────────────────────────────────────────────────────────

    #[test]
    fn prompt_plan_opencode_has_no_execution_model_line() {
        assert!(
            !PROMPT_PLAN_OPENCODE.contains("## Execution model"),
            "the OpenCode plan prompt must drop the complexity tier line (D3/D8a)"
        );
    }

    #[test]
    fn prompt_plan_opencode_keeps_reviewer_step() {
        assert!(
            PROMPT_PLAN_OPENCODE.contains("reviewer"),
            "planning prompt must reference the reviewer skill"
        );
        let lower = PROMPT_PLAN_OPENCODE.to_lowercase();
        assert!(
            lower.contains("only") && lower.contains("commits you made"),
            "reviewer step must scope to this issue's own commits"
        );
        // No Claude Task-tool idiom carried over.
        assert!(
            !PROMPT_PLAN_OPENCODE.contains("independent subagent"),
            "must not use Claude 'independent subagent' phrasing"
        );
        // The reviewer step commits to the concrete working mechanism: the
        // inline `reviewer` skill, not a subagent (opencode#29616/#20059 block
        // headless custom-subagent dispatch — see ADR-0005 D8).
        assert!(
            lower.contains("inline") && lower.contains("skill"),
            "reviewer step must name the inline reviewer skill mechanism"
        );
        // No subagent-dispatch phrasing for the reviewer: the prompt must not
        // claim the reviewer runs "as a subagent".
        assert!(
            !lower.contains("as a subagent"),
            "reviewer step must not describe the reviewer as running as a subagent"
        );
    }

    // ── trait binding (compile-level) ─────────────────────────────────────────

    #[test]
    fn opencode_agent_is_a_dyn_agent() {
        // Proves `OpenCodeAgent: Agent` and that it can be handed to the core as a
        // `&dyn Agent` (the core never learns the vendor).
        let agent = OpenCodeAgent::new(None, PathBuf::from("/run")).with_variant(None);
        let _as_dyn: &dyn Agent = &agent;
    }

    // ── materialize_opencode_skills ────────────────────────────────────────

    #[test]
    fn materialize_opencode_skills_extracts_required_skills() {
        let base =
            std::env::temp_dir().join(format!("ralphy-opencode-skills-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let ws = Workspace::new(&base);

        let skills_dir = materialize_opencode_skills(&ws).expect("materialize");
        assert_eq!(skills_dir, ws.ralphy_dir().join("skills"));
        assert!(
            skills_dir.join("reviewer/SKILL.md").is_file(),
            "reviewer/SKILL.md must be materialized"
        );
        assert!(
            skills_dir.join("staged-plan/SKILL.md").is_file(),
            "staged-plan/SKILL.md must be materialized"
        );
        assert!(
            skills_dir.join("reviewer/scripts/audit.py").is_file(),
            "reviewer/scripts/audit.py must be materialized"
        );
        assert!(
            ws.ralphy_dir().join(".gitignore").is_file(),
            ".ralphy/.gitignore must be written"
        );

        // Idempotent: a second call clears and re-extracts cleanly.
        materialize_opencode_skills(&ws).expect("re-materialize");
        assert!(skills_dir.join("reviewer/SKILL.md").is_file());

        let _ = fs::remove_dir_all(&base);
    }

    // ── opencode_skills_config ─────────────────────────────────────────────

    #[test]
    fn opencode_skills_config_is_well_formed_json() {
        let dir = std::env::temp_dir().join("ralphy-skills-cfg-test");
        fs::create_dir_all(&dir).unwrap();
        let json_str = opencode_skills_config(&dir);
        let val: serde_json::Value = serde_json::from_str(&json_str).expect("must be valid JSON");
        let paths = val["skills"]["paths"]
            .as_array()
            .expect("skills.paths must be an array");
        assert_eq!(paths.len(), 1, "exactly one path entry");
        let entry = paths[0].as_str().expect("path entry must be a string");
        let expected = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        assert_eq!(
            PathBuf::from(entry),
            expected,
            "path entry must equal the canonicalized skills dir"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
