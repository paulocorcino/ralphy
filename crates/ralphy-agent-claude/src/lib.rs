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
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ralphy_core::{plan, Agent, Issue, Outcome, Plan, Workspace};
use ralphy_pty::{PtyCommand, PtySession, CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tracing::info;

/// The planning prompt, embedded so the binary is self-contained as a global
/// tool. Single source of truth lives at the repo root.
const PROMPT_PLAN: &str = include_str!("../../../prompt.plan.md");

/// The execution charter, embedded for the same reason and copied to
/// `.ralphy/exec.md` for the live session to read.
const PROMPT_EXECUTE: &str = include_str!("../../../prompt.execute.md");

/// The one-line charter the interactive session is launched with; it points the
/// agent at the embedded charter and the plan, and names the exit sentinel.
const EXEC_CHARTER: &str = "Read .ralphy/exec.md and follow it exactly to implement .ralphy/plan.md for this issue. Emit RALPHY_DONE_EXIT when finished.";

/// Minimal settings that keep a headless `claude -p` from hanging on a prompt.
/// The Stop hook is an execution concern, added by [`exec_settings_json`].
const SETTINGS_JSON: &str = r#"{"skipDangerousModePermissionPrompt":true,"skipAutoPermissionPrompt":true,"autoCompactEnabled":false}"#;

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
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            exec_model: None,
            exec_effort: Some("medium".into()),
            default_exec_model: "sonnet".into(),
            max_minutes_per_issue: 45,
            remote_control: true,
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
    ) -> Self {
        self.exec = ExecConfig {
            exec_model,
            exec_effort,
            default_exec_model,
            max_minutes_per_issue,
            remote_control,
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
        let json = exec_settings_json(&stop_hook_command(&exe));
        let path = self.run_dir.join("ralphy.settings.json");
        fs::write(&path, json).context("writing exec settings")?;
        Ok(path)
    }
}

/// Quote the Stop-hook command line for the platform: `"<exe>" hook stop`.
fn stop_hook_command(exe: &Path) -> String {
    format!("\"{}\" hook stop", exe.display())
}

/// Build the execution settings JSON: the headless skip flags plus a `Stop` hook
/// running `stop_command`. **No `PreToolUse` guard yet — that is #4.**
fn exec_settings_json(stop_command: &str) -> String {
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
            ]
        }
    });
    serde_json::to_string_pretty(&settings).expect("settings serialize")
}

impl Agent for ClaudeAgent {
    fn plan(&self, _issue: &Issue, ws: &Workspace) -> Result<Plan> {
        fs::create_dir_all(ws.ralphy_dir()).ok();
        fs::create_dir_all(&self.run_dir).ok();

        let plan_path = ws.plan_path();
        // Plan fresh every run; never reuse a stale artifact.
        let _ = fs::remove_file(&plan_path);

        let settings_path = self.run_dir.join("ralphy.settings.json");
        fs::write(&settings_path, SETTINGS_JSON).context("writing claude settings")?;

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

        info!(model = ?self.plan_model, effort = ?self.plan_effort, "planning with claude -p");
        let mut child = Command::new("claude")
            .args(&args)
            .current_dir(ws.repo_root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn the `claude` CLI (is it installed and on PATH?)")?;

        // Pipe the prompt on stdin; dropping the handle closes it so claude sees EOF.
        child
            .stdin
            .take()
            .expect("stdin was piped")
            .write_all(PROMPT_PLAN.as_bytes())
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
        fs::create_dir_all(&self.run_dir).ok();
        fs::create_dir_all(ws.ralphy_dir()).ok();

        // The live session reads the charter from disk (the headless copy keeps
        // the binary self-contained).
        fs::write(ws.ralphy_dir().join("exec.md"), PROMPT_EXECUTE)
            .context("writing .ralphy/exec.md")?;

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
        let mut cmd = PtyCommand::new("claude")
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
/// authoritative; otherwise a timeout is a [`Outcome::Timeout`], usage-limit text
/// in the transcript is a [`Outcome::Limit`], and a quiet exit is [`Outcome::Stuck`].
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
        return Outcome::Limit;
    }
    Outcome::Stuck
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
    use ralphy_core::Plan;
    use std::path::PathBuf;

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
    fn settings_have_stop_hook_and_no_pretooluse() {
        let json = exec_settings_json("\"ralphy.exe\" hook stop");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["skipDangerousModePermissionPrompt"], true);
        assert_eq!(v["skipAutoPermissionPrompt"], true);
        assert_eq!(v["autoCompactEnabled"], false);
        assert!(v["hooks"].get("PreToolUse").is_none(), "no guard yet (#4)");
        let cmd = &v["hooks"]["Stop"][0]["hooks"][0]["command"];
        assert_eq!(cmd, "\"ralphy.exe\" hook stop");
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["type"], "command");
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
                Some("You've reached your usage limit; resets 3pm")
            ),
            Outcome::Limit
        );
    }

    #[test]
    fn classify_stuck_when_quiet_exit() {
        assert_eq!(
            classify_outcome(None, false, Some("just a normal log")),
            Outcome::Stuck
        );
        assert_eq!(classify_outcome(None, false, None), Outcome::Stuck);
    }
}
