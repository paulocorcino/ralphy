//! Claude run settings: the persisted [`ClaudeSettings`] schema (ADR-0010), the
//! headless skip-flag settings file, the execution-side [`ExecConfig`], the Stop/
//! guard/post hook wiring, and the plan's `## Execution model` judgment parser.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ralphy_core::Plan;

use crate::ClaudeAgent;

/// The settings every headless session shares: the flags that keep `claude -p`
/// from hanging on a prompt, and the guard's `PreToolUse` hook. Every session
/// Ralphy starts carries the guard (ADR-0072 D6); the phase builders add their
/// own hooks on top.
fn base_settings(guard_command: &str) -> serde_json::Value {
    serde_json::json!({
        "skipDangerousModePermissionPrompt": true,
        "skipAutoPermissionPrompt": true,
        "autoCompactEnabled": false,
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "Bash|Edit|Write|MultiEdit|NotebookEdit",
                    "hooks": [ { "type": "command", "command": guard_command } ]
                }
            ]
        }
    })
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
    /// Opt-in: start a workbench-opened Claude console with its console name as
    /// its session name (`claude --name wb-<folded console name>`, such as
    /// `wb-fincal-1`), so the address other sessions use is the name the
    /// operator reads in the title. Left off, the CLI names the session itself
    /// — `<folder>-<2 hex>`.
    ///
    /// A plain `bool`, not an `Option`, unlike every field above: those carry a
    /// tri-state (unset → a hardcoded run default that is not `false`), while
    /// absent here means exactly what `false` means — no flag. Mirrors
    /// `CursorSettings`' opt-in, and an untouched section still serializes no key.
    ///
    /// Read by the daemon's launch path, which cannot import this crate
    /// (ADR-0032 §10) and so reparses the file; `crates/ralphy-daemon/src/session.rs`
    /// pins the section and key against this schema.
    #[serde(default, skip_serializing_if = "is_false")]
    pub console_name: bool,
}

/// `skip_serializing_if` for a `false` flag: an un-opted repo writes no key, so
/// an existing `settings.json` is unchanged by this field's arrival.
fn is_false(b: &bool) -> bool {
    !*b
}

impl ClaudeSettings {
    /// The settings-file section this struct lives under.
    pub const SECTION: &'static str = "claude";
}

/// The planner's `## Execution model: sonnet|opus|opus-high` judgment, lowercased,
/// if any. Claude-vocabulary parsing lives here, not in core (ADR-0002 amendment,
/// #79): core's `Plan.recommended_model` is an opaque token it only carries across.
/// `opus-high` is the effort-bearing rung (ADR-0002, Amendment 2026-07-24) — opus
/// at high reasoning effort. `opus-high|opus` order matters: alternation is
/// leftmost-first, so `opus` must not shadow `opus-high`.
pub(crate) fn recommended_model(md: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?im)^\s*##\s*Execution model:\s*(opus-high|opus|sonnet)")
        .expect("valid regex");
    re.captures(md).map(|c| c[1].to_lowercase())
}

