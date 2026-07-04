//! The Claude Code adapter: drives `claude` behind the core [`Agent`] contract.
//! Everything Claude-specific — the binary, the model and effort flags, the
//! settings file, the PTY, completion detection — is confined here.
//!
//! `plan` runs headless `claude -p` (prompt piped on stdin). `execute` runs a
//! *live* interactive session over [`ralphy_pty`]: it lets `claude` commit onto
//! the run branch, detects completion from a flag file its Stop hook writes, and
//! reclaims the session on a per-issue wall timeout.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{bail, Context, Result};
use include_dir::{include_dir, Dir};
use ralphy_adapter_support::{
    list_session_files, run_headless_logged, run_json_session, run_text_session,
    session_files_appeared, JsonSession, TextSession, PROMPT_EXECUTE,
};
use ralphy_core::{
    build_diagnose_prompt, build_init_issues_prompt, build_triage_prompt, git, plan, Agent,
    DiagnosisReport, DraftRequest, Execution, Issue, IssuesDraft, Outcome, Plan, PlanLimit,
    TriageDraft, TriageRequest, Usage, Workspace,
};
use ralphy_pty::{PtyCommand, PtySession, CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tracing::info;

/// The planning prompt, embedded so the binary is self-contained as a global
/// tool. Copied to `.ralphy/plan-charter.md` for the live session to read;
/// only a one-line pointer is piped on stdin. Single source of truth lives at
/// `assets/prompts/`.
const PROMPT_PLAN: &str = include_str!("../../../assets/prompts/prompt.plan.md");

/// The staged-plan planning prompt, used when the issue carries the
/// `stagedplan` label.
const PROMPT_PLAN_STAGED: &str = include_str!("../../../assets/prompts/prompt.plan.staged.md");

/// The knowledge-consolidation charter (`ralphy consolidate`): curate the loose
/// `.ralphy/knowledge/issue-<N>.md` notes into one `KNOWLEDGE.md`.
const PROMPT_CONSOLIDATE: &str = include_str!("../../../assets/prompts/prompt.consolidate.md");

/// The one-line charter the interactive session is launched with; it points the
/// agent at the embedded charter and the plan, and names the exit sentinel.
const EXEC_CHARTER: &str = "Read .ralphy/exec.md and follow it exactly to implement .ralphy/plan.md for this issue. Emit RALPHY_DONE_EXIT when finished.";

/// Minimal settings that keep a headless `claude -p` from hanging on a prompt.
/// The Stop hook is an execution concern, added by [`exec_settings_json`].
const SETTINGS_JSON: &str = r#"{"skipDangerousModePermissionPrompt":true,"skipAutoPermissionPrompt":true,"autoCompactEnabled":false}"#;

/// The operational Claude Code plugin, embedded at build time so the binary is a
/// self-contained global tool. It bundles the `reviewer` and `staged-plan`
/// skills the planning/execution prompts depend on; the single source of truth
/// lives at the repo root under `assets/plugin`.
static PLUGIN: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/plugin");

/// `<repo>/.ralphy/plugin` — where this adapter materializes its embedded
/// Claude Code plugin. Derived from the vendor-neutral `ralphy_dir()`; the
/// core does not know the subdir exists (ADR-0002 amendment, #79).
fn plugin_dir(ws: &Workspace) -> PathBuf {
    ws.ralphy_dir().join("plugin")
}

/// Materialize the embedded plugin into the workspace's `.ralphy/plugin` so it
/// can be handed to `claude` via `--plugin-dir`. Re-extracted from scratch each
/// call (the tree is tiny) so a stale or partly-written copy never lingers, and
/// the run never depends on whatever skills are installed globally. Returns the
/// plugin directory to pass on the command line.
fn materialize_plugin(ws: &Workspace) -> Result<PathBuf> {
    let dir = plugin_dir(ws);
    ralphy_adapter_support::materialize_assets(&PLUGIN, &dir, None)?;
    Ok(dir)
}

/// Select the planning prompt for an issue. Returns `(prompt, staged)` where
/// `staged` is `true` when the issue carries the `stagedplan` label.
fn plan_prompt_for(issue: &Issue) -> (&'static str, bool) {
    if issue.labels.iter().any(|l| l == "stagedplan") {
        (PROMPT_PLAN_STAGED, true)
    } else {
        (PROMPT_PLAN, false)
    }
}

/// Write the selected planning charter to `.ralphy/plan-charter.md` (mirrors
/// `.ralphy/exec.md`); rewritten each plan call so a resumed session and a
/// `stagedplan` label switch both see the right content.
fn write_plan_charter(ws: &Workspace, prompt: &str) -> Result<()> {
    fs::write(ws.plan_charter_path(), prompt).context("writing .ralphy/plan-charter.md")
}

/// The env var a staged plan sets so the `staged-plan` skill knows it is running
/// non-interactively (no TTY to prompt on). Returns `Some((key, value))` when
/// `staged`, otherwise `None` so the standard plan leaves the environment clean.
fn staged_plan_env(staged: bool) -> Option<(&'static str, &'static str)> {
    if staged {
        Some(("STAGED_PLAN_NONINTERACTIVE", "1"))
    } else {
        None
    }
}

/// Claude-specific run defaults persisted under the [`ClaudeSettings::SECTION`]
/// section of `.ralphy/settings.json` (ADR-0010). The core stores the section as
/// opaque JSON; this adapter owns the schema (ADR-0002 amendment, #79). Each
/// field is `None` out of the box, leaving the hardcoded run defaults in place;
/// resolution precedence stays per-run flag > settings.json > hardcoded default.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClaudeSettings {
    /// Planning model default (`--plan-model`). `None` → hardcoded `opus`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_model: Option<String>,
    /// Planning effort default (`--plan-effort`). `None` → hardcoded `medium`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_effort: Option<String>,
    /// Execution model used when the plan emits no complexity judgment
    /// (`--default-exec-model`). `None` → hardcoded `sonnet`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_exec_model: Option<String>,
    /// Execution effort default (`--exec-effort`). `None` → hardcoded `medium`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_effort: Option<String>,
    /// Per-issue wall-clock budget in minutes (`--max-minutes-per-issue`).
    /// `None` → [`ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE`] (unbounded by
    /// default). `0` — whether from the default or set explicitly — disables the
    /// per-issue cap: the issue is then bounded only by `--deadline-hours`. A
    /// positive value opts into a cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_minutes_per_issue: Option<u64>,
}

impl ClaudeSettings {
    /// The settings-file section this struct lives under.
    pub const SECTION: &'static str = "claude";
}

/// The planner's `## Execution model: sonnet|opus` judgment, lowercased, if any.
/// Claude-vocabulary parsing lives here, not in core (ADR-0002 amendment, #79):
/// core's `Plan.recommended_model` is an opaque token it only carries across.
fn recommended_model(md: &str) -> Option<String> {
    let re =
        regex::Regex::new(r"(?im)^\s*##\s*Execution model:\s*(opus|sonnet)").expect("valid regex");
    re.captures(md).map(|c| c[1].to_lowercase())
}

/// Drives the `claude` CLI. `plan_model`/`plan_effort` are the planning knobs;
/// the `exec_*` fields configure the interactive execution session. `run_dir` is
/// where the settings file, the captured logs, and the per-issue flag file live.
pub struct ClaudeAgent {
    plan_model: Option<String>,
    plan_effort: Option<String>,
    run_dir: PathBuf,
    exec: ExecConfig,
}

/// The execution-side configuration, separate from the planning knobs.
struct ExecConfig {
    /// Forces the execution model for the issue when set (overrides the plan's
    /// judgment).
    exec_model: Option<String>,
    /// Reasoning effort for the execution session.
    exec_effort: Option<String>,
    /// Model used when neither an override nor a plan judgment is present.
    default_exec_model: String,
    /// Per-issue wall-clock budget before the session is reclaimed.
    max_minutes_per_issue: u64,
    /// Whether to enable Remote Control (follow/intervene from the mobile app).
    remote_control: bool,
    /// When true, use a `claude -p` loop instead of an interactive PTY session.
    headless_exec: bool,
    /// Maximum number of `-p` calls before declaring MaxCalls (headless only).
    max_exec_calls: u32,
    /// The run's global wall-clock deadline, if any. Each issue's budget is
    /// clamped to `min(per-issue, run_deadline)` so an issue started near the
    /// global limit can't overrun it (mirrors `min(issueDeadline, $Deadline)`
    /// in ralphy.ps1:270).
    run_deadline: Option<Instant>,
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            exec_model: None,
            exec_effort: Some("medium".into()),
            default_exec_model: "sonnet".into(),
            max_minutes_per_issue: ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE,
            remote_control: true,
            headless_exec: false,
            max_exec_calls: 6,
            run_deadline: None,
        }
    }
}

impl ClaudeAgent {
    pub fn new(plan_model: Option<String>, plan_effort: Option<String>, run_dir: PathBuf) -> Self {
        Self {
            plan_model,
            plan_effort,
            run_dir,
            exec: ExecConfig::default(),
        }
    }

    /// Set the execution-side configuration (the composition root supplies it
    /// from the CLI flags).
    #[allow(clippy::too_many_arguments)]
    pub fn with_exec_config(
        mut self,
        exec_model: Option<String>,
        exec_effort: Option<String>,
        default_exec_model: String,
        max_minutes_per_issue: u64,
        remote_control: bool,
        headless_exec: bool,
        max_exec_calls: u32,
    ) -> Self {
        self.exec = ExecConfig {
            exec_model,
            exec_effort,
            default_exec_model,
            max_minutes_per_issue,
            remote_control,
            headless_exec,
            max_exec_calls,
            run_deadline: None,
        };
        self
    }

    /// Set the run's global wall-clock deadline (from `--deadline-hours`). Each
    /// issue's execution budget is then clamped to it, so an issue started just
    /// under the global limit can't overrun by a whole per-issue window.
    pub fn with_run_deadline(mut self, run_deadline: Option<Instant>) -> Self {
        self.exec.run_deadline = run_deadline;
        self
    }

    /// The deadline for the current issue: the per-issue budget, clamped to the
    /// run's global deadline when one is set. A budget of `0` disables the
    /// per-issue cap — the issue is then bounded only by the run deadline (or the
    /// far-future [`UNBOUNDED_ISSUE_HORIZON`] when no run deadline is set).
    fn issue_deadline(&self) -> Instant {
        ralphy_adapter_support::issue_deadline(
            Instant::now(),
            self.exec.max_minutes_per_issue,
            self.exec.run_deadline,
            ralphy_core::UNBOUNDED_ISSUE_HORIZON,
        )
    }

    /// The single tier→model decision point: explicit override > the plan's
    /// `## Execution model` judgment > the configured default. Returns the
    /// literal model string `claude --model` expects (`sonnet`/`opus`).
    fn resolve_exec_model(&self, plan: &Plan) -> String {
        if let Some(m) = &self.exec.exec_model {
            return m.clone();
        }
        if let Some(m) = &plan.recommended_model {
            return m.clone();
        }
        self.exec.default_exec_model.clone()
    }

    /// Write `ralphy.settings.json` with the skip flags and a Stop hook that
    /// invokes *this* binary's `hook stop`. Returns the settings path.
    fn write_exec_settings(&self) -> Result<PathBuf> {
        let exe =
            std::env::current_exe().context("locating the ralphy binary for the Stop hook")?;
        let json = exec_settings_json(
            &stop_hook_command(&exe),
            &guard_hook_command(&exe),
            &post_hook_command(&exe),
        );
        let path = self.run_dir.join("ralphy.settings.json");
        fs::write(&path, json).context("writing exec settings")?;
        Ok(path)
    }
}

/// Quote the Stop-hook command line for the platform: `"<exe>" hook stop`.
fn stop_hook_command(exe: &Path) -> String {
    format!("\"{}\" hook stop", exe.display())
}

/// Quote the guard-hook command line for the platform: `"<exe>" hook guard`.
fn guard_hook_command(exe: &Path) -> String {
    format!("\"{}\" hook guard", exe.display())
}

/// Quote the post-hook command line for the platform: `"<exe>" hook post`.
fn post_hook_command(exe: &Path) -> String {
    format!("\"{}\" hook post", exe.display())
}

/// Build the execution settings JSON: the headless skip flags, a `Stop` hook
/// running `stop_command`, a `PreToolUse` guard running `guard_command`, and a
/// `PostToolUse` Bash timer running `post_command` (the other half of the
/// verification-cost gate: the guard stamps a verify command's start, this hook
/// records its measured duration).
fn exec_settings_json(stop_command: &str, guard_command: &str, post_command: &str) -> String {
    let settings = serde_json::json!({
        "skipDangerousModePermissionPrompt": true,
        "skipAutoPermissionPrompt": true,
        "autoCompactEnabled": false,
        "hooks": {
            "Stop": [
                {
                    "matcher": "",
                    "hooks": [ { "type": "command", "command": stop_command } ]
                }
            ],
            "PreToolUse": [
                {
                    "matcher": "Bash|Edit|Write|MultiEdit|NotebookEdit",
                    "hooks": [ { "type": "command", "command": guard_command } ]
                }
            ],
            "PostToolUse": [
                {
                    "matcher": "Bash",
                    "hooks": [ { "type": "command", "command": post_command } ]
                }
            ]
        }
    });
    serde_json::to_string_pretty(&settings).expect("settings serialize")
}

