//! Claude run settings: the persisted [`ClaudeSettings`] schema (ADR-0010), the
//! headless skip-flag settings file, the execution-side [`ExecConfig`], the Stop/
//! guard/post hook wiring, and the plan's `## Execution model` judgment parser.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ralphy_core::Plan;

use crate::ClaudeAgent;

/// Minimal settings that keep a headless `claude -p` from hanging on a prompt.
/// The Stop hook is an execution concern, added by [`exec_settings_json`].
pub(crate) const SETTINGS_JSON: &str = r#"{"skipDangerousModePermissionPrompt":true,"skipAutoPermissionPrompt":true,"autoCompactEnabled":false}"#;

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
    /// Per-issue wall-clock budget in minutes (`--max-minutes-per-issue`): an
    /// opt-in productivity cap. `None` →
    /// [`ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE`], which is `0` = **no cap**
    /// by default. `0` — whether left unset or written explicitly — leaves the
    /// issue bounded only by `--deadline-hours`.
    ///
    /// A wedged child is not this knob's problem: that is the idle watchdog
    /// (`--idle-minutes`), which keys on progress instead of elapsed time
    /// (docs/adr/0038).
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
pub(crate) fn recommended_model(md: &str) -> Option<String> {
    let re =
        regex::Regex::new(r"(?im)^\s*##\s*Execution model:\s*(opus|sonnet)").expect("valid regex");
    re.captures(md).map(|c| c[1].to_lowercase())
}

/// The execution-side configuration, separate from the planning knobs.
pub(crate) struct ExecConfig {
    /// Forces the execution model for the issue when set (overrides the plan's
    /// judgment).
    pub(crate) exec_model: Option<String>,
    /// Reasoning effort for the execution session.
    pub(crate) exec_effort: Option<String>,
    /// Model used when neither an override nor a plan judgment is present.
    pub(crate) default_exec_model: String,
    /// Per-issue wall-clock budget before the session is reclaimed.
    pub(crate) max_minutes_per_issue: u64,
    /// Whether to enable Remote Control (follow/intervene from the mobile app).
    /// Opt-in (#148): `false` by default, resolved by the CLI's agnostic
    /// `remote_control` config key / `--remote-control` flag.
    pub(crate) remote_control: bool,
    /// When true, use a `claude -p` loop instead of an interactive PTY session.
    pub(crate) headless_exec: bool,
    /// Maximum number of `-p` calls before declaring MaxCalls (headless only).
    pub(crate) max_exec_calls: u32,
    /// The run's global wall-clock deadline, if any. Each issue's budget is
    /// clamped to `min(per-issue, run_deadline)` so an issue started near the
    /// global limit can't overrun it (mirrors `min(issueDeadline, $Deadline)`
    /// in ralphy.ps1:270).
    pub(crate) run_deadline: Option<std::time::Instant>,
    /// The operator's idle watchdog window in minutes, or `None` to let each
    /// execution path pick its own default.
    ///
    /// Deliberately an `Option` carried this far: the two paths have different
    /// progress signals and therefore different safe defaults (see
    /// [`ExecConfig::idle_minutes_for`]), but an operator who names a number
    /// means it for whichever path runs.
    pub(crate) idle_minutes: Option<u64>,
}

impl ExecConfig {
    /// The idle window to arm for the path about to run.
    ///
    /// An explicit operator value wins for both paths. Unset, headless gets the
    /// tighter default (any byte on either stream proves life) and interactive
    /// gets the looser one (only transcript growth does, and a long tool call
    /// legitimately produces none) — docs/adr/0038.
    pub(crate) fn idle_minutes_for(&self, interactive: bool) -> u64 {
        self.idle_minutes.unwrap_or(if interactive {
            ralphy_core::DEFAULT_INTERACTIVE_IDLE_MINUTES
        } else {
            ralphy_core::DEFAULT_IDLE_MINUTES
        })
    }
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            exec_model: None,
            exec_effort: Some("medium".into()),
            default_exec_model: "sonnet".into(),
            max_minutes_per_issue: ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE,
            remote_control: false,
            headless_exec: false,
            max_exec_calls: 6,
            run_deadline: None,
            idle_minutes: None,
        }
    }
}

impl ClaudeAgent {
    /// The single tier→model decision point: explicit override > the plan's
    /// `## Execution model` judgment > the configured default. Returns the
    /// literal model string `claude --model` expects (`sonnet`/`opus`).
    pub(crate) fn resolve_exec_model(&self, plan: &Plan) -> String {
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
    pub(crate) fn write_exec_settings(&self) -> Result<PathBuf> {
        let exe =
            std::env::current_exe().context("locating the ralphy binary for the Stop hook")?;
        let json = exec_settings_json(
            &stop_hook_command(&exe),
            &guard_hook_command(&exe),
            &post_hook_command(&exe),
        );
        let path = self.run_dir.join("ralphy.settings.json");
        std::fs::write(&path, json).context("writing exec settings")?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use ralphy_core::Usage;
    use std::path::PathBuf;

    fn plan_with(recommended: Option<&str>) -> Plan {
        Plan {
            path: PathBuf::from("/x/plan.md"),
            open_steps: 3,
            recommended_model: recommended.map(str::to_string),
            usage: Usage::default(),
            session_id: None,
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
    fn exec_config_default_remote_control_off() {
        assert!(!ExecConfig::default().remote_control);
    }

    #[test]
    fn reads_recommended_model() {
        assert_eq!(
            recommended_model("## Execution model: Opus\nbecause").as_deref(),
            Some("opus")
        );
        assert_eq!(recommended_model("no judgment here"), None);
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
}