/// Normalize a plan judgment token to the literal model `claude --model` expects:
/// the effort-bearing `opus-high` rung selects the `opus` model (its effort is
/// carried separately by [`ClaudeAgent::resolve_exec_effort`]). Every other token
/// passes through unchanged.
fn model_of_judgment(token: &str) -> &str {
    match token {
        "opus-high" => "opus",
        other => other,
    }
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
    /// literal model string `claude --model` expects (`sonnet`/`opus`); the
    /// `opus-high` rung normalizes to `opus` (its effort rides
    /// [`Self::resolve_exec_effort`], ADR-0002 Amendment 2026-07-24).
    pub(crate) fn resolve_exec_model(&self, plan: &Plan) -> String {
        if let Some(m) = &self.exec.exec_model {
            return m.clone();
        }
        if let Some(m) = &plan.recommended_model {
            return model_of_judgment(m).to_string();
        }
        self.exec.default_exec_model.clone()
    }

    /// The single execution-effort decision point, mirroring [`Self::resolve_exec_model`]:
    /// the operator's `--exec-effort` (or persisted `claude.exec_effort`) wins on
    /// every issue; otherwise the plan's `opus-high` judgment couples to `high`;
    /// otherwise absent, leaving Claude on its own default (ADR-0002, Amendment
    /// 2026-07-24). `None` omits `--effort` from the argv entirely.
    pub(crate) fn resolve_exec_effort(&self, plan: &Plan) -> Option<String> {
        if let Some(effort) = &self.exec.exec_effort {
            return Some(effort.clone());
        }
        if plan.recommended_model.as_deref() == Some("opus-high") {
            return Some("high".to_string());
        }
        None
    }

    /// Write `ralphy.settings.json` with the skip flags and a Stop hook that
    /// invokes *this* binary's `hook stop`. Returns the settings path.
    pub(crate) fn write_exec_settings(&self) -> Result<PathBuf> {
        let exe =
            std::env::current_exe().context("locating the ralphy binary for the Stop hook")?;
        let json = exec_settings_json(
            &stop_hook_command(&exe),
            &guard_hook_command(&exe, true),
            &post_hook_command(&exe),
            &status_hook_command(&exe),
        );
        let path = self.run_dir.join("ralphy.settings.json");
        std::fs::write(&path, json).context("writing exec settings")?;
        Ok(path)
    }

    /// Write the plan phase's `ralphy.settings.json`: the skip flags, the
    /// guard without the cost gate, and the agent-state hooks — no Stop
    /// sentinel hook (ADR-0059 §4).
    pub(crate) fn write_plan_settings(&self) -> Result<PathBuf> {
        let exe = std::env::current_exe()
            .context("locating the ralphy binary for the agent-state hook")?;
        let json = plan_settings_json(&guard_hook_command(&exe, false), &status_hook_command(&exe));
        let path = self.run_dir.join("ralphy.settings.json");
        std::fs::write(&path, json).context("writing plan settings")?;
        Ok(path)
    }
}

/// Quote the Stop-hook command line for the platform: `"<exe>" hook stop`.
fn stop_hook_command(exe: &Path) -> String {
    format!("\"{}\" hook stop", exe.display())
}

/// Quote the guard-hook command line for the platform: `"<exe>" hook guard`,
/// with `--cost-gate` for an execute session — the gate reads the run's
/// `plan.md`, which only execute owns (ADR-0072 D6).
fn guard_hook_command(exe: &Path, cost_gate: bool) -> String {
    let flag = if cost_gate { " --cost-gate" } else { "" };
    format!("\"{}\" hook guard{flag}", exe.display())
}

/// The settings of a one-shot task session (consolidate, diagnose, draft,
/// triage): the skip flags and the guard.
fn task_settings_json(guard_command: &str) -> String {
    serde_json::to_string_pretty(&base_settings(guard_command)).expect("settings serialize")
}

/// Write a task session's `ralphy.settings.json` into `dir` (created when
/// missing). Returns the settings path.
pub(crate) fn write_task_settings(dir: &Path) -> Result<PathBuf> {
    let exe = std::env::current_exe().context("locating the ralphy binary for the guard hook")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("ralphy.settings.json");
    std::fs::write(&path, task_settings_json(&guard_hook_command(&exe, false)))
        .context("writing claude settings")?;
    Ok(path)
}

/// Quote the post-hook command line for the platform: `"<exe>" hook post`.
fn post_hook_command(exe: &Path) -> String {
    format!("\"{}\" hook post", exe.display())
}

/// Quote the agent-state hook command line: `"<exe>" hook status`.
fn status_hook_command(exe: &Path) -> String {
    format!("\"{}\" hook status", exe.display())
}