impl ClaudeAgent {
    /// Spawn a single `claude -p` call for headless execution, piping
    /// `PROMPT_EXECUTE` on stdin and draining stdout/stderr via reader threads
    /// to avoid pipe-buffer deadlock. Polls `try_wait` until `timeout` fires;
    /// kills the child on expiry and returns `exited = false`.
    fn run_headless_call(
        &self,
        cmd_dir: &Path,
        settings: &Path,
        plugin_dir: &Path,
        model: &str,
        timeout: Duration,
        call_index: u32,
    ) -> Result<(bool, String)> {
        let mut args: Vec<String> = vec![
            "-p".into(),
            "--dangerously-skip-permissions".into(),
            "--settings".into(),
            settings.to_string_lossy().into_owned(),
            "--plugin-dir".into(),
            plugin_dir.to_string_lossy().into_owned(),
            "--model".into(),
            model.into(),
        ];
        if let Some(e) = &self.exec.exec_effort {
            args.push("--effort".into());
            args.push(e.clone());
        }

        let mut cmd = Command::new(resolve_claude_binary());
        cmd.args(&args)
            .current_dir(cmd_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Delegate the OS-level spawn/drain/poll/kill/collect and log-persist
        // plumbing to the shared runner. Claude's `exited` ("the child exited
        // rather than being killed on the wall timeout") is `!timed_out` — NOT the
        // runner's `exited_cleanly` (a *successful* exit); the D10 auth bail stays
        // inline here.
        let log_path = self.run_dir.join(format!("exec-{}.out", call_index));
        let r = run_headless_logged(cmd, PROMPT_EXECUTE, timeout, &log_path)
            .context("failed to spawn the `claude` CLI for headless exec")?;

        if is_claude_auth_error(&r.log) {
            bail!("{} (see {})", CLAUDE_AUTH_ERROR_MSG, log_path.display());
        }

        Ok((!r.timed_out, r.log))
    }

    /// Drive the issue with a `claude -p` loop (headless mode). Mirrors the
    /// `Invoke-ExecLoop` ps1 oracle: writes `exec.md`, loops up to
    /// `max_exec_calls` calls, and classifies the per-call output into a core
    /// `Outcome` via `headless_reason_to_outcome`.
    fn execute_headless(&self, plan: &Plan, ws: &Workspace) -> Result<Outcome> {
        fs::create_dir_all(&self.run_dir).ok();
        fs::create_dir_all(ws.ralphy_dir()).ok();

        fs::write(ws.ralphy_dir().join("exec.md"), PROMPT_EXECUTE)
            .context("writing .ralphy/exec.md")?;

        let settings_path = self.write_exec_settings()?;
        let plugin_dir = materialize_plugin(ws)?;
        let exec_model = self.resolve_exec_model(plan);
        let deadline = self.issue_deadline();

        // budget_min field consumed by the telegram notifier / presenter — keep stable
        info!(
            model = %exec_model,
            effort = self.exec.exec_effort.as_deref().unwrap_or("medium"),
            max_calls = self.exec.max_exec_calls,
            budget_min = self.exec.max_minutes_per_issue,
            "executing with headless claude -p loop"
        );

        let mut no_commit_streak = 0u32;

        for i in 1..=self.exec.max_exec_calls {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining <= Duration::from_secs(5) {
                info!(
                    call = i,
                    "per-issue deadline reached before next headless call"
                );
                return Ok(headless_reason_to_outcome(HeadlessReason::Timeout));
            }

            let before_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
            let (exited, out) = self.run_headless_call(
                ws.repo_root(),
                &settings_path,
                &plugin_dir,
                &exec_model,
                remaining,
                i,
            )?;

            let plan_md = fs::read_to_string(&plan.path).unwrap_or_default();
            let open_steps = plan::count_open_steps(&plan_md);
            let after_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
            let committed = before_sha != after_sha;

            let classified = classify_exec_call(&out, exited, open_steps);
            match headless_step(no_commit_streak, classified, committed) {
                LoopStep::Terminal(reason) => {
                    info!(call = i, "headless call terminal");
                    return Ok(headless_reason_to_outcome(reason));
                }
                LoopStep::Continue(streak) => {
                    no_commit_streak = streak;
                    if !committed {
                        info!(
                            call = i,
                            streak = no_commit_streak,
                            "no commit this headless call"
                        );
                    }
                }
            }
        }

        info!(
            max_calls = self.exec.max_exec_calls,
            "headless loop exhausted max calls"
        );
        Ok(headless_reason_to_outcome(HeadlessReason::MaxCalls))
    }
}

/// Run a one-shot headless `claude -p` knowledge-consolidation session in
/// `ws`: pipe the consolidation charter on stdin and wait up to `timeout`.
/// Mirrors the planning pass's invocation (settings with the skip flags, no
/// Stop hook) — the session's only deliverable is `KNOWLEDGE.md`, which the
/// caller verifies; the consumed notes are archived by the caller, not here.
pub fn consolidate_knowledge(
    ws: &Workspace,
    run_dir: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<()> {
    fs::create_dir_all(run_dir).ok();
    let settings_path = run_dir.join("ralphy.settings.json");
    fs::write(&settings_path, SETTINGS_JSON).context("writing claude settings")?;

    let mut args: Vec<String> = Vec::new();
    if let Some(m) = model {
        args.push("--model".into());
        args.push(m.into());
    }
    args.push("-p".into());
    args.push("--dangerously-skip-permissions".into());
    args.push("--settings".into());
    args.push(settings_path.to_string_lossy().into_owned());
    if let Some(e) = effort {
        args.push("--effort".into());
        args.push(e.into());
    }

    info!(?model, ?effort, "consolidating knowledge with claude -p");
    let mut cmd = Command::new(resolve_claude_binary());
    cmd.args(&args)
        .current_dir(ws.repo_root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // A non-JSON one-shot: spawn, persist the log, bail on auth then timeout — the
    // shared `run_text_session` owns that exact tail (same messages, same order).
    // The consolidated log is the only deliverable, so its returned value is
    // dropped; the caller verifies `KNOWLEDGE.md` separately.
    run_text_session(
        TextSession {
            cmd,
            prompt: PROMPT_CONSOLIDATE,
            timeout,
            log_path: &run_dir.join("consolidate.log"),
            spawn_err: "failed to spawn the `claude` CLI (is it installed and on PATH?)",
            auth_msg: CLAUDE_AUTH_ERROR_MSG,
            timeout_msg: "consolidation session hit the wall timeout",
        },
        is_claude_auth_error,
    )?;
    Ok(())
}

/// Run a one-shot headless `claude -p` repo-diagnosis session (ADR-0012 stage 2)
/// from `neutral_cwd` — a directory OUTSIDE the target repo, so the agent CLI
/// never auto-loads the target's `CLAUDE.md`/`AGENTS.md` as system instructions.
/// The target `repo` is passed as data in the prompt; the session writes its JSON
/// report to `<neutral_cwd>/diagnosis.json`, which this function reads, validates
/// against [`DiagnosisReport`], and returns. Mirrors [`consolidate_knowledge`]'s
/// settings/auth/timeout handling.
pub fn diagnose_repo(
    repo: &Path,
    neutral_cwd: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<DiagnosisReport> {
    fs::create_dir_all(neutral_cwd).ok();
    let settings_path = neutral_cwd.join("ralphy.settings.json");
    fs::write(&settings_path, SETTINGS_JSON).context("writing claude settings")?;

    let out_path = neutral_cwd.join("diagnosis.json");
    // A stale report from a prior run must never masquerade as this session's
    // output, so clear it before the session runs.
    let _ = fs::remove_file(&out_path);

    let mut args: Vec<String> = Vec::new();
    if let Some(m) = model {
        args.push("--model".into());
        args.push(m.into());
    }
    args.push("-p".into());
    args.push("--dangerously-skip-permissions".into());
    args.push("--settings".into());
    args.push(settings_path.to_string_lossy().into_owned());
    if let Some(e) = effort {
        args.push("--effort".into());
        args.push(e.into());
    }

    info!(?model, ?effort, "diagnosing repo with claude -p");
    let mut cmd = Command::new(resolve_claude_binary());
    cmd.args(&args)
        .current_dir(neutral_cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let log_path = neutral_cwd.join("diagnose.log");
    run_json_session(
        JsonSession {
            cmd,
            prompt: &build_diagnose_prompt(repo, &out_path),
            timeout,
            log_path: &log_path,
            out_path: &out_path,
            spawn_err: "failed to spawn the `claude` CLI (is it installed and on PATH?)",
            auth_msg: CLAUDE_AUTH_ERROR_MSG,
            timeout_msg: "diagnosis session hit the wall timeout",
            missing_msg: "diagnosis session left no report",
        },
        is_claude_auth_error,
        |raw| {
            serde_json::from_str(raw).with_context(|| {
                format!(
                    "diagnosis report at {} did not match the schema",
                    out_path.display()
                )
            })
        },
    )
}

/// Run a one-shot headless `claude -p` backlog/milestone → issues session
/// (ADR-0012 stage 8). Unlike [`diagnose_repo`] this runs IN the repo cwd — it
/// needs the repo's domain glossary/ADRs and (on the milestone path) writes a PRD
/// under `docs/prd/`. The session writes its [`IssuesDraft`] JSON to
/// `out_path`, which this function reads, validates against the schema, and
/// returns. It NEVER publishes to GitHub — that is the cli's job after the dev
/// confirms. Mirrors [`diagnose_repo`]'s settings/auth/timeout handling.
pub fn draft_issues(
    repo: &Path,
    out_path: &Path,
    req: &DraftRequest,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<IssuesDraft> {
    let mode = req.mode;
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let settings_path = repo.join(".ralphy").join("ralphy.settings.json");
    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(&settings_path, SETTINGS_JSON).context("writing claude settings")?;

    // A stale draft from a prior run must never masquerade as this session's
    // output, so clear it before the session runs.
    let _ = fs::remove_file(out_path);

    let mut args: Vec<String> = Vec::new();
    if let Some(m) = model {
        args.push("--model".into());
        args.push(m.into());
    }
    args.push("-p".into());
    args.push("--dangerously-skip-permissions".into());
    args.push("--settings".into());
    args.push(settings_path.to_string_lossy().into_owned());
    if let Some(e) = effort {
        args.push("--effort".into());
        args.push(e.into());
    }

    info!(
        ?model,
        ?effort,
        mode = mode.as_str(),
        "drafting issues with claude -p"
    );
    let mut cmd = Command::new(resolve_claude_binary());
    cmd.args(&args)
        .current_dir(repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let prompt = build_init_issues_prompt(repo, mode, req.source_docs, req.triage_label, out_path);
    let log_path = repo.join(".ralphy").join("init-issues.log");
    run_json_session(
        JsonSession {
            cmd,
            prompt: &prompt,
            timeout,
            log_path: &log_path,
            out_path,
            spawn_err: "failed to spawn the `claude` CLI (is it installed and on PATH?)",
            auth_msg: CLAUDE_AUTH_ERROR_MSG,
            timeout_msg: "backlog → issues session hit the wall timeout",
            missing_msg: "issues session left no draft",
        },
        is_claude_auth_error,
        |raw| {
            serde_json::from_str(raw).with_context(|| {
                format!(
                    "issues draft at {} did not match the schema",
                    out_path.display()
                )
            })
        },
    )
}

/// Run a one-shot headless `claude -p` agent-triage session (ADR-0017). Like
/// [`draft_issues`] it runs IN the repo cwd (the triage judgment reads the repo's
/// glossary/ADRs to decide whether a spec is executable) and reads each
/// `triage-agent` issue's body + full comment thread via `gh`. The session writes
/// its [`TriageDraft`] JSON to `out_path`, which this function reads and validates.
/// It NEVER publishes to GitHub — the cli applies the verdicts after the operator
/// confirms. Mirrors [`draft_issues`]'s settings/auth/timeout handling.
pub fn triage_issues(
    repo: &Path,
    out_path: &Path,
    req: &TriageRequest,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<TriageDraft> {
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let settings_path = repo.join(".ralphy").join("ralphy.settings.json");
    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(&settings_path, SETTINGS_JSON).context("writing claude settings")?;

    // A stale draft from a prior run must never masquerade as this session's
    // output, so clear it before the session runs.
    let _ = fs::remove_file(out_path);

    let mut args: Vec<String> = Vec::new();
    if let Some(m) = model {
        args.push("--model".into());
        args.push(m.into());
    }
    args.push("-p".into());
    args.push("--dangerously-skip-permissions".into());
    args.push("--settings".into());
    args.push(settings_path.to_string_lossy().into_owned());
    if let Some(e) = effort {
        args.push("--effort".into());
        args.push(e.into());
    }

    info!(?model, ?effort, "triaging issues with claude -p");
    let mut cmd = Command::new(resolve_claude_binary());
    cmd.args(&args)
        .current_dir(repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let prompt = build_triage_prompt(repo, req.issue_numbers, req.queue_label, out_path);
    let log_path = repo.join(".ralphy").join("triage.log");
    run_json_session(
        JsonSession {
            cmd,
            prompt: &prompt,
            timeout,
            log_path: &log_path,
            out_path,
            spawn_err: "failed to spawn the `claude` CLI (is it installed and on PATH?)",
            auth_msg: CLAUDE_AUTH_ERROR_MSG,
            timeout_msg: "triage session hit the wall timeout",
            missing_msg: "triage session left no draft",
        },
        is_claude_auth_error,
        |raw| {
            serde_json::from_str(raw).with_context(|| {
                format!(
                    "triage draft at {} did not match the schema",
                    out_path.display()
                )
            })
        },
    )
}

impl Agent for ClaudeAgent {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn plan(&self, issue: &Issue, ws: &Workspace) -> Result<Plan> {
        fs::create_dir_all(ws.ralphy_dir()).ok();
        fs::create_dir_all(&self.run_dir).ok();

        let plan_path = ws.plan_path();
        // Plan fresh every run; never reuse a stale artifact.
        let _ = fs::remove_file(&plan_path);

        let settings_path = self.run_dir.join("ralphy.settings.json");
        fs::write(&settings_path, SETTINGS_JSON).context("writing claude settings")?;

        // Provision the reviewer/staged-plan skills the prompt depends on, scoped
        // to this run via --plugin-dir (no reliance on globally-installed skills).
        let plugin_dir = materialize_plugin(ws)?;

        let (prompt, staged) = plan_prompt_for(issue);
        write_plan_charter(ws, prompt)?;

        // `--model` first (as the ps1 oracle does), then the headless flags.
        let mut args: Vec<String> = Vec::new();
        if let Some(m) = &self.plan_model {
            args.push("--model".into());
            args.push(m.clone());
        }
        args.push("-p".into());
        args.push("--dangerously-skip-permissions".into());
        // Capture per-invocation token usage off the result event (ADR-0008 D5).
        // `stream-json` requires `--verbose` on the pinned CLI; the plan markdown
        // is still written to disk by the session, so the stdout format is free to
        // change. `parse_plan_usage` skips the non-JSON warning preamble.
        args.push("--output-format".into());
        args.push("stream-json".into());
        args.push("--verbose".into());
        args.push("--settings".into());
        args.push(settings_path.to_string_lossy().into_owned());
        args.push("--plugin-dir".into());
        args.push(plugin_dir.to_string_lossy().into_owned());
        if let Some(e) = &self.plan_effort {
            args.push("--effort".into());
            args.push(e.clone());
        }

        info!(
            model = self.plan_model.as_deref().unwrap_or(""),
            effort = self.plan_effort.as_deref().unwrap_or("medium"),
            staged,
            "planning with claude -p"
        );
        let mut cmd = Command::new(resolve_claude_binary());
        cmd.args(&args)
            .current_dir(ws.repo_root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some((key, value)) = staged_plan_env(staged) {
            cmd.env(key, value);
        }
        let mut child = cmd
            .spawn()
            .context("failed to spawn the `claude` CLI (is it installed and on PATH?)")?;

        // Pipe only the one-line pointer charter on stdin (the full charter is
        // on disk at .ralphy/plan-charter.md); dropping the handle closes it so
        // claude sees EOF.
        child
            .stdin
            .take()
            .context("claude plan child stdin was not piped")?
            .write_all(ralphy_adapter_support::PLAN_CHARTER.as_bytes())
            .context("piping the plan pointer charter to claude")?;

        let out = child.wait_with_output().context("waiting for claude")?;
        let mut log = String::from_utf8_lossy(&out.stdout).into_owned();
        log.push_str(&String::from_utf8_lossy(&out.stderr));
        let _ = fs::write(self.run_dir.join("plan.log"), &log);

        if is_claude_auth_error(&log) {
            bail!(
                "{} (see {})",
                CLAUDE_AUTH_ERROR_MSG,
                self.run_dir.join("plan.log").display()
            );
        }

        if !plan_path.exists() {
            if is_limit_text(&log) {
                return Err(PlanLimit {
                    reset: parse_reset_hhmm(&log),
                }
                .into());
            }
            bail!(
                "claude produced no plan at {} (see {})",
                plan_path.display(),
                self.run_dir.join("plan.log").display()
            );
        }
        let md = fs::read_to_string(&plan_path).context("reading the written plan.md")?;
        Ok(Plan {
            open_steps: plan::count_open_steps(&md),
            recommended_model: recommended_model(&md),
            path: plan_path,
            usage: parse_plan_usage(&log),
        })
    }

    fn execute(&self, plan: &Plan, ws: &Workspace) -> Result<Execution> {
        // Snapshot the dashed-cwd transcript dir around the whole session so a
        // file that APPEARED is this run's transcript, while one that merely grew
        // is a concurrent pre-existing session and is excluded (ADR-0008 D10,
        // appeared-over-grew). A missing dir yields empty before/after and zero
        // usage — best-effort, never failing the run.
        let transcript_dir = self.transcript_dir(ws);
        let before = transcript_dir
            .as_deref()
            .map(|d| list_session_files(d, "jsonl", false, None))
            .unwrap_or_default();

        let outcome = self.execute_outcome(plan, ws)?;

        let after = transcript_dir
            .as_deref()
            .map(|d| list_session_files(d, "jsonl", false, None))
            .unwrap_or_default();
        let per_transcript: Vec<Usage> = session_files_appeared(&before, &after)
            .iter()
            .filter_map(|p| fs::read_to_string(p).ok())
            .map(|t| parse_transcript_usage(&t))
            .collect();
        let usage = fold_exec_usage(&per_transcript, &self.resolve_exec_model(plan));
        Ok(Execution { outcome, usage })
    }
}

impl ClaudeAgent {
    /// `~/.claude/projects/<dashed-cwd>` for the repo this run operates on — the
    /// directory Claude writes the session transcript JSONL into (ADR-0008 D10).
    /// Derived from the byte-exact cwd passed to `claude` (the repo root).
    fn transcript_dir(&self, ws: &Workspace) -> Option<PathBuf> {
        let cwd = ws.repo_root().to_string_lossy();
        ralphy_adapter_support::home_scoped_path(
            None,
            Path::new(".claude/projects"),
            &PathBuf::from(dashed_cwd(&cwd)),
        )
    }

    /// Drive the execution session (headless `-p` loop or interactive PTY) to a
    /// core [`Outcome`]. The token snapshot/wrap lives in [`Agent::execute`]; this
    /// keeps the completion-classification logic exactly as it was.
    fn execute_outcome(&self, plan: &Plan, ws: &Workspace) -> Result<Outcome> {
        if self.exec.headless_exec {
            return self.execute_headless(plan, ws);
        }

        fs::create_dir_all(&self.run_dir).ok();
        fs::create_dir_all(ws.ralphy_dir()).ok();

        // The live session reads the charter from disk (the headless copy keeps
        // the binary self-contained).
        fs::write(ws.ralphy_dir().join("exec.md"), PROMPT_EXECUTE)
            .context("writing .ralphy/exec.md")?;

        // Pre-clear Claude's first-run interactive gates (workspace trust AND the
        // theme/onboarding wizard) so the live session doesn't stall on a keypress.
        ensure_interactive_session_ready(ws.repo_root());

        let settings_path = self.write_exec_settings()?;
        let plugin_dir = materialize_plugin(ws)?;
        let exec_model = self.resolve_exec_model(plan);
        let flag_file = self.run_dir.join("status.flag");
        let _ = fs::remove_file(&flag_file);

        // The Stop hook writes the flag; it learns the path from this env var,
        // inherited by claude through the PTY child.
        let rc_name = self
            .run_dir
            .file_name()
            .map(|s| format!("ralphy-{}", s.to_string_lossy()))
            .unwrap_or_else(|| "ralphy".into());

        // Build the claude argv: settings, skip-permissions, model, effort,
        // optional remote-control, then the charter as the positional prompt.
        let mut cmd = PtyCommand::new(resolve_claude_binary())
            .cwd(ws.repo_root())
            .env("RALPHY_FLAG_FILE", &flag_file)
            .arg("--dangerously-skip-permissions")
            .arg("--settings")
            .arg(settings_path.as_os_str())
            .arg("--plugin-dir")
            .arg(plugin_dir.as_os_str());
        cmd = cmd.arg("--model").arg(&exec_model);
        if let Some(e) = &self.exec.exec_effort {
            cmd = cmd.arg("--effort").arg(e);
        }
        if self.exec.remote_control {
            cmd = cmd.arg("--remote-control").arg(&rc_name);
        }
        cmd = cmd.arg(EXEC_CHARTER);

        // budget_min field consumed by the telegram notifier / presenter — keep stable
        info!(model = %exec_model, effort = self.exec.exec_effort.as_deref().unwrap_or("medium"), remote_control = self.exec.remote_control, budget_min = self.exec.max_minutes_per_issue, "executing with interactive claude over the PTY");

        let transcript_dir = self.transcript_dir(ws);
        let transcript_since = SystemTime::now()
            .checked_sub(Duration::from_secs(2))
            .unwrap_or(SystemTime::UNIX_EPOCH);

        let mut session =
            PtySession::spawn(cmd).context("spawning the claude execution session")?;
        let result = self.drive_session(
            &mut session,
            &flag_file,
            transcript_dir.as_deref(),
            transcript_since,
        );
        // Reclaim: kill the tree and drop the session (closes the ConPTY).
        // Kill unconditionally so the child never outlives us on error paths.
        let _ = session.kill();
        result
    }
}

impl ClaudeAgent {
    /// Drain the PTY (tee to `exec.log`, answer DSR queries) while polling for the
    /// flag file, the child's own exit, and the per-issue wall timeout. Classifies
    /// the result into an [`Outcome`].
    fn drive_session(
        &self,
        session: &mut PtySession,
        flag_file: &Path,
        transcript_dir: Option<&Path>,
        transcript_since: SystemTime,
    ) -> Result<Outcome> {
        let mut reader = session.reader()?;
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });

        let mut log = fs::File::create(self.run_dir.join("exec.log")).ok();
        let deadline = self.issue_deadline();

        let mut timed_out = false;
        let mut child_exited = false;
        let mut limit_transcript: Option<String> = None;
        let mut next_transcript_poll = Instant::now();
        let mut dsr_carry: Vec<u8> = Vec::new();
        let mut login_watch = LoginTuiWatch::new();
        loop {
            // Act as the terminal: tee output and answer cursor-position queries.
            while let Ok(chunk) = rx.try_recv() {
                if scan_dsr_request(&mut dsr_carry, &chunk) {
                    let _ = session.write_all(CURSOR_POSITION_REPLY);
                }
                login_watch.feed(&chunk);
                if let Some(f) = log.as_mut() {
                    let _ = f.write_all(&chunk);
                }
            }

            if flag_file.exists() {
                break;
            }
            if Instant::now() >= next_transcript_poll {
                if let Some(t) = latest_transcript_text_since(transcript_dir, transcript_since) {
                    // Any transcript activity proves the model loop started —
                    // a logged-out session never produces one (see LoginTuiWatch).
                    login_watch.disarm();
                    if transcript_limit(&t).is_some() {
                        limit_transcript = Some(t);
                        break;
                    }
                } else if login_watch.detected() {
                    // Logged-out interactive session: the login TUI stalls
                    // without exiting, so fail fast with the auth message
                    // instead of burning the wall budget into a misleading
                    // `Timeout` (issue #72). The caller kills the session.
                    bail!("{CLAUDE_AUTH_ERROR_MSG}");
                }
                next_transcript_poll = Instant::now() + Duration::from_secs(2);
            }
            if session.try_wait()?.is_some() {
                child_exited = true;
                break;
            }
            if Instant::now() >= deadline {
                timed_out = true;
                break;
            }
            thread::sleep(Duration::from_millis(500));
        }

        let flag = fs::read_to_string(flag_file).ok();
        // A transcript read is needed to spot a usage limit when the session
        // ends without a sentinel, and the live loop above also watches for the
        // Claude CLI's subagent/tool-result rate-limit shape while the PTY stays
        // alive.
        let transcript = if flag.is_none() {
            limit_transcript.or_else(|| {
                (child_exited || timed_out)
                    .then(|| latest_transcript_text_since(transcript_dir, transcript_since))
                    .flatten()
                    .or_else(|| {
                        (child_exited || timed_out)
                            .then(latest_transcript_text)
                            .flatten()
                    })
            })
        } else {
            None
        };

        // An auth failure in the transcript takes precedence over classification:
        // it won't self-heal (unlike a usage limit), so surface it immediately.
        if child_exited && flag.is_none() {
            if let Some(ref t) = transcript {
                if is_claude_auth_error(t) {
                    bail!("{CLAUDE_AUTH_ERROR_MSG}");
                }
            }
        }

        let outcome = classify_outcome(flag.as_deref(), timed_out, transcript.as_deref());
        info!(?outcome, child_exited, timed_out, "execution session ended");
        Ok(outcome)
    }
}

/// The actionable message surfaced when a run hits a Claude Code authentication
/// failure — the account is signed out or has never been logged in.
const CLAUDE_AUTH_ERROR_MSG: &str =
    "Claude Code is not authenticated — run `claude login` and retry";

/// Return `true` when `text` shows a Claude Code authentication failure.
/// A logged-out headless `claude -p` prints `Not logged in · Please run /login`
/// on stdout and exits with code 1 (verified against CLI v2.1.170), so without
/// this the failure masquerades as a generic "no plan" (planning) or
/// `Outcome::Stuck` (headless execution) — both of which hide the real cause.
/// The line is a `-p`-only signal: an *interactive* logged-out session instead
/// renders the onboarding/login TUI and stalls, so the live path detects auth
/// failure only when it surfaces in the transcript (mid-session revocation).
/// That gap is benign because `plan` runs headless first and bails here before
/// `execute` is ever reached.
///
/// Detection is per-line and skips `user`/`assistant` transcript records. In
/// `--output-format stream-json` (the plan path) the log carries `tool_result`
/// records whose content is *the files the agent read* — and this adapter's own
/// source documents the `Not logged in · Please run /login` string, so a naive
/// whole-text scan self-triggers the moment a "repo diagnosis" plan reads
/// `lib.rs`. The genuine signal is never a `user`/`assistant` record: it is a
/// CLI-level message (plain text in default `-p`, a `system`/`result` record in
/// stream-json) emitted before the model loop runs. Plain output has no JSON
/// envelope, so its lines are scanned as-is.
fn is_claude_auth_error(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim_start();
        if trimmed.starts_with('{') {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if matches!(
                    v.get("type").and_then(|t| t.as_str()),
                    Some("user") | Some("assistant")
                ) {
                    return false;
                }
            }
        }
        // One AND-group: the genuine CLI banner carries both substrings on the
        // same line, so an AND avoids matching prose that mentions only one.
        ralphy_adapter_support::auth_error(line, &[&["not logged in", "please run /login"]])
    })
}

/// Flatten raw PTY bytes into matchable text: ANSI escape sequences are
/// dropped, and the ones that *position* text (CSI cursor-forward `ESC[nC`,
/// cursor-position `ESC[r;cH`/`f`) become a single space — the interactive TUI
/// separates the words of one visual line with cursor moves instead of spaces
/// (`Not<ESC[1C>logged<ESC[1C>in`), so without this no substring can match.
/// CR/LF also become spaces so a phrase split across writes still joins.
fn strip_pty_escapes(raw: &[u8]) -> String {
    let mut out: Vec<u8> = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        match raw[i] {
            0x1b => {
                i += 1;
                match raw.get(i) {
                    // CSI: params/intermediates until a final byte in 0x40-0x7e.
                    Some(b'[') => {
                        i += 1;
                        while i < raw.len() && !(0x40..=0x7e).contains(&raw[i]) {
                            i += 1;
                        }
                        if matches!(raw.get(i), Some(b'C') | Some(b'H') | Some(b'f')) {
                            out.push(b' ');
                        }
                        i += 1;
                    }
                    // OSC: swallow until BEL or ST (ESC \).
                    Some(b']') => {
                        i += 1;
                        while i < raw.len() {
                            if raw[i] == 0x07 {
                                i += 1;
                                break;
                            }
                            if raw[i] == 0x1b && raw.get(i + 1) == Some(&b'\\') {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    // Two-byte escape (ESC c, ESC =, ...): drop the pair.
                    Some(_) => i += 1,
                    None => {}
                }
            }
            b'\r' | b'\n' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Return `true` when raw interactive PTY output shows the logged-out REPL.
/// The signature is the status-line pair `Not logged in · Run /login`
/// (captured from CLI v2.1.198 — see tests/fixtures/login_tui_exec.log); the
/// headless banner says `Please run /login`, so `run /login` matches both.
/// Both substrings are required, mirroring [`is_claude_auth_error`]'s AND rule.
fn is_login_tui_output(raw: &[u8]) -> bool {
    let text = strip_pty_escapes(raw).to_lowercase();
    text.contains("not logged in") && text.contains("run /login")
}

/// Rolling watch over the live PTY stream for the logged-out login TUI.
///
/// A logged-out *interactive* session renders the login TUI and stalls without
/// exiting, so the `child_exited` auth check never runs and the session used
/// to burn its whole wall budget and surface as a misleading `Timeout`
/// (issue #72). The watch accumulates a bounded tail of the raw output and is
/// consulted on the session's poll cadence.
///
/// Once the JSONL transcript shows any activity the watch disarms for the rest
/// of the session: a live transcript proves the model loop started (a
/// logged-out session never produces one), and from then on agent output that
/// merely *echoes* the signature — reading this source, for instance — must
/// not trip it.
struct LoginTuiWatch {
    buf: Vec<u8>,
    disarmed: bool,
}

impl LoginTuiWatch {
    /// Plenty for the login screen; the TUI redraws, so the signature recurs.
    const MAX_BUF: usize = 32 * 1024;

    fn new() -> Self {
        Self {
            buf: Vec::new(),
            disarmed: false,
        }
    }

    /// Accumulate a PTY chunk (keeps the most recent [`Self::MAX_BUF`] bytes).
    fn feed(&mut self, chunk: &[u8]) {
        if self.disarmed {
            return;
        }
        self.buf.extend_from_slice(chunk);
        if self.buf.len() > Self::MAX_BUF {
            let cut = self.buf.len() - Self::MAX_BUF;
            self.buf.drain(..cut);
        }
    }

    /// Transcript activity observed — stop watching and drop the buffer.
    fn disarm(&mut self) {
        self.disarmed = true;
        self.buf = Vec::new();
    }

    fn detected(&self) -> bool {
        !self.disarmed && is_login_tui_output(&self.buf)
    }
}

/// Map the session's end state to an [`Outcome`]. The flag the Stop hook wrote is
/// authoritative; otherwise usage-limit text in the transcript is
/// [`Outcome::Limit`] (with a parsed reset hint) — it wins over a wall-timeout so
/// the run can resume after the reset — a timeout is [`Outcome::Timeout`], and a
/// quiet exit is [`Outcome::Stuck`].
fn classify_outcome(flag: Option<&str>, timed_out: bool, transcript: Option<&str>) -> Outcome {
    if let Some(f) = flag {
        let f = f.trim();
        if f == "DONE" {
            return Outcome::Done;
        }
        if let Some(reason) = f.strip_prefix("BLOCKED") {
            return Outcome::Blocked(reason.trim().to_string());
        }
    }
    // A usage limit wins over a wall-timeout: the oracle reclassifies a
    // timed-out/exited session to `limit` when the transcript shows one
    // (ralphy.ps1:395-397), preserving the reset hint so the run can resume.
    // Detection is structural (`transcript_limit`), not a substring scan, so the
    // agent reading source that *mentions* a limit cannot fabricate one.
    if let Some(reset) = transcript.and_then(transcript_limit) {
        return Outcome::Limit(reset);
    }
    if timed_out {
        return Outcome::Timeout;
    }
    Outcome::Stuck
}

/// Terminal reason for one headless `-p` call, mirroring `Invoke-ExecLoop`'s
/// returned strings. Mapped to a core [`Outcome`] by [`headless_reason_to_outcome`].
#[derive(Debug, Clone, PartialEq)]
enum HeadlessReason {
    Done,
    Blocked(String),
    Limit(Option<String>),
    Timeout,
    Stuck,
    MaxCalls,
}

/// Classify the result of a single headless `-p` call. Returns the terminal
/// reason if this call ends the loop, or `None` to continue to the next call.
///
/// Priority order mirrors `Invoke-ExecLoop` (ralphy.ps1:283), which checks
/// `Test-LimitText` *first* — a usage limit wins over everything, including a
/// `RALPHY_DONE_EXIT` emitted in the same output, so the run resumes after reset
/// rather than closing the issue:
/// 1. Limit text anywhere → `Limit`.
/// 2. Process did not exit (timed out) → `Timeout`.
/// 3. `RALPHY_BLOCKED_EXIT` in output → `Blocked`.
/// 4. `RALPHY_DONE_EXIT` or zero open steps → `Done`.
/// 5. Otherwise → `None` (continue).
fn classify_exec_call(out: &str, exited: bool, open_steps: usize) -> Option<HeadlessReason> {
    if let Some(reset) = ralphy_adapter_support::detect_limit(out, is_limit_text, parse_reset_hhmm)
    {
        return Some(HeadlessReason::Limit(reset));
    }
    if !exited {
        return Some(HeadlessReason::Timeout);
    }
    if let Some(reason) = ralphy_adapter_support::blocked_reason(out) {
        return Some(HeadlessReason::Blocked(reason));
    }
    if ralphy_adapter_support::done_sentinel(out) || open_steps == 0 {
        return Some(HeadlessReason::Done);
    }
    None
}

/// One transition of the headless loop's decision logic, factored out of
/// [`ClaudeAgent::execute_headless`] so the code the loop runs *is* the code the
/// tests exercise — no transcribed copy that can silently drift.
#[derive(Debug, Clone, PartialEq)]
enum LoopStep {
    /// This call ends the loop with the given reason.
    Terminal(HeadlessReason),
    /// Continue to the next call carrying this no-commit streak.
    Continue(u32),
}

/// Decide the loop's next step from a call's classification and whether it
/// committed. A terminal `classified` reason ends the loop immediately;
/// otherwise the no-commit streak advances and two consecutive no-commit calls
/// are `Stuck` (mirrors `Invoke-ExecLoop`'s `$stuck -ge 2`).
fn headless_step(streak: u32, classified: Option<HeadlessReason>, committed: bool) -> LoopStep {
    if let Some(reason) = classified {
        return LoopStep::Terminal(reason);
    }
    let streak = if committed { 0 } else { streak + 1 };
    if streak >= 2 {
        LoopStep::Terminal(HeadlessReason::Stuck)
    } else {
        LoopStep::Continue(streak)
    }
}

/// Collapse a headless terminal reason onto an existing core [`Outcome`].
/// `MaxCalls` maps to `Stuck` — it is a headless-only safety cap that does not
/// warrant a new core variant (ADR-0002).
fn headless_reason_to_outcome(r: HeadlessReason) -> Outcome {
    match r {
        HeadlessReason::Done => Outcome::Done,
        HeadlessReason::Blocked(s) => Outcome::Blocked(s),
        HeadlessReason::Limit(t) => Outcome::Limit(t),
        HeadlessReason::Timeout => Outcome::Timeout,
        HeadlessReason::Stuck | HeadlessReason::MaxCalls => Outcome::Stuck,
    }
}

/// Whether text looks like a subscription usage/rate-limit notice. Ports the ps1
/// `Test-LimitText` oracle. Used only on the bounded `claude -p` **stdout**
/// channels (plan / headless-exec), never on the live PTY transcript — see
/// [`transcript_limit`] for why a raw scan is unsafe there.
fn is_limit_text(text: &str) -> bool {
    use regex::Regex;
    let re = Regex::new(
        r"(?i)(rate[_ -]?limit|usage limit|session limit|reached your .* limit|limit reached|resets\s+\d)",
    )
    .expect("valid regex");
    re.is_match(text)
}

/// Detect a *genuine* usage-limit banner in a Claude session transcript JSONL,
/// returning `Some(reset_hint)` (the hint itself may be `None`) when found and
/// `None` otherwise.
///
/// This is line-oriented and **anchored on the API-error structure** — the real
/// banner is an assistant line carrying `isApiErrorMessage: true` together with
/// `error: "rate_limit"` or `apiErrorStatus: 429` (verified against a captured
/// 429), or a `rate_limit_event` whose status is `rejected`. A raw substring
/// scan ([`is_limit_text`]) over the whole transcript cannot be used here: the
/// transcript records everything the agent *read and wrote*, so it false-trips
/// the instant the agent touches source that merely mentions "usage limit" /
/// "session limit" — which is exactly what happens when ralphy runs against a
/// repo about rate limiting (its own included, where the test fixtures alone
/// carry the phrase hundreds of times). Only Claude's own injected error line is
/// a limit; prose in tool results and assistant text is not.
///
/// The reset hint is parsed from that error line's own text via
/// [`parse_reset_hhmm`] (e.g. `"You've hit your session limit · resets 8:10am"`
/// → `Some("08:10")`).
fn transcript_limit(jsonl: &str) -> Option<Option<String>> {
    // Shares the line-delimited-JSON scan scaffold with OpenCode's limit parser
    // (`scan_json_lines`); the rate-limit *predicate* and reset-hint *format* stay
    // Claude-specific.
    ralphy_adapter_support::scan_json_lines(jsonl, |v| {
        line_is_rate_limit_error(v)
            .then(|| limit_line_text(v).as_deref().and_then(parse_reset_hhmm))
    })
}

/// Whether a parsed transcript line is Claude's own rate-limit error — either an
/// `isApiErrorMessage` line whose `error`/`apiErrorStatus` marks a rate limit, or
/// a rejected `rate_limit_event`.
fn line_is_rate_limit_error(v: &serde_json::Value) -> bool {
    let api_rate_limited = v
        .get("isApiErrorMessage")
        .and_then(|b| b.as_bool())
        .unwrap_or(false)
        && (v.get("error").and_then(|e| e.as_str()) == Some("rate_limit")
            || v.get("apiErrorStatus").and_then(|s| s.as_u64()) == Some(429));
    let rate_limit_event = v.get("type").and_then(|t| t.as_str()) == Some("rate_limit_event")
        && v.get("rate_limit_info")
            .and_then(|i| i.get("status"))
            .and_then(|s| s.as_str())
            == Some("rejected");
    api_rate_limited || rate_limit_event
}

/// Concatenate the `text` blocks of a transcript line's `message.content`, so the
/// reset hint can be parsed from the banner Claude rendered into it. `None` when
/// no text is present.
fn limit_line_text(v: &serde_json::Value) -> Option<String> {
    let blocks = v.get("message")?.get("content")?.as_array()?;
    let text: String = blocks
        .iter()
        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
        .collect();
    (!text.is_empty()).then_some(text)
}

/// Parse a reset time from a usage-limit transcript. Looks for a pattern like
/// "resets 3pm", "resets 3:00pm", or "resets Tue 12:30am" and converts it to 24h
/// `HH:mm` (minutes default to `00` when absent). When a day-of-week prefixes the
/// time it is captured, Title-cased, and prepended (`"Tue 00:30"`); a bare time
/// stays bare (`"15:00"`). Returns `None` when no match is found. Ports
/// `Get-ResetDateTime`; the optional weekday lets the core compute the next
/// correct occurrence rather than assuming "today".
fn parse_reset_hhmm(text: &str) -> Option<String> {
    use regex::Regex;
    let re = Regex::new(r"(?i)resets\s+(?:([a-z]{3})\s+)?(\d{1,2})(?::(\d{2}))?\s*([ap]m)")
        .expect("valid regex");
    let caps = re.captures(text)?;
    let hour: u32 = caps[2].parse().ok()?;
    let min: u32 = caps.get(3).map_or(Ok(0), |m| m.as_str().parse()).ok()?;
    let ampm = caps[4].to_lowercase();
    let hour24 = match ampm.as_str() {
        "am" => hour % 12,
        _ => (hour % 12) + 12,
    };
    let hhmm = format!("{:02}:{:02}", hour24, min);
    match caps.get(1) {
        Some(wd) => Some(format!("{} {}", title_case_weekday(wd.as_str()), hhmm)),
        None => Some(hhmm),
    }
}

/// Title-case a three-letter weekday abbreviation (`"tue"` → `"Tue"`).
fn title_case_weekday(wd: &str) -> String {
    let mut chars = wd.chars();
    match chars.next() {
        Some(first) => first
            .to_uppercase()
            .chain(chars.flat_map(|c| c.to_lowercase()))
            .collect(),
        None => String::new(),
    }
}

/// The most recent `claude` transcript JSONL under `~/.claude/projects`, read in
/// full, if it was touched in the last 5 minutes. Ports `Get-LatestTranscript`.
fn latest_transcript_text() -> Option<String> {
    let base = dirs_home()?.join(".claude").join("projects");
    let newest = newest_jsonl(&base)?;
    fs::read_to_string(newest).ok()
}

/// Read the newest transcript under `base` that was modified after
/// `transcript_since`. Used by the live PTY loop so a pre-existing transcript
/// from the same project cannot falsely trip a new session.
fn latest_transcript_text_since(
    base: Option<&Path>,
    transcript_since: SystemTime,
) -> Option<String> {
    let newest = newest_jsonl_since(base?, Some(transcript_since))?;
    fs::read_to_string(newest).ok()
}

/// The home directory, from the platform's usual env var. Thin alias over the
/// shared [`ralphy_adapter_support::home_dir`] so the env dance lives in one place.
fn dirs_home() -> Option<PathBuf> {
    ralphy_adapter_support::home_dir()
}

/// Pre-clear the first-run gates that block an *interactive* Claude session for
/// `repo_root`: the workspace-trust dialog AND the theme/onboarding wizard. The
/// headless `-p` planning path is exempt from both, but a live session stalls on
/// either forever waiting for a keypress — so an autonomous orchestrator must
/// grant up front what the operator would otherwise click. (Observed in the wild:
/// on a profile with `hasCompletedOnboarding=false`, every live exec hung at
/// "Choose the text style…" and silently burned the whole budget.) Best-effort:
/// reads `~/.claude.json`, sets the flags, and writes it back, preserving
/// everything else. A failure here just means the live session may stall
/// (surfaced as a Timeout), never a crash.
fn ensure_interactive_session_ready(repo_root: &Path) {
    let Some(home) = dirs_home() else {
        return;
    };
    let cfg_path = home.join(".claude.json");
    let root = fs::read_to_string(&cfg_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    // Claude keys projects by the cwd it is launched with; we launch it at
    // `repo_root`, whose display form uses forward slashes on every platform.
    let key = repo_root.to_string_lossy().replace('\\', "/");
    let updated = with_onboarding_completed(with_workspace_trusted(root, &key));
    if let Ok(s) = serde_json::to_string_pretty(&updated) {
        let _ = fs::write(&cfg_path, s);
    }
}

/// Set `projects[key].hasTrustDialogAccepted = true` on a parsed `~/.claude.json`,
/// creating the `projects` map and the per-project entry as needed and leaving
/// all other content untouched. Pure, so it unit-tests without the filesystem.
fn with_workspace_trusted(mut root: serde_json::Value, key: &str) -> serde_json::Value {
    use serde_json::{json, Value};
    if let Some(obj) = root.as_object_mut() {
        let projects = obj.entry("projects").or_insert_with(|| json!({}));
        if let Some(projects) = projects.as_object_mut() {
            let entry = projects.entry(key.to_string()).or_insert_with(|| json!({}));
            if let Some(entry) = entry.as_object_mut() {
                entry.insert("hasTrustDialogAccepted".into(), Value::Bool(true));
            }
        }
    }
    root
}

/// Mark Claude Code's first-run onboarding wizard complete on a parsed
/// `~/.claude.json`, so an interactive session boots straight into the prompt
/// instead of the "Let's get started" / theme picker. Sets the top-level
/// `hasCompletedOnboarding` flag and seeds a `theme` only when one is absent (so
/// a user's chosen theme is never overwritten). Leaves all other content intact.
/// Pure, so it unit-tests without the filesystem.
fn with_onboarding_completed(mut root: serde_json::Value) -> serde_json::Value {
    use serde_json::{json, Value};
    if let Some(obj) = root.as_object_mut() {
        obj.insert("hasCompletedOnboarding".into(), Value::Bool(true));
        obj.entry("theme").or_insert_with(|| json!("dark"));
    }
    root
}

/// Resolve the `claude` executable to an absolute path, mirroring the ps1
/// oracle's `$Claude` resolution. This matters because the PTY backend rebuilds
/// `PATH` from the Windows registry and ignores runtime `PATH` edits, so a bare
/// `"claude"` fails wherever the install dir isn't on the *persistent* PATH.
/// Falls back to `~/.local/bin/claude[.exe]`, then to the bare name so the spawn
/// error still names it. Delegates to [`ralphy_adapter_support::resolve_program`]
/// so detection (the `ralphy init` env gate) and execution share one resolver and
/// can never disagree about where (or whether) `claude` is installed.
fn resolve_claude_binary() -> std::ffi::OsString {
    ralphy_adapter_support::resolve_program("claude")
}

/// Recursively find the most-recently-modified `*.jsonl` under `base`, but only if
/// it was modified within the last 5 minutes (a stale transcript is irrelevant).
fn newest_jsonl(base: &Path) -> Option<PathBuf> {
    newest_jsonl_since(base, None)
}

fn newest_jsonl_since(base: &Path, min_modified: Option<SystemTime>) -> Option<PathBuf> {
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            if min_modified.is_some_and(|min| modified < min) {
                continue;
            }
            if newest.as_ref().is_none_or(|(t, _)| modified > *t) {
                newest = Some((modified, path));
            }
        }
    }
    let (modified, path) = newest?;
    let age = std::time::SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::ZERO);
    (age < Duration::from_secs(300)).then_some(path)
}

/// Parse the token usage off a headless `claude -p --output-format stream-json`
/// stdout (ADR-0008 D5, plan path). The stream is preceded by a human-readable
/// warning preamble ("no stdin data received in 3s…") and interleaves event
/// lines, so only lines whose trimmed start is `{` are JSON-parsed; the LAST
/// `{"type":"result",…}` object's `usage` is the authoritative per-invocation
/// total. Reads the four Messages-API fields and a model id (the `modelUsage`
/// map key, else `usage.model`). Returns `Usage::default()` when no result line
/// is found.
fn parse_plan_usage(stdout: &str) -> Usage {
    let mut found: Option<Usage> = None;
    for line in stdout.lines() {
        let line = line.trim_start();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|t| t.as_str()) != Some("result") {
            continue;
        }
        let Some(usage) = value.get("usage") else {
            continue;
        };
        let field = |k: &str| usage.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        let mut u = Usage {
            input: field("input_tokens"),
            output: field("output_tokens"),
            cache_read: field("cache_read_input_tokens"),
            cache_creation: field("cache_creation_input_tokens"),
            model: None,
        };
        // The model id resolves the price table (D8): prefer the *dominant*
        // `modelUsage` key — the main model the top-level `usage` block accounts
        // for — falling back to a `usage.model` field. Picking the dominant entry
        // (not the first) matters because Claude Code also bills a tiny amount to
        // a background model (e.g. haiku) for auxiliary work; that entry sorts
        // first alphabetically, so `keys().next()` mislabeled the whole phase as
        // haiku — and, being a dated id, missed the price table entirely.
        u.model = value
            .get("modelUsage")
            .and_then(|m| m.as_object())
            .and_then(dominant_model_key)
            .or_else(|| {
                usage
                    .get("model")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            });
        found = Some(u); // keep the LAST result object
    }
    found.unwrap_or_default()
}

/// Sum a session's per-transcript usages and attribute the phase to one model.
/// `add_tokens` drops `model` by design (it sums across records), so a plain fold
/// would leave the phase unattributed → the `unknown` pricing bucket even though
/// each transcript already resolved its own dominant model. Carry the model of the
/// heaviest transcript, falling back to `fallback_model` (the model we requested)
/// so a real, priceable id is always recorded rather than `unknown` (ADR-0008 D8).
fn fold_exec_usage(per_transcript: &[Usage], fallback_model: &str) -> Usage {
    let mut usage = per_transcript.iter().fold(Usage::default(), |mut acc, u| {
        acc.add_tokens(u);
        acc
    });
    usage.model = per_transcript
        .iter()
        .filter(|u| u.model.is_some())
        .max_by_key(|u| u.total())
        .and_then(|u| u.model.clone())
        .or_else(|| Some(fallback_model.to_string()));
    usage
}

/// The key of the `modelUsage` entry with the most tokens — the run's *main*
/// model, the one the top-level `usage` block accounts for. `None` for an empty
/// map. Ties resolve to the last-seen max, which is immaterial (a tie means equal
/// spend, so the price is the same either way for the figures that matter).
fn dominant_model_key(model_usage: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    model_usage
        .iter()
        .max_by_key(|(_, entry)| model_usage_total(entry))
        .map(|(k, _)| k.clone())
}

/// Sum a `modelUsage` entry's token counts. These fields are **camelCase**
/// (`inputTokens`, `cacheReadInputTokens`, …), unlike the snake_case top-level
/// `usage` block — Claude Code reports the per-model breakdown in the other case.
fn model_usage_total(entry: &serde_json::Value) -> u64 {
    let f = |k: &str| entry.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    f("inputTokens") + f("outputTokens") + f("cacheReadInputTokens") + f("cacheCreationInputTokens")
}

/// Encode a launch cwd the way Claude Code names its `~/.claude/projects/<dir>`
/// transcript folder (ADR-0008 D10): every non-ASCII-alphanumeric character maps
/// to `-`, drive-letter case preserved. So `c:\Dev\ralphy` → `c--Dev-ralphy` and
/// `C:\Dev\.ralph-worktrees\issue-10` → `C--Dev--ralph-worktrees-issue-10` (the
/// dot becomes a second `-`). Pure over the byte-exact cwd string.
fn dashed_cwd(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Sum `cache_creation` tokens from a transcript `usage` block: prefer the flat
/// `cache_creation_input_tokens`, else sum the `cache_creation` 5m/1h ephemeral
/// sub-tiers (they total to the flat field, so taking the flat first avoids
/// double-counting). ADR-0008 D5/D10.
fn cache_creation_tokens(usage: &serde_json::Value) -> u64 {
    if let Some(flat) = usage
        .get("cache_creation_input_tokens")
        .and_then(|v| v.as_u64())
    {
        return flat;
    }
    if let Some(obj) = usage.get("cache_creation").and_then(|v| v.as_object()) {
        let tier = |k: &str| obj.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        return tier("ephemeral_5m_input_tokens") + tier("ephemeral_1h_input_tokens");
    }
    0
}

/// Parse and sum the token usage across a Claude-exec transcript JSONL (ADR-0008
/// D5/D10). Two traps the spike found are load-bearing here: **dedup by
/// `message.id`** (resume/branch replays and parallel-tool-call lines reuse one
/// id; a naïve sum overcounts ~2.8×) and **never descending into the nested
/// `iterations[]`** array (it repeats the top-level `usage`). Only the top-level
/// `message.usage` of each unique `message.id` is summed.
fn parse_transcript_usage(jsonl: &str) -> Usage {
    use std::collections::{BTreeMap, HashSet};
    let mut seen: HashSet<String> = HashSet::new();
    let mut total = Usage::default();
    // Per-model token tallies so the price table can resolve on the *dominant*
    // model (D8) — mirrors `parse_plan_usage`'s `modelUsage` attribution. Without
    // this every execute row was written `model: None` → `unknown` in the ledger,
    // leaving the bulk of a run's spend unpriced (`~$?`) in `ralphy usage`.
    let mut by_model: BTreeMap<String, u64> = BTreeMap::new();
    for line in jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(message) = value.get("message") else {
            continue;
        };
        // Mandatory dedup: count one `usage` per unique `message.id`.
        if let Some(id) = message.get("id").and_then(|v| v.as_str()) {
            if !seen.insert(id.to_string()) {
                continue;
            }
        }
        let Some(usage) = message.get("usage") else {
            continue;
        };
        let field = |k: &str| usage.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        // Only the top-level `message.usage` is read; `iterations[]` is never
        // descended into, so its repeated `usage` is ignored by construction.
        let input = field("input_tokens");
        let output = field("output_tokens");
        let cache_read = field("cache_read_input_tokens");
        let cache_creation = cache_creation_tokens(usage);
        total.input += input;
        total.output += output;
        total.cache_read += cache_read;
        total.cache_creation += cache_creation;
        // Attribute this line's tokens to its assistant `message.model` so the
        // dominant model can be picked once the whole transcript is summed.
        if let Some(m) = message.get("model").and_then(|v| v.as_str()) {
            *by_model.entry(m.to_string()).or_insert(0) +=
                input + output + cache_read + cache_creation;
        }
    }
    // The dominant model (most tokens) is the one the price table resolves on; a
    // tie resolves to the last key, which is immaterial (equal spend → same price
    // for the figures that matter). `None` when no line carried a `model`.
    total.model = by_model.into_iter().max_by_key(|(_, n)| *n).map(|(k, _)| k);
    total
}

/// Index of the first occurrence of `needle` in `haystack`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Rolling-tail DSR scanner. Appends `chunk` to `carry`, searches the combined
/// buffer for `CURSOR_POSITION_REQUEST`, then truncates `carry` to the last
/// `CURSOR_POSITION_REQUEST.len() - 1` bytes so a split sequence spanning the
/// next chunk can still match. Returns `true` if the sequence was found.
fn scan_dsr_request(carry: &mut Vec<u8>, chunk: &[u8]) -> bool {
    carry.extend_from_slice(chunk);
    let found = find_subslice(carry, CURSOR_POSITION_REQUEST).is_some();
    let keep = CURSOR_POSITION_REQUEST.len().saturating_sub(1);
    if carry.len() > keep {
        carry.drain(..carry.len() - keep);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralphy_core::{Issue, Plan};
    use std::path::PathBuf;

    /// Anti-drift: the charter this adapter launches sessions with and the
    /// embedded execution prompt must both name the shared completion sentinel;
    /// `ralphy_adapter_support::DONE_SENTINEL` is the single source of truth.
    #[test]
    fn charter_and_prompt_name_the_done_sentinel() {
        assert!(EXEC_CHARTER.contains(ralphy_adapter_support::DONE_SENTINEL));
        assert!(PROMPT_EXECUTE.contains(ralphy_adapter_support::DONE_SENTINEL));
    }

    /// The charter file must carry whichever prompt `plan_prompt_for` selected,
    /// and a rewrite (as every `plan()` call performs) must replace a staged
    /// charter with the standard one after a label switch.
    #[test]
    fn plan_charter_file_carries_selected_prompt() {
        let base = std::env::temp_dir().join(format!("ralphy-plan-charter-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let ws = Workspace::new(&base);
        fs::create_dir_all(ws.ralphy_dir()).unwrap();

        let (staged_prompt, _) = plan_prompt_for(&issue_with_labels(&["stagedplan"]));
        write_plan_charter(&ws, staged_prompt).expect("write staged charter");
        assert_eq!(
            fs::read_to_string(ws.plan_charter_path()).unwrap(),
            PROMPT_PLAN_STAGED,
            "staged issue must put the staged charter on disk"
        );

        let (standard_prompt, _) = plan_prompt_for(&issue_with_labels(&["bug"]));
        write_plan_charter(&ws, standard_prompt).expect("rewrite standard charter");
        assert_eq!(
            fs::read_to_string(ws.plan_charter_path()).unwrap(),
            PROMPT_PLAN,
            "the per-call rewrite must replace a stale staged charter"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// The per-issue stdin payload must stay a one-line pointer, not regrow
    /// into the full charter — pins the byte reduction issue #80 delivers.
    #[test]
    fn plan_pointer_is_a_pointer_not_the_charter() {
        let pointer = ralphy_adapter_support::PLAN_CHARTER;
        assert!(pointer.len() * 50 < PROMPT_PLAN.len());
        assert!(pointer.len() * 50 < PROMPT_PLAN_STAGED.len());
    }

    #[test]
    fn scan_dsr_request_detects_split_sequence() {
        // Sequence split across two chunks: first call must return false, second true.
        let mut carry = Vec::new();
        assert!(
            !scan_dsr_request(&mut carry, b"\x1b["),
            "partial prefix should not fire"
        );
        assert!(
            scan_dsr_request(&mut carry, b"6n"),
            "completing the sequence should fire"
        );

        // Unsplit: a single chunk containing the full sequence fires immediately.
        let mut carry2 = Vec::new();
        assert!(
            scan_dsr_request(&mut carry2, CURSOR_POSITION_REQUEST),
            "full sequence in one chunk should fire"
        );

        // No sequence at all: never fires.
        let mut carry3 = Vec::new();
        assert!(
            !scan_dsr_request(&mut carry3, b"hello world"),
            "unrelated bytes should not fire"
        );
    }

    fn issue_with_labels(labels: &[&str]) -> Issue {
        Issue {
            number: 1,
            title: "test".into(),
            body: String::new(),
            labels: labels.iter().map(|s| s.to_string()).collect(),
            comments: vec![],
        }
    }

    #[test]
    fn plan_prompt_for_selects_staged_when_label_present() {
        let issue = issue_with_labels(&["bug", "stagedplan"]);
        let (prompt, staged) = plan_prompt_for(&issue);
        assert!(staged, "should be staged when 'stagedplan' label present");
        assert_eq!(
            prompt, PROMPT_PLAN_STAGED,
            "should use the staged plan prompt"
        );
    }

    #[test]
    fn materialize_plugin_extracts_required_skills() {
        // The embedded plugin must carry the skills the prompts invoke; a run
        // provisions them into .ralphy/plugin so claude finds them via
        // --plugin-dir without depending on globally-installed skills.
        let base = std::env::temp_dir().join(format!("ralphy-plugin-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let ws = Workspace::new(&base);

        let dir = materialize_plugin(&ws).expect("materialize");
        assert_eq!(dir, plugin_dir(&ws));
        assert!(
            dir.join(".claude-plugin/plugin.json").is_file(),
            "plugin manifest must be materialized"
        );
        assert!(
            dir.join("skills/reviewer/SKILL.md").is_file(),
            "reviewer skill must be materialized"
        );
        assert!(
            dir.join("skills/staged-plan/SKILL.md").is_file(),
            "staged-plan skill must be materialized"
        );
        // Multi-file skills must come through whole, not just the SKILL.md.
        assert!(
            dir.join("skills/reviewer/scripts/audit.py").is_file(),
            "reviewer helper scripts must be materialized"
        );

        // Idempotent: a second call clears and re-extracts cleanly.
        materialize_plugin(&ws).expect("re-materialize");
        assert!(dir.join("skills/reviewer/SKILL.md").is_file());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn plan_prompt_for_selects_standard_without_label() {
        let issue = issue_with_labels(&["bug", "ready-for-agent"]);
        let (prompt, staged) = plan_prompt_for(&issue);
        assert!(
            !staged,
            "should not be staged when 'stagedplan' label absent"
        );
        assert_eq!(prompt, PROMPT_PLAN, "should use the standard plan prompt");
    }

    #[test]
    fn plan_prompt_for_not_staged_with_no_labels() {
        let issue = issue_with_labels(&[]);
        let (_, staged) = plan_prompt_for(&issue);
        assert!(!staged);
    }

    #[test]
    fn staged_plan_env_set_when_staged() {
        assert_eq!(
            staged_plan_env(true),
            Some(("STAGED_PLAN_NONINTERACTIVE", "1")),
            "a staged plan must flag the skill as non-interactive"
        );
    }

    #[test]
    fn staged_plan_env_absent_when_not_staged() {
        assert_eq!(
            staged_plan_env(false),
            None,
            "the standard plan must not touch the environment"
        );
    }

    #[test]
    fn staged_prompt_and_skill_reference_noninteractive_flag() {
        // Anti-drift: the env the adapter sets is only meaningful if the staged
        // prompt (and the skill it invokes) still read STAGED_PLAN_NONINTERACTIVE.
        assert!(
            PROMPT_PLAN_STAGED.contains("STAGED_PLAN_NONINTERACTIVE"),
            "staged plan prompt must reference the non-interactive flag it is handed"
        );
        let skill = include_str!("../../../assets/plugin/skills/staged-plan/SKILL.md");
        assert!(
            skill.contains("STAGED_PLAN_NONINTERACTIVE"),
            "staged-plan skill must read the non-interactive flag"
        );
    }

    #[test]
    fn reads_recommended_model() {
        assert_eq!(
            recommended_model("## Execution model: Opus\nbecause").as_deref(),
            Some("opus")
        );
        assert_eq!(recommended_model("no judgment here"), None);
    }

    fn plan_with(recommended: Option<&str>) -> Plan {
        Plan {
            path: PathBuf::from("/x/plan.md"),
            open_steps: 3,
            recommended_model: recommended.map(str::to_string),
            usage: Usage::default(),
        }
    }

    fn agent_with(exec_model: Option<&str>, default: &str) -> ClaudeAgent {
        ClaudeAgent::new(None, None, PathBuf::from("/run")).with_exec_config(
            exec_model.map(str::to_string),
            Some("medium".into()),
            default.to_string(),
            45,
            true,
            false,
            6,
        )
    }

    fn agent_with_minutes(minutes: u64) -> ClaudeAgent {
        ClaudeAgent::new(None, None, PathBuf::from("/run")).with_exec_config(
            None,
            Some("medium".into()),
            "sonnet".into(),
            minutes,
            true,
            false,
            6,
        )
    }

    #[test]
    fn issue_deadline_zero_minutes_disables_the_cap() {
        // `0` → no per-issue cap: the deadline sits past any finite budget…
        let uncapped = agent_with_minutes(0);
        let capped = agent_with_minutes(1000);
        assert!(uncapped.issue_deadline() > capped.issue_deadline());

        // …yet the run deadline still bounds an uncapped issue when present.
        let rd = Instant::now() + Duration::from_secs(1);
        let bounded = agent_with_minutes(0).with_run_deadline(Some(rd));
        assert!(bounded.issue_deadline() <= rd);
    }

    #[test]
    fn exec_model_explicit_override_wins() {
        let agent = agent_with(Some("opus"), "sonnet");
        assert_eq!(agent.resolve_exec_model(&plan_with(Some("sonnet"))), "opus");
    }

    #[test]
    fn exec_model_falls_back_to_plan_judgment() {
        let agent = agent_with(None, "sonnet");
        assert_eq!(agent.resolve_exec_model(&plan_with(Some("opus"))), "opus");
    }

    #[test]
    fn exec_model_falls_back_to_default() {
        let agent = agent_with(None, "sonnet");
        assert_eq!(agent.resolve_exec_model(&plan_with(None)), "sonnet");
    }

    #[test]
    fn settings_have_stop_hook_pretooluse_guard_and_posttooluse_timer() {
        let json = exec_settings_json(
            "\"ralphy.exe\" hook stop",
            "\"ralphy.exe\" hook guard",
            "\"ralphy.exe\" hook post",
        );
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["skipDangerousModePermissionPrompt"], true);
        assert_eq!(v["skipAutoPermissionPrompt"], true);
        assert_eq!(v["autoCompactEnabled"], false);
        // Stop hook still present.
        let stop_cmd = &v["hooks"]["Stop"][0]["hooks"][0]["command"];
        assert_eq!(stop_cmd, "\"ralphy.exe\" hook stop");
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["type"], "command");
        // PreToolUse guard is wired.
        let guard_matcher = &v["hooks"]["PreToolUse"][0]["matcher"];
        assert_eq!(guard_matcher, "Bash|Edit|Write|MultiEdit|NotebookEdit");
        let guard_cmd = &v["hooks"]["PreToolUse"][0]["hooks"][0]["command"];
        assert_eq!(guard_cmd, "\"ralphy.exe\" hook guard");
        assert_eq!(v["hooks"]["PreToolUse"][0]["hooks"][0]["type"], "command");
        // PostToolUse Bash timer (verification-cost gate) is wired.
        assert_eq!(v["hooks"]["PostToolUse"][0]["matcher"], "Bash");
        let post_cmd = &v["hooks"]["PostToolUse"][0]["hooks"][0]["command"];
        assert_eq!(post_cmd, "\"ralphy.exe\" hook post");
        assert_eq!(v["hooks"]["PostToolUse"][0]["hooks"][0]["type"], "command");
    }

    #[test]
    fn classify_done_from_flag() {
        assert_eq!(classify_outcome(Some("DONE\n"), false, None), Outcome::Done);
    }

    #[test]
    fn classify_blocked_from_flag() {
        assert_eq!(
            classify_outcome(Some("BLOCKED missing key"), false, None),
            Outcome::Blocked("missing key".into())
        );
    }

    /// One real transcript api-error line carrying the limit banner `text`, in the
    /// exact shape Claude Code writes (`isApiErrorMessage`+`error`+`apiErrorStatus`).
    fn limit_jsonl(text: &str) -> String {
        serde_json::json!({
            "type": "assistant",
            "isApiErrorMessage": true,
            "error": "rate_limit",
            "apiErrorStatus": 429,
            "message": { "role": "assistant", "content": [ { "type": "text", "text": text } ] }
        })
        .to_string()
    }

    #[test]
    fn classify_limit_beats_timeout() {
        // A timed-out session whose transcript shows a real rate-limit error line
        // classifies as Limit (oracle parity, ralphy.ps1:395-397) so the run
        // resumes after reset.
        let t = limit_jsonl("You've hit your usage limit");
        assert_eq!(classify_outcome(None, true, Some(&t)), Outcome::Limit(None));
    }

    #[test]
    fn classify_timeout_when_no_limit_in_transcript() {
        assert_eq!(
            classify_outcome(None, true, Some("just a normal log")),
            Outcome::Timeout
        );
    }

    #[test]
    fn classify_limit_from_transcript() {
        let t = limit_jsonl("You've reached your usage limit; resets 3:00pm");
        assert_eq!(
            classify_outcome(None, false, Some(&t)),
            Outcome::Limit(Some("15:00".into()))
        );
    }

    #[test]
    fn classify_limit_from_subagent_session_limit_transcript() {
        // The exact line Claude Code records when the session cap is hit while the
        // interactive PTY remains alive (captured from a real 429).
        let t = limit_jsonl("You've hit your session limit · resets 8:10am (America/Bahia)");
        assert_eq!(
            classify_outcome(None, false, Some(&t)),
            Outcome::Limit(Some("08:10".into()))
        );
    }

    #[test]
    fn classify_does_not_trip_on_source_that_mentions_limits() {
        // THE REGRESSION GUARD: running ralphy on a repo about rate limiting (its
        // own included) fills the transcript with tool results and assistant text
        // that say "usage limit" / "session limit" / "resets 3:00pm" — none of
        // which is a real limit. A structural detector must ignore all of it.
        let transcript = concat!(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"fn is_limit_text(text) { /* rate limit, usage limit, session limit */ }\nassert_eq!(parse_reset_hhmm(\"resets 3:00pm\"), Some(\"15:00\"));"}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"I'll wire the usage limit handling so a session limit auto-resumes after reset."}]}}"#,
        );
        assert_eq!(transcript_limit(transcript), None);
        // ...and a timed-out session over that transcript is a Timeout, not a Limit.
        assert_eq!(
            classify_outcome(None, true, Some(transcript)),
            Outcome::Timeout
        );
    }

    #[test]
    fn transcript_limit_detects_real_429_error_line() {
        let t = limit_jsonl("You've hit your session limit · resets 8:10am (America/Bahia)");
        assert_eq!(transcript_limit(&t), Some(Some("08:10".into())));
    }

    #[test]
    fn transcript_limit_detects_rejected_rate_limit_event() {
        let t = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected"}}"#;
        assert_eq!(transcript_limit(t), Some(None));
    }

    #[test]
    fn limit_text_matches_claude_rate_limit_event() {
        let log = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected"}}"#;
        assert!(is_limit_text(log));
    }

    #[test]
    fn limit_text_matches_session_limit_message() {
        let log = "You've hit your session limit · resets 8:10am (America/Bahia)";
        assert!(is_limit_text(log));
        assert_eq!(parse_reset_hhmm(log), Some("08:10".into()));
    }

    #[test]
    fn parse_reset_hhmm_converts_pm() {
        assert_eq!(parse_reset_hhmm("resets 3:00pm"), Some("15:00".into()));
    }

    #[test]
    fn parse_reset_hhmm_midnight() {
        assert_eq!(parse_reset_hhmm("resets 12:30am"), Some("00:30".into()));
    }

    #[test]
    fn parse_reset_hhmm_without_minutes() {
        assert_eq!(parse_reset_hhmm("resets 3pm"), Some("15:00".into()));
        assert_eq!(parse_reset_hhmm("resets 12am"), Some("00:00".into()));
    }

    #[test]
    fn parse_reset_hhmm_no_match() {
        assert_eq!(parse_reset_hhmm("no time here"), None);
    }

    #[test]
    fn parse_reset_hhmm_captures_weekday() {
        // A weekday-qualified reset is captured and prefixed, Title-cased; the
        // bare-time form is unchanged.
        assert_eq!(
            parse_reset_hhmm("You've reached your usage limit; resets Tue 12:30am"),
            Some("Tue 00:30".into())
        );
        assert_eq!(parse_reset_hhmm("resets 3:00pm"), Some("15:00".into()));
    }

    #[test]
    fn workspace_trust_sets_flag_and_preserves_other_content() {
        use serde_json::json;

        // Existing config with an unrelated project and a top-level key.
        let root = json!({
            "numStartups": 7,
            "projects": { "C:/other": { "hasTrustDialogAccepted": false, "keep": 1 } }
        });
        let out = with_workspace_trusted(root, "C:/ws");

        // The new workspace is trusted...
        assert_eq!(out["projects"]["C:/ws"]["hasTrustDialogAccepted"], true);
        // ...and nothing else was disturbed.
        assert_eq!(out["numStartups"], 7);
        assert_eq!(out["projects"]["C:/other"]["hasTrustDialogAccepted"], false);
        assert_eq!(out["projects"]["C:/other"]["keep"], 1);
    }

    #[test]
    fn workspace_trust_bootstraps_empty_config() {
        let out = with_workspace_trusted(serde_json::json!({}), "C:/ws");
        assert_eq!(out["projects"]["C:/ws"]["hasTrustDialogAccepted"], true);
    }

    #[test]
    fn onboarding_completed_sets_flag_and_seeds_theme_once() {
        use serde_json::json;

        // No theme yet → flag set and a default theme seeded.
        let out = with_onboarding_completed(json!({ "numStartups": 7 }));
        assert_eq!(out["hasCompletedOnboarding"], true);
        assert_eq!(out["theme"], "dark");
        assert_eq!(out["numStartups"], 7);

        // An existing theme is never overwritten.
        let out = with_onboarding_completed(json!({ "theme": "light" }));
        assert_eq!(out["hasCompletedOnboarding"], true);
        assert_eq!(out["theme"], "light");
    }

    #[test]
    fn classify_stuck_when_quiet_exit() {
        assert_eq!(
            classify_outcome(None, false, Some("just a normal log")),
            Outcome::Stuck
        );
        assert_eq!(classify_outcome(None, false, None), Outcome::Stuck);
    }

    // ── is_claude_auth_error ────────────────────────────────────────────────

    #[test]
    fn is_claude_auth_error_matches_logged_out_output() {
        assert!(is_claude_auth_error(
            "Not logged in \u{00b7} Please run /login"
        ));
    }

    #[test]
    fn is_claude_auth_error_matches_case_insensitive() {
        assert!(is_claude_auth_error(
            "NOT LOGGED IN \u{00b7} PLEASE RUN /LOGIN"
        ));
    }

    #[test]
    fn is_claude_auth_error_requires_both_signals() {
        assert!(!is_claude_auth_error("Not logged in"));
        assert!(!is_claude_auth_error("Please run /login"));
        assert!(!is_claude_auth_error("all steps green\nRALPHY_DONE_EXIT\n"));
    }

    /// Raw PTY bytes of a logged-out interactive session (CLI v2.1.198),
    /// captured on Windows ConPTY: the REPL renders with a
    /// `Not logged in · Run /login` status line whose words are separated by
    /// cursor-forward escapes, not spaces.
    const LOGIN_TUI_FIXTURE: &[u8] = include_bytes!("../tests/fixtures/login_tui_exec.log");

    #[test]
    fn login_tui_fixture_detected() {
        assert!(is_login_tui_output(LOGIN_TUI_FIXTURE));
    }

    #[test]
    fn normal_pty_output_not_detected() {
        // ANSI-heavy healthy-session shapes: a working REPL status line and
        // agent prose that mentions login without the logged-out signature.
        let healthy = b"\x1b[38;2;153;153;153m\x1b[17;3H?\x1b[1Cfor\x1b[1Cshortcuts\
            \x1b[18;83H\x1b[1Chigh\x1b[1C\xc2\xb7\x1b[1C/effort\x1b[m\r\n\
            Running\x1b[1Ccargo\x1b[1Ctest...\r\n";
        assert!(!is_login_tui_output(healthy));
        assert!(!is_login_tui_output(
            b"the user is logged in \xc2\xb7 no action"
        ));
    }

    #[test]
    fn strip_pty_escapes_turns_cursor_moves_into_spaces() {
        // The fixture's exact word-separation shape: `ESC[1C` between words.
        let raw = b"\x1b[38;2;255;107;128mNot\x1b[1Clogged\x1b[1Cin\x1b[1C\xc2\xb7\x1b[1CRun\x1b[1C/login\x1b[38;2;153;153;153m";
        assert_eq!(strip_pty_escapes(raw), "Not logged in \u{b7} Run /login");
    }

    #[test]
    fn strip_pty_escapes_drops_osc_and_csi() {
        let raw = b"\x1b]0;claude\x07plain\x1b[2mtext\x1b[m";
        assert_eq!(strip_pty_escapes(raw), "plaintext");
    }

    /// Live end-to-end proof for issue #72: spawn a real logged-out `claude`
    /// in a PTY (isolated `CLAUDE_CONFIG_DIR`, onboarding pre-completed,
    /// workspace pre-trusted, no credentials) and assert the watch flags it on
    /// the same poll cadence `drive_session` uses. Needs the `claude` binary
    /// and ~15s, so it is opt-in: `cargo test -p ralphy-agent-claude -- --ignored`.
    #[test]
    #[ignore = "spawns the real claude CLI; run manually with -- --ignored"]
    fn live_logged_out_interactive_session_is_detected() {
        use std::io::Read as _;
        use std::sync::mpsc;

        let base = std::env::temp_dir().join(format!("ralphy-login-e2e-{}", std::process::id()));
        let cfg_dir = base.join("cfg");
        let work_dir = base.join("ws");
        fs::create_dir_all(&cfg_dir).unwrap();
        fs::create_dir_all(&work_dir).unwrap();
        let key = work_dir.to_string_lossy().replace('\\', "/");
        fs::write(
            cfg_dir.join(".claude.json"),
            serde_json::json!({
                "hasCompletedOnboarding": true,
                "theme": "dark",
                "projects": { key: { "hasTrustDialogAccepted": true } },
            })
            .to_string(),
        )
        .unwrap();

        let cmd = PtyCommand::new(resolve_claude_binary())
            .cwd(&work_dir)
            .env("CLAUDE_CONFIG_DIR", cfg_dir.as_os_str())
            .size(30, 100);
        let mut session = PtySession::spawn(cmd).expect("spawning claude");
        let mut reader = session.reader().unwrap();
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });

        let mut watch = LoginTuiWatch::new();
        let mut dsr_carry: Vec<u8> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        let detected = loop {
            while let Ok(chunk) = rx.try_recv() {
                if scan_dsr_request(&mut dsr_carry, &chunk) {
                    let _ = session.write_all(CURSOR_POSITION_REPLY);
                }
                watch.feed(&chunk);
            }
            if watch.detected() {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            thread::sleep(Duration::from_millis(500));
        };
        let _ = session.kill();
        let _ = fs::remove_dir_all(&base);
        assert!(
            detected,
            "a logged-out interactive claude session must be flagged as an auth failure"
        );
    }

    #[test]
    fn login_watch_detects_across_chunks_and_disarms_on_transcript() {
        // The signature arrives split across PTY chunks.
        let mut watch = LoginTuiWatch::new();
        let mid = LOGIN_TUI_FIXTURE.len() / 2;
        watch.feed(&LOGIN_TUI_FIXTURE[..mid]);
        watch.feed(&LOGIN_TUI_FIXTURE[mid..]);
        assert!(watch.detected());

        // Once the transcript shows activity the watch must stay quiet even if
        // the signature bytes appear again (agent echoing this source).
        let mut watch = LoginTuiWatch::new();
        watch.disarm();
        watch.feed(LOGIN_TUI_FIXTURE);
        assert!(!watch.detected());
    }

    #[test]
    fn is_claude_auth_error_ignores_file_content_in_tool_results() {
        // A "repo diagnosis" plan reads this adapter's own source, whose doc
        // comment quotes `Not logged in · Please run /login`. In stream-json the
        // read lands in a `type":"user"` tool_result — it must NOT be read as a
        // real auth failure (regression: run 20260625-145058).
        let line = serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": [{
                "type": "tool_result",
                "content": "//! prints `Not logged in \u{00b7} Please run /login` on stdout",
            }]},
        })
        .to_string();
        assert!(!is_claude_auth_error(&line));
    }

    #[test]
    fn is_claude_auth_error_ignores_assistant_prose() {
        // The planning agent may *describe* the auth detector in its own message;
        // an assistant record is never the genuine CLI signal.
        let line = serde_json::json!({
            "type": "assistant",
            "message": {"role": "assistant", "content": [{
                "type": "text",
                "text": "It checks for `Not logged in \u{00b7} Please run /login`.",
            }]},
        })
        .to_string();
        assert!(!is_claude_auth_error(&line));
    }

    #[test]
    fn is_claude_auth_error_detects_real_signal_amid_tool_results() {
        // The genuine CLI message (plain line in default `-p`, or a non-user
        // record in stream-json) still fires even when file-content noise
        // precedes it.
        let log = format!(
            "{}\nNot logged in \u{00b7} Please run /login\n",
            serde_json::json!({
                "type": "user",
                "message": {"content": [{"type": "tool_result", "content": "harmless file body"}]},
            })
        );
        assert!(is_claude_auth_error(&log));
    }

    // ── classify_exec_call ──────────────────────────────────────────────────

    #[test]
    fn classify_exec_not_exited_with_limit_text_is_limit() {
        let out = "You've reached your usage limit; resets 3:00pm";
        assert_eq!(
            classify_exec_call(out, false, 5),
            Some(HeadlessReason::Limit(Some("15:00".into())))
        );
    }

    #[test]
    fn classify_exec_not_exited_without_limit_is_timeout() {
        assert_eq!(
            classify_exec_call("partial output", false, 5),
            Some(HeadlessReason::Timeout)
        );
    }

    #[test]
    fn classify_exec_blocked_sentinel() {
        let out = "some work\nRALPHY_BLOCKED_EXIT missing key\nmore text";
        assert_eq!(
            classify_exec_call(out, true, 5),
            Some(HeadlessReason::Blocked("missing key".into()))
        );
    }

    #[test]
    fn classify_exec_done_via_done_sentinel() {
        let out = "all done\nRALPHY_DONE_EXIT\n";
        assert_eq!(classify_exec_call(out, true, 3), Some(HeadlessReason::Done));
    }

    #[test]
    fn classify_exec_done_via_zero_open_steps() {
        assert_eq!(
            classify_exec_call("no sentinel", true, 0),
            Some(HeadlessReason::Done)
        );
    }

    #[test]
    fn classify_exec_limit_on_natural_exit() {
        let out = "You've reached your usage limit; resets 3:00pm";
        assert_eq!(
            classify_exec_call(out, true, 2),
            Some(HeadlessReason::Limit(Some("15:00".into())))
        );
    }

    #[test]
    fn classify_exec_limit_beats_done_sentinel() {
        // The oracle checks Test-LimitText first (ralphy.ps1:283): a usage limit
        // wins even when the same exited call emitted RALPHY_DONE_EXIT, so the
        // run resumes after reset instead of closing the issue.
        let out = "RALPHY_DONE_EXIT\nYou've reached your usage limit; resets 3:00pm";
        assert_eq!(
            classify_exec_call(out, true, 0),
            Some(HeadlessReason::Limit(Some("15:00".into())))
        );
    }

    #[test]
    fn classify_exec_continue_when_no_terminal_condition() {
        assert_eq!(
            classify_exec_call("partial progress, no sentinel", true, 3),
            None
        );
    }

    // ── headless_reason_to_outcome ──────────────────────────────────────────

    #[test]
    fn headless_reason_done_maps_to_done() {
        assert_eq!(
            headless_reason_to_outcome(HeadlessReason::Done),
            Outcome::Done
        );
    }

    #[test]
    fn headless_reason_blocked_maps_to_blocked() {
        assert_eq!(
            headless_reason_to_outcome(HeadlessReason::Blocked("reason".into())),
            Outcome::Blocked("reason".into())
        );
    }

    #[test]
    fn headless_reason_timeout_maps_to_timeout() {
        assert_eq!(
            headless_reason_to_outcome(HeadlessReason::Timeout),
            Outcome::Timeout
        );
    }

    #[test]
    fn headless_reason_stuck_maps_to_stuck() {
        assert_eq!(
            headless_reason_to_outcome(HeadlessReason::Stuck),
            Outcome::Stuck
        );
    }

    #[test]
    fn headless_reason_maxcalls_maps_to_stuck() {
        assert_eq!(
            headless_reason_to_outcome(HeadlessReason::MaxCalls),
            Outcome::Stuck
        );
    }

    // ── loop-driver: stuck counter and MaxCalls ─────────────────────────────

    /// Drive the *production* `headless_step` over a scripted sequence, mirroring
    /// only the trivial `for i in 1..=max` bound in `execute_headless`. The
    /// decision logic under test is the real `headless_step`, not a copy — so a
    /// change to the loop's branching can't pass green here while diverging.
    fn run_headless_steps(
        calls: &[(Option<HeadlessReason>, bool)], // (classify result, committed) per call
        max_exec_calls: u32,
    ) -> HeadlessReason {
        let mut streak = 0u32;
        for (classified, committed) in calls.iter().take(max_exec_calls as usize) {
            match headless_step(streak, classified.clone(), *committed) {
                LoopStep::Terminal(r) => return r,
                LoopStep::Continue(s) => streak = s,
            }
        }
        HeadlessReason::MaxCalls
    }

    #[test]
    fn headless_step_passes_through_terminal_reason() {
        assert_eq!(
            headless_step(0, Some(HeadlessReason::Done), false),
            LoopStep::Terminal(HeadlessReason::Done)
        );
    }

    #[test]
    fn headless_step_commit_resets_streak() {
        assert_eq!(headless_step(1, None, true), LoopStep::Continue(0));
    }

    #[test]
    fn headless_step_second_no_commit_is_stuck() {
        assert_eq!(headless_step(0, None, false), LoopStep::Continue(1));
        assert_eq!(
            headless_step(1, None, false),
            LoopStep::Terminal(HeadlessReason::Stuck)
        );
    }

    #[test]
    fn stuck_fires_after_two_consecutive_no_commit_calls() {
        let calls = vec![
            (None, false), // call 1: streak = 1
            (None, false), // call 2: streak = 2 → Stuck
        ];
        assert_eq!(run_headless_steps(&calls, 6), HeadlessReason::Stuck);
    }

    #[test]
    fn commit_resets_no_commit_streak() {
        let calls = vec![
            (None, false), // streak = 1
            (None, true),  // committed → streak reset to 0
            (None, false), // streak = 1
            (None, false), // streak = 2 → Stuck
        ];
        assert_eq!(run_headless_steps(&calls, 6), HeadlessReason::Stuck);
    }

    #[test]
    fn loop_exhaustion_yields_maxcalls() {
        let calls: Vec<(Option<HeadlessReason>, bool)> = (0..6).map(|_| (None, true)).collect();
        assert_eq!(run_headless_steps(&calls, 6), HeadlessReason::MaxCalls);
    }

    #[test]
    fn maxcalls_outcome_is_stuck() {
        // End-to-end: loop exhaustion maps to Outcome::Stuck via headless_reason_to_outcome.
        let calls: Vec<(Option<HeadlessReason>, bool)> = (0..6).map(|_| (None, true)).collect();
        let reason = run_headless_steps(&calls, 6);
        assert_eq!(headless_reason_to_outcome(reason), Outcome::Stuck);
    }

    // ── token-usage capture (ADR-0008) ──────────────────────────────────────

    #[test]
    fn parse_plan_usage_skips_warning_preamble() {
        // The headless `-p --output-format stream-json` stdout is preceded by a
        // non-JSON warning line; the parser must skip it and read the terminal
        // result event's usage (reconciled exactly against the payload, D5).
        let stdout = "no stdin data received in 3s. Continuing without stdin.\n\
{\"type\":\"system\",\"subtype\":\"init\"}\n\
{\"type\":\"assistant\",\"message\":{\"id\":\"m1\"}}\n\
{\"type\":\"result\",\"modelUsage\":{\"claude-opus-4-8\":{\"input_tokens\":3747}},\"usage\":{\"input_tokens\":3747,\"output_tokens\":9,\"cache_read_input_tokens\":0,\"cache_creation_input_tokens\":23406}}\n";
        let usage = parse_plan_usage(stdout);
        assert_eq!(usage.input, 3747);
        assert_eq!(usage.output, 9);
        assert_eq!(usage.cache_read, 0);
        assert_eq!(usage.cache_creation, 23406);
        assert_eq!(usage.model.as_deref(), Some("claude-opus-4-8"));
    }

    #[test]
    fn parse_plan_usage_attributes_to_dominant_not_alphabetical_model() {
        // The real shape (captured from a plan.log): Claude bills a tiny amount to
        // the background `claude-haiku-4-5-20251001` and the bulk to the main
        // `claude-opus-4-8`. The top-level `usage` is the MAIN model's split, so the
        // phase must be labeled opus — not haiku (which sorts first alphabetically
        // and is a dated id absent from the price table).
        let stdout = "{\"type\":\"result\",\
\"modelUsage\":{\
\"claude-haiku-4-5-20251001\":{\"inputTokens\":4375,\"outputTokens\":17,\"cacheReadInputTokens\":0,\"cacheCreationInputTokens\":0},\
\"claude-opus-4-8\":{\"inputTokens\":4237,\"outputTokens\":14023,\"cacheReadInputTokens\":1129426,\"cacheCreationInputTokens\":76510}},\
\"usage\":{\"input_tokens\":4237,\"output_tokens\":14023,\"cache_read_input_tokens\":1129426,\"cache_creation_input_tokens\":76510}}\n";
        let usage = parse_plan_usage(stdout);
        assert_eq!(
            usage.model.as_deref(),
            Some("claude-opus-4-8"),
            "the dominant model, not the alphabetically-first background haiku"
        );
        // The numeric split is the main model's (the top-level `usage`), unchanged.
        assert_eq!(usage.input, 4237);
        assert_eq!(usage.output, 14023);
        assert_eq!(usage.cache_read, 1129426);
        assert_eq!(usage.cache_creation, 76510);
    }

    #[test]
    fn dashed_cwd_encodes_nonalnum_and_preserves_case() {
        assert_eq!(dashed_cwd("c:\\Dev\\ralphy"), "c--Dev-ralphy");
        assert_eq!(
            dashed_cwd("C:\\Dev\\.ralph-worktrees\\issue-10"),
            "C--Dev--ralph-worktrees-issue-10"
        );
    }

    #[test]
    fn parse_transcript_usage_dedups_message_id_and_ignores_iterations() {
        // Three assistant lines: two share `m1` (counted once), one carries `m2`
        // and nests an `iterations[]` that repeats its usage (must be ignored).
        let jsonl = "\
{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":100,\"output_tokens\":10,\"cache_read_input_tokens\":1000,\"cache_creation_input_tokens\":5}}}
{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":999,\"output_tokens\":999,\"cache_read_input_tokens\":999,\"cache_creation_input_tokens\":999}}}
{\"type\":\"assistant\",\"message\":{\"id\":\"m2\",\"usage\":{\"input_tokens\":200,\"output_tokens\":20,\"cache_read_input_tokens\":2000,\"cache_creation_input_tokens\":7}},\"iterations\":[{\"usage\":{\"input_tokens\":200,\"output_tokens\":20,\"cache_read_input_tokens\":2000,\"cache_creation_input_tokens\":7}}]}
";
        let usage = parse_transcript_usage(jsonl);
        // m1 (first only) + m2: input 100+200, output 10+20, cache_read 1000+2000,
        // cache_creation 5+7. The duplicate m1 line and the nested iterations are
        // both excluded.
        assert_eq!(
            usage,
            Usage {
                input: 300,
                output: 30,
                cache_read: 3000,
                cache_creation: 12,
                model: None,
            }
        );
    }

    #[test]
    fn parse_transcript_usage_attributes_dominant_model() {
        // Two models in one transcript: a little haiku auxiliary work and the
        // bulk on opus. The summed `usage.model` must resolve to the *dominant*
        // (most-tokens) model so the price table prices the run — without this
        // every execute row was written `unknown` and went unpriced (`~$?`).
        let jsonl = "\
{\"type\":\"assistant\",\"message\":{\"id\":\"h1\",\"model\":\"claude-haiku-4-5\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1,\"cache_read_input_tokens\":5,\"cache_creation_input_tokens\":0}}}
{\"type\":\"assistant\",\"message\":{\"id\":\"o1\",\"model\":\"claude-opus-4-8\",\"usage\":{\"input_tokens\":200,\"output_tokens\":20,\"cache_read_input_tokens\":2000,\"cache_creation_input_tokens\":7}}}
";
        let usage = parse_transcript_usage(jsonl);
        // Tokens still sum across both models...
        assert_eq!(usage.input, 210);
        assert_eq!(usage.cache_read, 2005);
        // ...but the model attribution picks opus (the dominant spend), not haiku.
        assert_eq!(usage.model.as_deref(), Some("claude-opus-4-8"));
    }

    #[test]
    fn fold_exec_usage_carries_heaviest_transcript_model() {
        // Two transcripts; the second is heavier. Tokens sum across both, and the
        // attribution follows the heaviest (opus) — not lost to `unknown`.
        let a = Usage {
            input: 10,
            output: 1,
            cache_read: 5,
            cache_creation: 0,
            model: Some("claude-haiku-4-5".into()),
        };
        let b = Usage {
            input: 200,
            output: 20,
            cache_read: 2000,
            cache_creation: 7,
            model: Some("claude-opus-4-8".into()),
        };
        let usage = fold_exec_usage(&[a, b], "sonnet");
        assert_eq!(usage.input, 210);
        assert_eq!(usage.cache_read, 2005);
        assert_eq!(usage.model.as_deref(), Some("claude-opus-4-8"));
    }

    #[test]
    fn fold_exec_usage_falls_back_to_requested_model_when_none_attributed() {
        // No transcript carried a model (counts present, attribution absent): the
        // phase falls back to the model we requested rather than `unknown`.
        let a = Usage {
            input: 100,
            output: 10,
            cache_read: 0,
            cache_creation: 0,
            model: None,
        };
        let usage = fold_exec_usage(&[a], "sonnet");
        assert_eq!(usage.input, 100);
        assert_eq!(usage.model.as_deref(), Some("sonnet"));
    }

    #[test]
    fn parse_transcript_usage_sums_cache_creation_subtiers() {
        // When only the `cache_creation` 5m/1h breakdown is present (no flat
        // field), the sub-tiers are summed.
        let jsonl = "{\"message\":{\"id\":\"x\",\"usage\":{\"input_tokens\":1,\"cache_creation\":{\"ephemeral_5m_input_tokens\":40,\"ephemeral_1h_input_tokens\":2}}}}\n";
        let usage = parse_transcript_usage(jsonl);
        assert_eq!(usage.input, 1);
        assert_eq!(usage.cache_creation, 42);
    }
}
