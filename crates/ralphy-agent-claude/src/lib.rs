//! The Claude Code adapter: drives `claude` behind the core [`Agent`] contract.
//! Everything Claude-specific — the binary, the model and effort flags, the
//! settings file, the PTY, completion detection — is confined here.
//!
//! `plan` runs headless `claude -p` (prompt piped on stdin). `execute` runs a
//! *live* interactive session over [`ralphy_pty`]: it lets `claude` commit onto
//! the run branch, detects completion from a flag file its Stop hook writes, and
//! reclaims the session on a per-issue wall timeout.

use std::fs;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ralphy_core::{git, plan, Agent, Issue, Outcome, Plan, Workspace};
use ralphy_pty::{PtyCommand, PtySession, CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tracing::info;

/// The planning prompt, embedded so the binary is self-contained as a global
/// tool. Single source of truth lives at the repo root.
const PROMPT_PLAN: &str = include_str!("../../../prompt.plan.md");

/// The staged-plan planning prompt, used when the issue carries the
/// `stagedplan` label.
const PROMPT_PLAN_STAGED: &str = include_str!("../../../prompt.plan.staged.md");

/// The execution charter, embedded for the same reason and copied to
/// `.ralphy/exec.md` for the live session to read.
const PROMPT_EXECUTE: &str = include_str!("../../../prompt.execute.md");

/// The one-line charter the interactive session is launched with; it points the
/// agent at the embedded charter and the plan, and names the exit sentinel.
const EXEC_CHARTER: &str = "Read .ralphy/exec.md and follow it exactly to implement .ralphy/plan.md for this issue. Emit RALPHY_DONE_EXIT when finished.";

/// Minimal settings that keep a headless `claude -p` from hanging on a prompt.
/// The Stop hook is an execution concern, added by [`exec_settings_json`].
const SETTINGS_JSON: &str = r#"{"skipDangerousModePermissionPrompt":true,"skipAutoPermissionPrompt":true,"autoCompactEnabled":false}"#;

/// Select the planning prompt for an issue. Returns `(prompt, staged)` where
/// `staged` is `true` when the issue carries the `stagedplan` label.
fn plan_prompt_for(issue: &Issue) -> (&'static str, bool) {
    if issue.labels.iter().any(|l| l == "stagedplan") {
        (PROMPT_PLAN_STAGED, true)
    } else {
        (PROMPT_PLAN, false)
    }
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
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            exec_model: None,
            exec_effort: Some("medium".into()),
            default_exec_model: "sonnet".into(),
            max_minutes_per_issue: 45,
            remote_control: true,
            headless_exec: false,
            max_exec_calls: 6,
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
        };
        self
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
        let json = exec_settings_json(&stop_hook_command(&exe), &guard_hook_command(&exe));
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

/// Build the execution settings JSON: the headless skip flags, a `Stop` hook
/// running `stop_command`, and a `PreToolUse` guard running `guard_command`.
fn exec_settings_json(stop_command: &str, guard_command: &str) -> String {
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
        model: &str,
        timeout: Duration,
        call_index: u32,
    ) -> Result<(bool, String)> {
        let mut args: Vec<String> = vec![
            "-p".into(),
            "--dangerously-skip-permissions".into(),
            "--settings".into(),
            settings.to_string_lossy().into_owned(),
            "--model".into(),
            model.into(),
        ];
        if let Some(e) = &self.exec.exec_effort {
            args.push("--effort".into());
            args.push(e.clone());
        }

        let mut child = Command::new(resolve_claude_binary())
            .args(&args)
            .current_dir(cmd_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn the `claude` CLI for headless exec")?;

        child
            .stdin
            .take()
            .expect("stdin was piped")
            .write_all(PROMPT_EXECUTE.as_bytes())
            .context("piping exec prompt to claude")?;

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

        let deadline = Instant::now() + timeout;
        let exited = loop {
            if child.try_wait().context("polling claude child")?.is_some() {
                break true;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
            thread::sleep(Duration::from_millis(500));
        };

        let collect = Duration::from_secs(5);
        let stdout_bytes = rx_out.recv_timeout(collect).unwrap_or_default();
        let stderr_bytes = rx_err.recv_timeout(collect).unwrap_or_default();
        let mut text = String::from_utf8_lossy(&stdout_bytes).into_owned();
        text.push_str(&String::from_utf8_lossy(&stderr_bytes));
        let _ = fs::write(self.run_dir.join(format!("exec-{}.out", call_index)), &text);
        Ok((exited, text))
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
        let exec_model = self.resolve_exec_model(plan);
        let deadline = Instant::now() + Duration::from_secs(self.exec.max_minutes_per_issue * 60);

        info!(
            model = %exec_model,
            effort = ?self.exec.exec_effort,
            max_calls = self.exec.max_exec_calls,
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
            let (exited, out) =
                self.run_headless_call(ws.repo_root(), &settings_path, &exec_model, remaining, i)?;

            let plan_md = fs::read_to_string(&plan.path).unwrap_or_default();
            let open_steps = plan::count_open_steps(&plan_md);
            let after_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
            let committed = before_sha != after_sha;

            if let Some(reason) = classify_exec_call(&out, exited, open_steps, committed) {
                info!(call = i, "headless call terminal");
                return Ok(headless_reason_to_outcome(reason));
            }

            if committed {
                no_commit_streak = 0;
            } else {
                no_commit_streak += 1;
                info!(
                    call = i,
                    streak = no_commit_streak,
                    "no commit this headless call"
                );
                if no_commit_streak >= 2 {
                    return Ok(headless_reason_to_outcome(HeadlessReason::Stuck));
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

impl Agent for ClaudeAgent {
    fn plan(&self, issue: &Issue, ws: &Workspace) -> Result<Plan> {
        fs::create_dir_all(ws.ralphy_dir()).ok();
        fs::create_dir_all(&self.run_dir).ok();

        let plan_path = ws.plan_path();
        // Plan fresh every run; never reuse a stale artifact.
        let _ = fs::remove_file(&plan_path);

        let settings_path = self.run_dir.join("ralphy.settings.json");
        fs::write(&settings_path, SETTINGS_JSON).context("writing claude settings")?;

        let (prompt, staged) = plan_prompt_for(issue);

        // `--model` first (as the ps1 oracle does), then the headless flags.
        let mut args: Vec<String> = Vec::new();
        if let Some(m) = &self.plan_model {
            args.push("--model".into());
            args.push(m.clone());
        }
        args.push("-p".into());
        args.push("--dangerously-skip-permissions".into());
        args.push("--settings".into());
        args.push(settings_path.to_string_lossy().into_owned());
        if let Some(e) = &self.plan_effort {
            args.push("--effort".into());
            args.push(e.clone());
        }

        info!(model = ?self.plan_model, effort = ?self.plan_effort, staged, "planning with claude -p");
        let mut cmd = Command::new(resolve_claude_binary());
        cmd.args(&args)
            .current_dir(ws.repo_root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if staged {
            cmd.env("STAGED_PLAN_NONINTERACTIVE", "1");
        }
        let mut child = cmd
            .spawn()
            .context("failed to spawn the `claude` CLI (is it installed and on PATH?)")?;

        // Pipe the prompt on stdin; dropping the handle closes it so claude sees EOF.
        child
            .stdin
            .take()
            .expect("stdin was piped")
            .write_all(prompt.as_bytes())
            .context("piping the plan prompt to claude")?;

        let out = child.wait_with_output().context("waiting for claude")?;
        let mut log = String::from_utf8_lossy(&out.stdout).into_owned();
        log.push_str(&String::from_utf8_lossy(&out.stderr));
        let _ = fs::write(self.run_dir.join("plan.log"), &log);

        if !plan_path.exists() {
            bail!(
                "claude produced no plan at {} (see {})",
                plan_path.display(),
                self.run_dir.join("plan.log").display()
            );
        }
        let md = fs::read_to_string(&plan_path).context("reading the written plan.md")?;
        Ok(Plan {
            open_steps: plan::count_open_steps(&md),
            recommended_model: plan::recommended_model(&md),
            path: plan_path,
        })
    }

    fn execute(&self, plan: &Plan, ws: &Workspace) -> Result<Outcome> {
        if self.exec.headless_exec {
            return self.execute_headless(plan, ws);
        }

        fs::create_dir_all(&self.run_dir).ok();
        fs::create_dir_all(ws.ralphy_dir()).ok();

        // The live session reads the charter from disk (the headless copy keeps
        // the binary self-contained).
        fs::write(ws.ralphy_dir().join("exec.md"), PROMPT_EXECUTE)
            .context("writing .ralphy/exec.md")?;

        // Grant the one-time workspace trust so the interactive session doesn't
        // stall on Claude's first-run "do you trust this folder?" dialog.
        ensure_workspace_trusted(ws.repo_root());

        let settings_path = self.write_exec_settings()?;
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
            .arg(settings_path.as_os_str());
        cmd = cmd.arg("--model").arg(&exec_model);
        if let Some(e) = &self.exec.exec_effort {
            cmd = cmd.arg("--effort").arg(e);
        }
        if self.exec.remote_control {
            cmd = cmd.arg("--remote-control").arg(&rc_name);
        }
        cmd = cmd.arg(EXEC_CHARTER);

        info!(model = %exec_model, effort = ?self.exec.exec_effort, remote_control = self.exec.remote_control, "executing with interactive claude over the PTY");

        let mut session =
            PtySession::spawn(cmd).context("spawning the claude execution session")?;
        let outcome = self.drive_session(&mut session, &flag_file)?;
        // Reclaim: kill the tree and drop the session (closes the ConPTY).
        let _ = session.kill();
        Ok(outcome)
    }
}

impl ClaudeAgent {
    /// Drain the PTY (tee to `exec.log`, answer DSR queries) while polling for the
    /// flag file, the child's own exit, and the per-issue wall timeout. Classifies
    /// the result into an [`Outcome`].
    fn drive_session(&self, session: &mut PtySession, flag_file: &Path) -> Result<Outcome> {
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
        let deadline = Instant::now() + Duration::from_secs(self.exec.max_minutes_per_issue * 60);

        let mut timed_out = false;
        let mut child_exited = false;
        loop {
            // Act as the terminal: tee output and answer cursor-position queries.
            while let Ok(chunk) = rx.try_recv() {
                if find_subslice(&chunk, CURSOR_POSITION_REQUEST).is_some() {
                    let _ = session.write_all(CURSOR_POSITION_REPLY);
                }
                if let Some(f) = log.as_mut() {
                    let _ = f.write_all(&chunk);
                }
            }

            if flag_file.exists() {
                break;
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
        // A transcript read is only needed to spot a usage limit when the session
        // ended without a sentinel (it exits on its own when limited).
        let transcript = if flag.is_none() && (child_exited || timed_out) {
            latest_transcript_text()
        } else {
            None
        };

        let outcome = classify_outcome(flag.as_deref(), timed_out, transcript.as_deref());
        info!(?outcome, child_exited, timed_out, "execution session ended");
        Ok(outcome)
    }
}

/// Map the session's end state to an [`Outcome`]. The flag the Stop hook wrote is
/// authoritative; otherwise a timeout is [`Outcome::Timeout`], usage-limit text
/// in the transcript is [`Outcome::Limit`] (with a parsed reset hint), and a
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
    if timed_out {
        return Outcome::Timeout;
    }
    if transcript.is_some_and(is_limit_text) {
        return Outcome::Limit(transcript.and_then(parse_reset_hhmm));
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
/// Priority order mirrors `Invoke-ExecLoop`:
/// 1. Process did not exit (timed out): limit text → `Limit`, else → `Timeout`.
/// 2. `RALPHY_BLOCKED_EXIT` in output → `Blocked`.
/// 3. `RALPHY_DONE_EXIT` or zero open steps → `Done`.
/// 4. Limit text on natural exit → `Limit`.
/// 5. Otherwise → `None` (continue).
fn classify_exec_call(
    out: &str,
    exited: bool,
    open_steps: usize,
    _committed: bool,
) -> Option<HeadlessReason> {
    if !exited {
        if is_limit_text(out) {
            return Some(HeadlessReason::Limit(parse_reset_hhmm(out)));
        }
        return Some(HeadlessReason::Timeout);
    }
    if out.contains("RALPHY_BLOCKED_EXIT") {
        let reason = out
            .lines()
            .find(|l| l.contains("RALPHY_BLOCKED_EXIT"))
            .and_then(|l| l.split("RALPHY_BLOCKED_EXIT").nth(1))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        return Some(HeadlessReason::Blocked(reason));
    }
    if out.contains("RALPHY_DONE_EXIT") || open_steps == 0 {
        return Some(HeadlessReason::Done);
    }
    if is_limit_text(out) {
        return Some(HeadlessReason::Limit(parse_reset_hhmm(out)));
    }
    None
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
/// `Test-LimitText` oracle.
fn is_limit_text(text: &str) -> bool {
    use regex::Regex;
    let re =
        Regex::new(r"(?i)(rate limit|usage limit|reached your .* limit|limit reached|resets\s+\d)")
            .expect("valid regex");
    re.is_match(text)
}

/// Parse a reset time from a usage-limit transcript. Looks for a pattern like
/// "resets 3:00pm" or "resets Tue 12:30am" and converts it to 24h `HH:mm`.
/// Returns `None` when no match is found. Ports `Get-ResetDateTime`.
fn parse_reset_hhmm(text: &str) -> Option<String> {
    use regex::Regex;
    let re = Regex::new(r"(?i)resets\s+(?:[a-z]{3}\s+)?(\d{1,2}):(\d{2})\s*([ap]m)")
        .expect("valid regex");
    let caps = re.captures(text)?;
    let hour: u32 = caps[1].parse().ok()?;
    let min: u32 = caps[2].parse().ok()?;
    let ampm = caps[3].to_lowercase();
    let hour24 = match ampm.as_str() {
        "am" => hour % 12,
        _ => (hour % 12) + 12,
    };
    Some(format!("{:02}:{:02}", hour24, min))
}

/// The most recent `claude` transcript JSONL under `~/.claude/projects`, read in
/// full, if it was touched in the last 5 minutes. Ports `Get-LatestTranscript`.
fn latest_transcript_text() -> Option<String> {
    let base = dirs_home()?.join(".claude").join("projects");
    let newest = newest_jsonl(&base)?;
    fs::read_to_string(newest).ok()
}

/// The home directory, from the platform's usual env var.
fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// Pre-accept Claude Code's workspace-trust dialog for `repo_root`. The headless
/// `-p` planning path is exempt from that dialog, but an *interactive* session
/// stalls on it forever waiting for a keypress — so an autonomous orchestrator
/// must grant the one-time trust the operator would otherwise click. Best-effort:
/// reads `~/.claude.json`, sets the flag for this workspace, and writes it back,
/// preserving everything else. A failure here just means the live session may
/// stall (surfaced as a Timeout), never a crash.
fn ensure_workspace_trusted(repo_root: &Path) {
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
    let updated = with_workspace_trusted(root, &key);
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

/// Resolve the `claude` executable to an absolute path, mirroring the ps1
/// oracle's `$Claude` resolution. This matters because the PTY backend rebuilds
/// `PATH` from the Windows registry and ignores runtime `PATH` edits, so a bare
/// `"claude"` fails wherever the install dir isn't on the *persistent* PATH.
/// Falls back to `~/.local/bin/claude[.exe]`, then to the bare name so the spawn
/// error still names it.
fn resolve_claude_binary() -> std::ffi::OsString {
    if let Some(found) = find_program(
        "claude",
        std::env::var_os("PATH"),
        std::env::var_os("PATHEXT"),
    ) {
        return found.into_os_string();
    }
    if let Some(home) = dirs_home() {
        let mut cand = home.join(".local").join("bin").join("claude");
        if cfg!(windows) {
            cand.set_extension("exe");
        }
        if cand.is_file() {
            return cand.into_os_string();
        }
    }
    "claude".into()
}

/// Search `path_var` for `name`, trying each `PATHEXT` extension on Windows. Pure
/// over its inputs so it unit-tests against a temp dir.
fn find_program(
    name: &str,
    path_var: Option<std::ffi::OsString>,
    pathext: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let path_var = path_var?;
    let exts: Vec<String> = if cfg!(windows) {
        pathext
            .and_then(|p| p.into_string().ok())
            .unwrap_or_else(|| ".EXE".into())
            .split(';')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        Vec::new()
    };
    for dir in std::env::split_paths(&path_var) {
        let direct = dir.join(name);
        if direct.is_file() {
            return Some(direct);
        }
        for ext in &exts {
            let cand = dir.join(name).with_extension(ext.trim_start_matches('.'));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// Recursively find the most-recently-modified `*.jsonl` under `base`, but only if
/// it was modified within the last 5 minutes (a stale transcript is irrelevant).
fn newest_jsonl(base: &Path) -> Option<PathBuf> {
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

/// Index of the first occurrence of `needle` in `haystack`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralphy_core::{Issue, Plan};
    use std::path::PathBuf;

    fn issue_with_labels(labels: &[&str]) -> Issue {
        Issue {
            number: 1,
            title: "test".into(),
            body: String::new(),
            labels: labels.iter().map(|s| s.to_string()).collect(),
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

    fn plan_with(recommended: Option<&str>) -> Plan {
        Plan {
            path: PathBuf::from("/x/plan.md"),
            open_steps: 3,
            recommended_model: recommended.map(str::to_string),
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
    fn settings_have_stop_hook_and_pretooluse_guard() {
        let json = exec_settings_json("\"ralphy.exe\" hook stop", "\"ralphy.exe\" hook guard");
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

    #[test]
    fn classify_timeout_beats_transcript() {
        assert_eq!(
            classify_outcome(None, true, Some("usage limit reached")),
            Outcome::Timeout
        );
    }

    #[test]
    fn classify_limit_from_transcript() {
        assert_eq!(
            classify_outcome(
                None,
                false,
                Some("You've reached your usage limit; resets 3:00pm")
            ),
            Outcome::Limit(Some("15:00".into()))
        );
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
    fn parse_reset_hhmm_no_match() {
        assert_eq!(parse_reset_hhmm("no time here"), None);
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
    fn find_program_locates_a_file_on_the_search_path() {
        let dir = std::env::temp_dir().join(format!("ralphy-find-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = if cfg!(windows) { "tool.exe" } else { "tool" };
        std::fs::write(dir.join(exe), b"x").unwrap();

        let path_var = std::ffi::OsString::from(dir.to_string_lossy().into_owned());
        let got = find_program("tool", Some(path_var), Some(".EXE".into()));
        // Resolves to the real file (the extension casing follows PATHEXT, so
        // compare by existence + parent rather than an exact path string).
        let got = got.expect("tool should be found on the search path");
        assert!(got.is_file());
        assert_eq!(got.parent(), Some(dir.as_path()));
        assert_eq!(got.file_stem().and_then(|s| s.to_str()), Some("tool"));

        // A name that isn't there resolves to nothing.
        let path_var = std::ffi::OsString::from(dir.to_string_lossy().into_owned());
        assert!(find_program("nope", Some(path_var), Some(".EXE".into())).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn classify_stuck_when_quiet_exit() {
        assert_eq!(
            classify_outcome(None, false, Some("just a normal log")),
            Outcome::Stuck
        );
        assert_eq!(classify_outcome(None, false, None), Outcome::Stuck);
    }

    // ── classify_exec_call ──────────────────────────────────────────────────

    #[test]
    fn classify_exec_not_exited_with_limit_text_is_limit() {
        let out = "You've reached your usage limit; resets 3:00pm";
        assert_eq!(
            classify_exec_call(out, false, 5, false),
            Some(HeadlessReason::Limit(Some("15:00".into())))
        );
    }

    #[test]
    fn classify_exec_not_exited_without_limit_is_timeout() {
        assert_eq!(
            classify_exec_call("partial output", false, 5, false),
            Some(HeadlessReason::Timeout)
        );
    }

    #[test]
    fn classify_exec_blocked_sentinel() {
        let out = "some work\nRALPHY_BLOCKED_EXIT missing key\nmore text";
        assert_eq!(
            classify_exec_call(out, true, 5, false),
            Some(HeadlessReason::Blocked("missing key".into()))
        );
    }

    #[test]
    fn classify_exec_done_via_done_sentinel() {
        let out = "all done\nRALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_exec_call(out, true, 3, true),
            Some(HeadlessReason::Done)
        );
    }

    #[test]
    fn classify_exec_done_via_zero_open_steps() {
        assert_eq!(
            classify_exec_call("no sentinel", true, 0, true),
            Some(HeadlessReason::Done)
        );
    }

    #[test]
    fn classify_exec_limit_on_natural_exit() {
        let out = "You've reached your usage limit; resets 3:00pm";
        assert_eq!(
            classify_exec_call(out, true, 2, false),
            Some(HeadlessReason::Limit(Some("15:00".into())))
        );
    }

    #[test]
    fn classify_exec_continue_when_no_terminal_condition() {
        assert_eq!(
            classify_exec_call("partial progress, no sentinel", true, 3, true),
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

    /// Pure simulation of execute_headless's decision loop, used to verify the
    /// stuck-counter and MaxCalls logic without spawning real processes.
    fn simulate_headless_loop(
        calls: &[(Option<HeadlessReason>, bool)], // (classify result, committed) per call
        max_exec_calls: u32,
    ) -> HeadlessReason {
        let mut no_commit_streak = 0u32;
        for (reason_opt, committed) in calls.iter().take(max_exec_calls as usize) {
            if let Some(r) = reason_opt.clone() {
                return r;
            }
            if *committed {
                no_commit_streak = 0;
            } else {
                no_commit_streak += 1;
                if no_commit_streak >= 2 {
                    return HeadlessReason::Stuck;
                }
            }
        }
        HeadlessReason::MaxCalls
    }

    #[test]
    fn stuck_fires_after_two_consecutive_no_commit_calls() {
        let calls = vec![
            (None, false), // call 1: streak = 1
            (None, false), // call 2: streak = 2 → Stuck
        ];
        assert_eq!(simulate_headless_loop(&calls, 6), HeadlessReason::Stuck);
    }

    #[test]
    fn commit_resets_no_commit_streak() {
        let calls = vec![
            (None, false), // streak = 1
            (None, true),  // committed → streak reset to 0
            (None, false), // streak = 1
            (None, false), // streak = 2 → Stuck
        ];
        assert_eq!(simulate_headless_loop(&calls, 6), HeadlessReason::Stuck);
    }

    #[test]
    fn loop_exhaustion_yields_maxcalls() {
        let calls: Vec<(Option<HeadlessReason>, bool)> = (0..6).map(|_| (None, true)).collect();
        assert_eq!(simulate_headless_loop(&calls, 6), HeadlessReason::MaxCalls);
    }

    #[test]
    fn maxcalls_outcome_is_stuck() {
        // End-to-end: loop exhaustion maps to Outcome::Stuck via headless_reason_to_outcome.
        let calls: Vec<(Option<HeadlessReason>, bool)> = (0..6).map(|_| (None, true)).collect();
        let reason = simulate_headless_loop(&calls, 6);
        assert_eq!(headless_reason_to_outcome(reason), Outcome::Stuck);
    }
}