/// The agent-state hook set (ADR-0059 §4 as amended 2026-09-16), as `hooks`
/// entries keyed by event: `SessionStart`, `UserPromptSubmit`, `PreToolUse`
/// and `PostToolUse` on every tool, `PermissionRequest` on every tool,
/// `Stop`, `SubagentStop` — two spawns per tool call, one per prompt, one
/// per turn end. `PostToolUse` is the one event that ENDS a `waiting`: the
/// question answered, the permission granted. Deliberately absent:
/// `Notification`, `PreCompact`, `PostToolUseFailure`, `SubagentStart` —
/// each would add a spawn and no state.
fn status_hook_entries(status_command: &str) -> Vec<(&'static str, serde_json::Value)> {
    let entry = |matcher: &str| {
        serde_json::json!({
            "matcher": matcher,
            "hooks": [ { "type": "command", "command": status_command } ]
        })
    };
    vec![
        ("SessionStart", entry("")),
        ("UserPromptSubmit", entry("")),
        ("PreToolUse", entry("*")),
        ("PostToolUse", entry("*")),
        ("PermissionRequest", entry("*")),
        ("Stop", entry("")),
        ("SubagentStop", entry("")),
    ]
}

/// Append the status entries to a `hooks` object, AFTER whatever an event
/// already holds: the guard's narrow `PreToolUse` and the sentinel `Stop`
/// keep their first slot and their own command; the two are independent
/// hooks on the same event.
fn with_status_hooks(hooks: &mut serde_json::Map<String, serde_json::Value>, status_command: &str) {
    for (event, entry) in status_hook_entries(status_command) {
        let list = hooks
            .entry(event)
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        if let serde_json::Value::Array(items) = list {
            items.push(entry);
        }
    }
}

/// The plan phase's settings: the skip flags, the guard, and the agent-state
/// hooks.
fn plan_settings_json(guard_command: &str, status_command: &str) -> String {
    let mut settings = base_settings(guard_command);
    if let Some(hooks) = settings["hooks"].as_object_mut() {
        with_status_hooks(hooks, status_command);
    }
    serde_json::to_string_pretty(&settings).expect("settings serialize")
}

