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
//! `skills.paths` config key points at it. Usage-limit (D9) and auth-error (D6)
//! are deferred-until-live in ADR-0005 and are intentionally not handled here.

use std::fs;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use include_dir::{include_dir, Dir};
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
    if skills_dir.exists() {
        fs::remove_dir_all(&skills_dir).context("clearing stale .ralphy/skills")?;
    }
    fs::create_dir_all(&skills_dir).context("creating .ralphy/skills")?;
    SKILLS
        .extract(&skills_dir)
        .context("extracting the embedded skills to .ralphy/skills")?;
    fs::write(ralphy_dir.join(".gitignore"), "*\n").context("writing .ralphy/.gitignore")?;
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
/// reviewer step rephrased to vendor-neutral dispatch. Single source of truth
/// lives at `assets/prompts/`.
const PROMPT_PLAN_OPENCODE: &str = include_str!("../../../assets/prompts/prompt.plan.opencode.md");

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
    let mut cmd = Command::new("opencode");
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

/// Parse OpenCode's `--format json` line-delimited event stream: concatenate the
/// assistant `text` parts into the returned string (the source the sentinel scan
/// reads) and set the bool when any line is an `error` event. Tolerant of
/// unparseable lines (skipped) — the exact event JSON is "deferred until live" in
/// ADR-0005, so this keys only on the documented `type:"text"` / `type:"error"`
/// shapes and ignores everything else.
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
        let kind = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if kind == "error" {
            saw_error = true;
        }
        if kind == "text" {
            if let Some(t) = val.get("text").and_then(|v| v.as_str()) {
                text.push_str(t);
                text.push('\n');
            }
        }
    }
    (text, saw_error)
}

/// Map an execution call's end state onto a core [`Outcome`] (ADR-0005 D2): the
/// wall timeout wins (`Timeout`); a `RALPHY_BLOCKED_EXIT <reason>` sentinel in the
/// `text` parts is `Blocked(reason)`; a clean exit that committed, saw no `error`
/// event, and emitted `RALPHY_DONE_EXIT` is `Done`; anything else — a non-zero
/// exit, a JSON `error` event, no new commit, or no sentinel — is `Stuck`. The
/// HEAD-diff `committed` check is the progress guard the Claude headless loop and
/// the Codex adapter already use: OpenCode makes internal snapshots, not git
/// commits, so a `Done` claim with no commit is distrusted and downgraded.
fn classify_opencode_outcome(
    exited_cleanly: bool,
    timed_out: bool,
    committed: bool,
    text: &str,
    saw_error: bool,
) -> Outcome {
    if timed_out {
        return Outcome::Timeout;
    }
    if let Some(line) = text.lines().find(|l| l.contains("RALPHY_BLOCKED_EXIT")) {
        let reason = line
            .split_once("RALPHY_BLOCKED_EXIT")
            .map(|(_, rest)| rest.trim().to_string())
            .unwrap_or_default();
        return Outcome::Blocked(reason);
    }
    if exited_cleanly && committed && !saw_error && text.contains("RALPHY_DONE_EXIT") {
        return Outcome::Done;
    }
    Outcome::Stuck
}