/// Build the execution settings JSON: the headless skip flags, a `Stop` hook
/// running `stop_command`, a `PreToolUse` guard running `guard_command`, and a
/// `PostToolUse` Bash timer running `post_command` (the other half of the
/// verification-cost gate: the guard stamps a verify command's start, this hook
/// records its measured duration).
fn exec_settings_json(
    stop_command: &str,
    guard_command: &str,
    post_command: &str,
    status_command: &str,
) -> String {
    let mut settings = base_settings(guard_command);
    settings["hooks"]["Stop"] = serde_json::json!([
        {
            "matcher": "",
            "hooks": [ { "type": "command", "command": stop_command } ]
        }
    ]);
    settings["hooks"]["PostToolUse"] = serde_json::json!([
        {
            "matcher": "Bash",
            "hooks": [ { "type": "command", "command": post_command } ]
        }
    ]);
    if let Some(hooks) = settings["hooks"].as_object_mut() {
        with_status_hooks(hooks, status_command);
    }
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
        // The effort-bearing rung must parse whole, not be shadowed by `opus`.
        assert_eq!(
            recommended_model("## Execution model: opus-high\nbecause").as_deref(),
            Some("opus-high")
        );
        assert_eq!(
            recommended_model("## Execution model: Opus-High\n").as_deref(),
            Some("opus-high")
        );
    }

    #[test]
    fn opus_high_judgment_selects_the_opus_model() {
        // The rung carries effort separately; the `--model` argv is plain `opus`.
        let agent = agent_with(None, "sonnet");
        assert_eq!(
            agent.resolve_exec_model(&plan_with(Some("opus-high"))),
            "opus"
        );
    }

    #[test]
    fn opus_high_judgment_couples_effort_to_high_when_operator_is_silent() {
        // No `--exec-effort`: the plan's opus-high rung raises effort to high.
        let silent = ClaudeAgent::new(None, None, PathBuf::from("/run")).with_exec_config(
            None,
            None, // operator did not name an exec effort
            "sonnet".into(),
            45,
            true,
            false,
            6,
        );
        assert_eq!(
            silent
                .resolve_exec_effort(&plan_with(Some("opus-high")))
                .as_deref(),
            Some("high")
        );
        // A plain opus (or sonnet) rung leaves effort absent — Claude's own default.
        assert_eq!(silent.resolve_exec_effort(&plan_with(Some("opus"))), None);
        assert_eq!(silent.resolve_exec_effort(&plan_with(None)), None);
    }

    #[test]
    fn operator_exec_effort_wins_over_the_opus_high_rung() {
        // An explicit `--exec-effort medium` beats the plan's opus-high default.
        let forced = agent_with(None, "sonnet"); // agent_with sets exec_effort = medium
        assert_eq!(
            forced
                .resolve_exec_effort(&plan_with(Some("opus-high")))
                .as_deref(),
            Some("medium")
        );
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

    /// ADR-0059 §4: the status hooks ride both phases; the guard and the
    /// sentinel Stop keep their FIRST slot and their own command on execute;
    /// the plan phase has the status hooks and nothing else; and the events
    /// the ADR rejected are registered on neither.
    #[test]
    fn status_hooks_ride_both_phases_after_the_guard_and_the_sentinel() {
        let status = "\"ralphy.exe\" hook status";
        let exec: serde_json::Value = serde_json::from_str(&exec_settings_json(
            "\"ralphy.exe\" hook stop",
            "\"ralphy.exe\" hook guard --cost-gate",
            "\"ralphy.exe\" hook post",
            status,
        ))
        .unwrap();
        let plan: serde_json::Value =
            serde_json::from_str(&plan_settings_json("\"ralphy.exe\" hook guard", status)).unwrap();
        for (doc, name) in [(&exec, "exec"), (&plan, "plan")] {
            let hooks = &doc["hooks"];
            for event in [
                "SessionStart",
                "UserPromptSubmit",
                "PermissionRequest",
                "SubagentStop",
            ] {
                let list = hooks[event]
                    .as_array()
                    .unwrap_or_else(|| panic!("{name}: {event}"));
                assert_eq!(list.len(), 1, "{name}: one status hook on {event}");
                assert_eq!(list[0]["hooks"][0]["command"], status, "{name}: {event}");
            }
            assert_eq!(hooks["PermissionRequest"][0]["matcher"], "*", "{name}");
            for absent in [
                "Notification",
                "PreCompact",
                "PostToolUseFailure",
                "SubagentStart",
            ] {
                assert!(
                    hooks.get(absent).is_none(),
                    "{name}: {absent} must not be registered"
                );
            }
            for (key, want) in [
                ("skipDangerousModePermissionPrompt", true),
                ("skipAutoPermissionPrompt", true),
                ("autoCompactEnabled", false),
            ] {
                assert_eq!(doc[key], want, "{name}: {key}");
            }
        }
        // Execute: the guard is first on PreToolUse with its narrow matcher,
        // the status hook second with `*`; the sentinel first on Stop.
        let pre = exec["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 2);
        assert_eq!(pre[0]["matcher"], "Bash|Edit|Write|MultiEdit|NotebookEdit");
        assert_eq!(
            pre[0]["hooks"][0]["command"],
            "\"ralphy.exe\" hook guard --cost-gate"
        );
        assert_eq!(pre[0]["hooks"][0]["type"], "command");
        assert_eq!(pre[1]["matcher"], "*");
        assert_eq!(pre[1]["hooks"][0]["command"], status);
        let stop = exec["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "\"ralphy.exe\" hook stop");
        assert_eq!(stop[0]["hooks"][0]["type"], "command");
        assert_eq!(stop[1]["hooks"][0]["command"], status);
        // The Bash timer first on PostToolUse, the status hook second with `*`.
        let post = exec["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(post.len(), 2);
        assert_eq!(post[0]["matcher"], "Bash");
        assert_eq!(post[0]["hooks"][0]["command"], "\"ralphy.exe\" hook post");
        assert_eq!(post[0]["hooks"][0]["type"], "command");
        assert_eq!(post[1]["matcher"], "*");
        assert_eq!(post[1]["hooks"][0]["command"], status);
        // Plan: the guard first, then the status hooks — no sentinel, no timer.
        let pre = plan["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 2);
        assert_eq!(pre[0]["matcher"], "Bash|Edit|Write|MultiEdit|NotebookEdit");
        assert_eq!(pre[0]["hooks"][0]["command"], "\"ralphy.exe\" hook guard");
        assert_eq!(pre[1]["matcher"], "*");
        assert_eq!(pre[1]["hooks"][0]["command"], status);
        let stop = plan["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1);
        assert_eq!(stop[0]["hooks"][0]["command"], status);
        let post = plan["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(post.len(), 1);
        assert_eq!(post[0]["matcher"], "*");
        assert_eq!(post[0]["hooks"][0]["command"], status);
    }

    /// ADR-0059 §4 as WRITTEN, not as built in memory: the files
    /// `write_exec_settings`/`write_plan_settings` leave in the run dir carry
    /// the status hook on every one of the seven events, and the plan file has
    /// no sentinel. Dropping the `status_hook_command` argument
    /// from either writer reds here where the pure-builder test stays green.
    #[test]
    fn the_written_settings_files_carry_the_status_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let agent = ClaudeAgent::new(None, None, dir.path().to_path_buf());
        // Both writers land on the SAME path (the run's one settings file), so
        // each is written and read back in turn.
        type Writer = fn(&ClaudeAgent) -> Result<PathBuf>;
        let writers: [(&str, Writer, bool); 2] = [
            ("exec", |a| a.write_exec_settings(), true),
            ("plan", |a| a.write_plan_settings(), false),
        ];
        for (name, write, sentinel_wanted) in writers {
            let path = write(&agent).unwrap();
            assert_eq!(path, dir.path().join("ralphy.settings.json"), "{name}");
            let doc: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let hooks = doc["hooks"]
                .as_object()
                .unwrap_or_else(|| panic!("{name}: hooks"));
            for event in [
                "SessionStart",
                "UserPromptSubmit",
                "PreToolUse",
                "PostToolUse",
                "PermissionRequest",
                "Stop",
                "SubagentStop",
            ] {
                let list = hooks[event]
                    .as_array()
                    .unwrap_or_else(|| panic!("{name}: {event}"));
                let status = list
                    .iter()
                    .filter_map(|e| e["hooks"][0]["command"].as_str())
                    .filter(|c| c.ends_with("hook status"))
                    .count();
                assert_eq!(status, 1, "{name}: one status hook on {event}");
            }
            let sentinel = hooks["Stop"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|e| e["hooks"][0]["command"].as_str())
                .any(|c| c.ends_with("hook stop"));
            assert_eq!(sentinel, sentinel_wanted, "{name}: sentinel presence");
            // The command quotes THIS binary's path.
            let cmd = hooks["SessionStart"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap();
            assert!(
                cmd.starts_with('"') && cmd.contains("\" hook status"),
                "{name}: {cmd}"
            );
        }
    }
    /// The guard command of a written settings file, if it carries one.
    fn guard_command_of(path: &Path) -> Option<String> {
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        doc["hooks"]["PreToolUse"]
            .as_array()?
            .iter()
            .filter_map(|e| e["hooks"][0]["command"].as_str())
            .find(|c| c.contains("hook guard"))
            .map(str::to_string)
    }

    /// Every settings file the Claude adapter writes carries the guard, and
    /// only the execute file turns the cost gate on. The task sessions share
    /// one writer: no task builds its own settings file.
    #[test]
    fn every_settings_file_carries_the_guard() {
        let dir = tempfile::tempdir().unwrap();
        let agent = ClaudeAgent::new(None, None, dir.path().join("run"));
        std::fs::create_dir_all(dir.path().join("run")).unwrap();
        let exec = agent.write_exec_settings().unwrap();
        let exec_guard = guard_command_of(&exec);
        let plan = agent.write_plan_settings().unwrap();
        let plan_guard = guard_command_of(&plan);
        let task = write_task_settings(&dir.path().join("task").join(".ralphy")).unwrap();
        let task_guard = guard_command_of(&task);
        for (name, got, want) in [
            ("exec", exec_guard, "hook guard --cost-gate"),
            ("plan", plan_guard, "hook guard"),
            ("task", task_guard, "hook guard"),
        ] {
            let got = got.unwrap_or_else(|| panic!("{name}: no guard hook"));
            assert!(got.ends_with(want), "{name}: {got}");
        }
        let tasks = include_str!("tasks.rs");
        assert!(
            !tasks.contains("std::fs::write(&settings_path"),
            "a task session writes its own settings file"
        );
        assert_eq!(tasks.matches("write_task_settings(").count(), 4);
    }
}