impl OpenCodeAgent {
    /// Spawn a single `opencode run` call, piping `prompt` on stdin and draining
    /// stdout/stderr via reader threads to avoid pipe-buffer deadlock (mirrors
    /// `CodexAgent::run_codex`). Polls `try_wait` until `timeout`; kills the child
    /// on expiry. Returns `(exited_cleanly, timed_out, stdout_text)` — stdout is
    /// the JSON event stream the caller parses; the combined stdout+stderr is also
    /// written to `run_dir/opencode.log` for inspection.
    fn run_opencode(
        &self,
        mut cmd: Command,
        prompt: &str,
        timeout: Duration,
    ) -> Result<(bool, bool, String)> {
        let mut child = cmd
            .spawn()
            .context("failed to spawn the `opencode` CLI (is it installed and on PATH?)")?;

        // Spawn the reader threads *before* writing stdin, so a prompt larger than
        // the pipe buffer (~64KB) can't deadlock against a child that starts
        // emitting output before it finishes draining stdin.
        let mut stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");

        let (tx_out, rx_out) = mpsc::channel::<Vec<u8>>();
        let (tx_err, rx_err) = mpsc::channel::<Vec<u8>>();
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = BufReader::new(stdout).read_to_end(&mut buf);
            let _ = tx_out.send(buf);
        });
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = BufReader::new(stderr).read_to_end(&mut buf);
            let _ = tx_err.send(buf);
        });

        stdin
            .write_all(prompt.as_bytes())
            .context("piping the prompt to opencode")?;
        drop(stdin); // close stdin so opencode sees EOF

        let deadline = Instant::now() + timeout;
        let mut timed_out = false;
        let status = loop {
            if let Some(s) = child.try_wait().context("polling opencode child")? {
                break Some(s);
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                timed_out = true;
                break None;
            }
            thread::sleep(Duration::from_millis(500));
        };

        let collect = Duration::from_secs(5);
        let stdout_bytes = rx_out.recv_timeout(collect).unwrap_or_default();
        let stderr_bytes = rx_err.recv_timeout(collect).unwrap_or_default();
        let stdout_text = String::from_utf8_lossy(&stdout_bytes).into_owned();
        // The combined log keeps stderr too — the JSON stream lives on stdout, but
        // a crash or auth failure often only prints to stderr.
        let mut log = stdout_text.clone();
        log.push_str(&String::from_utf8_lossy(&stderr_bytes));
        let _ = fs::write(self.run_dir.join("opencode.log"), &log);

        let exited_cleanly = status.map(|s| s.success()).unwrap_or(false);
        Ok((exited_cleanly, timed_out, stdout_text))
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
        let _ = self.run_opencode(cmd, PROMPT_PLAN_OPENCODE, timeout)?;

        if !plan_path.exists() {
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
        let (exited_cleanly, timed_out, stdout_text) =
            self.run_opencode(cmd, PROMPT_EXECUTE, timeout)?;

        let after_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        let committed = before_sha != after_sha;
        let (text, saw_error) = parse_opencode_events(&stdout_text);

        let outcome =
            classify_opencode_outcome(exited_cleanly, timed_out, committed, &text, saw_error);
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

    // ── classify_opencode_outcome ───────────────────────────────────────────

    #[test]
    fn classify_done_on_clean_exit_commit_and_sentinel() {
        let text = "all steps green\nRALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_opencode_outcome(true, false, true, text, false),
            Outcome::Done
        );
    }

    #[test]
    fn classify_stuck_on_no_commit() {
        // A DONE claim with no new commit is distrusted (HEAD-diff progress guard).
        let text = "RALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_opencode_outcome(true, false, false, text, false),
            Outcome::Stuck
        );
    }

    #[test]
    fn classify_blocked_on_blocked_sentinel() {
        let text = "did some work\nRALPHY_BLOCKED_EXIT missing upstream crate\n";
        assert_eq!(
            classify_opencode_outcome(true, false, true, text, false),
            Outcome::Blocked("missing upstream crate".into())
        );
    }

    #[test]
    fn classify_stuck_on_non_zero_exit() {
        // A non-zero exit is Stuck even when the output carries a DONE sentinel.
        let text = "RALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_opencode_outcome(false, false, true, text, false),
            Outcome::Stuck
        );
    }

    #[test]
    fn classify_stuck_on_error_event() {
        // A JSON `error` event downgrades an otherwise-clean DONE claim to Stuck.
        let text = "RALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_opencode_outcome(true, false, true, text, true),
            Outcome::Stuck
        );
    }

    #[test]
    fn classify_stuck_on_no_sentinel() {
        assert_eq!(
            classify_opencode_outcome(true, false, true, "quiet exit, no sentinel", false),
            Outcome::Stuck
        );
    }

    #[test]
    fn classify_timeout_wins() {
        // The wall timeout wins over everything, including a DONE sentinel.
        let text = "RALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_opencode_outcome(false, true, false, text, false),
            Outcome::Timeout
        );
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
        assert_eq!(cmd.get_program().to_string_lossy(), "opencode");
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
