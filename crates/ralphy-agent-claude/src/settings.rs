//! Claude run settings: the persisted [`ClaudeSettings`] schema (ADR-0010), the
//! headless skip-flag settings file, the execution-side [`ExecConfig`], the Stop/
//! guard/post hook wiring, and the plan's `## Execution model` judgment parser.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use ralphy_core::Plan;
use ralphy_proc_util::hook_form::{claude_hook, HookCommand, HookForm};

use crate::ClaudeAgent;

/// The settings every headless session shares: the flags that keep `claude -p`
/// from hanging on a prompt, and the guard's `PreToolUse` hook. Every session
/// Ralphy starts carries the guard (ADR-0072 D6); the phase builders add their
/// own hooks on top.
fn base_settings(guard: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "skipDangerousModePermissionPrompt": true,
        "skipAutoPermissionPrompt": true,
        "autoCompactEnabled": false,
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "Bash|Edit|Write|MultiEdit|NotebookEdit",
                    "hooks": [ guard ]
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
    /// Opt-in: start a workbench-opened Claude console in Claude Code's classic
    /// renderer (`CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN=1`), so the conversation
    /// stays in the terminal's history and the console's scrollbar reaches it.
    /// Left off, Claude Code picks its renderer itself; its fullscreen renderer
    /// draws on the alternate screen, which has no history to scroll.
    ///
    /// A plain `bool` for the reason `console_name` gives. Read by the daemon's
    /// launch path, which reparses the file; `crates/ralphy-daemon/src/session/spec.rs`
    /// pins the key against this schema.
    #[serde(default, skip_serializing_if = "is_false")]
    pub console_classic_screen: bool,
    /// Local-only opt-in: a session whose hooks did not prove they run goes
    /// on instead of being refused, with one warning event per session. It
    /// removes only the refusal: every hook is still written. Set only with
    /// `ralphy config set` on this machine; the daemon refuses to set it.
    // ADR-0072 D6: the escape of the hook proof.
    #[serde(default, skip_serializing_if = "is_false")]
    pub run_without_guard_proof_i_understand_the_risk: bool,
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
    /// `claude.run_without_guard_proof_i_understand_the_risk`: an unproven
    /// session goes on with a warning instead of being refused.
    pub(crate) run_without_guard_proof: bool,
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
            run_without_guard_proof: false,
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
    /// invokes *this* binary's `hook stop`.
    pub(crate) fn write_exec_settings(&self) -> Result<SettingsFile> {
        let exe = hook_exe().context("locating the ralphy binary for the Stop hook")?;
        let stop = hook_command(exe, &["hook", "stop"]);
        let guard = hook_command(exe, &["hook", "guard", "--cost-gate"]);
        let post = hook_command(exe, &["hook", "post"]);
        let status = hook_command(exe, &["hook", "status"]);
        let json = exec_settings_json(
            &stop.entry(),
            &guard.entry(),
            &post.entry(),
            &status.entry(),
        );
        let path = self.run_dir.join("ralphy.settings.json");
        std::fs::write(&path, json).context("writing exec settings")?;
        Ok(SettingsFile::new(path, [&stop, &guard, &post, &status]))
    }

    /// Write the plan phase's `ralphy.settings.json`: the skip flags, the
    /// guard without the cost gate, and the agent-state hooks — no Stop
    /// sentinel hook (ADR-0059 §4).
    pub(crate) fn write_plan_settings(&self) -> Result<SettingsFile> {
        let exe = hook_exe().context("locating the ralphy binary for the agent-state hook")?;
        let guard = hook_command(exe, &["hook", "guard"]);
        let status = hook_command(exe, &["hook", "status"]);
        let json = plan_settings_json(&guard.entry(), &status.entry());
        let path = self.run_dir.join("ralphy.settings.json");
        std::fs::write(&path, json).context("writing plan settings")?;
        Ok(SettingsFile::new(path, [&guard, &status]))
    }
}

/// A settings file the adapter wrote. `unproven` is the reason a hook of the
/// file runs through a shell, so that the form cannot be trusted; `None` when
/// every hook uses the exec form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingsFile {
    pub path: PathBuf,
    pub unproven: Option<String>,
}

impl SettingsFile {
    fn new<'a>(path: PathBuf, hooks: impl IntoIterator<Item = &'a HookCommand>) -> Self {
        let unproven = hooks
            .into_iter()
            .find(|h| h.form() == HookForm::Shell)
            .map(|h| h.reason().to_string());
        Self { path, unproven }
    }
}

/// This binary, as the hook commands name it, resolved once per process:
/// Claude Code runs a hook through Git Bash on Windows, which cannot run the
/// `\\?\` form.
fn hook_exe() -> Result<&'static Path> {
    static EXE: OnceLock<PathBuf> = OnceLock::new();
    if let Some(exe) = EXE.get() {
        return Ok(exe);
    }
    let exe = ralphy_proc_util::exe::running_exe()?;
    Ok(EXE.get_or_init(|| exe))
}

/// One hook of `ralphy`, in the form the installed `claude` runs.
fn hook_command(exe: &Path, args: &[&str]) -> HookCommand {
    let claude = crate::interactive::resolve_claude_binary();
    claude_hook(Path::new(&claude), exe, args)
}

/// A `hooks` array item for `matcher` that runs `hook`.
fn matcher_entry(matcher: &str, hook: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "matcher": matcher, "hooks": [ hook ] })
}

/// The settings of a one-shot task session (consolidate, diagnose, draft,
/// triage): the skip flags, the guard, and one status hook on `SessionStart`
/// so the session proves its hooks run.
fn task_settings_json(guard: &serde_json::Value, status: &serde_json::Value) -> String {
    let mut settings = base_settings(guard);
    settings["hooks"]["SessionStart"] = serde_json::json!([matcher_entry("", status)]);
    serde_json::to_string_pretty(&settings).expect("settings serialize")
}

/// Write a task session's `ralphy.settings.json` into `dir` (created when
/// missing).
pub(crate) fn write_task_settings(dir: &Path) -> Result<SettingsFile> {
    let exe = hook_exe().context("locating the ralphy binary for the guard hook")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let guard = hook_command(exe, &["hook", "guard"]);
    let status = hook_command(exe, &["hook", "status"]);
    let path = dir.join("ralphy.settings.json");
    std::fs::write(&path, task_settings_json(&guard.entry(), &status.entry()))
        .context("writing claude settings")?;
    Ok(SettingsFile::new(path, [&guard, &status]))
}

/// The agent-state hook events and their matchers (ADR-0059 §4 as amended
/// 2026-09-16, and ADR-0085 D7): `SessionStart`, `UserPromptSubmit`,
/// `PreToolUse` and `PostToolUse` on every tool, `PermissionRequest` on every
/// tool, `Elicitation` and `ElicitationResult` on every MCP server, `Stop`,
/// `SubagentStop` — two spawns per tool call, one per prompt, one per turn
/// end. `PostToolUse` and `ElicitationResult` are the events that END a
/// `waiting`. Deliberately absent: `Notification`, `PreCompact`,
/// `PostToolUseFailure`, `SubagentStart` — each would add a spawn and no
/// state. Pinned by `tests/fixtures/hook_events.json`, which the daemon's
/// console settings are pinned by too.
const STATUS_HOOK_EVENTS: [(&str, &str); 9] = [
    ("SessionStart", ""),
    ("UserPromptSubmit", ""),
    ("PreToolUse", "*"),
    ("PostToolUse", "*"),
    ("PermissionRequest", "*"),
    ("Elicitation", "*"),
    ("ElicitationResult", "*"),
    ("Stop", ""),
    ("SubagentStop", ""),
];

/// The agent-state hook set as `hooks` entries keyed by event.
fn status_hook_entries(status: &serde_json::Value) -> Vec<(&'static str, serde_json::Value)> {
    STATUS_HOOK_EVENTS
        .iter()
        .map(|(event, matcher)| (*event, matcher_entry(matcher, status)))
        .collect()
}

/// Append the status entries to a `hooks` object, AFTER whatever an event
/// already holds: the guard's narrow `PreToolUse` and the sentinel `Stop`
/// keep their first slot and their own command; the two are independent
/// hooks on the same event.
fn with_status_hooks(
    hooks: &mut serde_json::Map<String, serde_json::Value>,
    status: &serde_json::Value,
) {
    for (event, entry) in status_hook_entries(status) {
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
fn plan_settings_json(guard: &serde_json::Value, status: &serde_json::Value) -> String {
    let mut settings = base_settings(guard);
    if let Some(hooks) = settings["hooks"].as_object_mut() {
        with_status_hooks(hooks, status);
    }
    serde_json::to_string_pretty(&settings).expect("settings serialize")
}

/// Build the execution settings JSON: the headless skip flags, a `Stop` hook
/// running `stop`, a `PreToolUse` guard running `guard`, and a `PostToolUse`
/// Bash timer running `post` (the other half of the verification-cost gate:
/// the guard stamps a verify command's start, this hook records its measured
/// duration).
fn exec_settings_json(
    stop: &serde_json::Value,
    guard: &serde_json::Value,
    post: &serde_json::Value,
    status: &serde_json::Value,
) -> String {
    let mut settings = base_settings(guard);
    settings["hooks"]["Stop"] = serde_json::json!([matcher_entry("", stop)]);
    settings["hooks"]["PostToolUse"] = serde_json::json!([matcher_entry("Bash", post)]);
    if let Some(hooks) = settings["hooks"].as_object_mut() {
        with_status_hooks(hooks, status);
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
        let shell = |c: &str| serde_json::json!({ "type": "command", "command": c });
        let status = "\"ralphy.exe\" hook status";
        let exec: serde_json::Value = serde_json::from_str(&exec_settings_json(
            &shell("\"ralphy.exe\" hook stop"),
            &shell("\"ralphy.exe\" hook guard --cost-gate"),
            &shell("\"ralphy.exe\" hook post"),
            &shell(status),
        ))
        .unwrap();
        let plan: serde_json::Value = serde_json::from_str(&plan_settings_json(
            &shell("\"ralphy.exe\" hook guard"),
            &shell(status),
        ))
        .unwrap();
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

    /// The status hook events and their matchers are the shared list the
    /// daemon's console settings are pinned by too.
    #[test]
    fn the_status_hooks_match_the_shared_event_list() {
        let shared: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../tests/fixtures/hook_events.json")).unwrap();
        let shared: Vec<(String, String)> = shared
            .iter()
            .map(|e| {
                (
                    e["event"].as_str().unwrap().to_string(),
                    e["matcher"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        let status = serde_json::json!({ "type": "command", "command": "ralphy hook status" });
        let ours: Vec<(String, String)> = status_hook_entries(&status)
            .into_iter()
            .map(|(event, entry)| {
                assert_eq!(entry["hooks"][0], status, "{event}");
                (
                    event.to_string(),
                    entry["matcher"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(ours, shared);
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
        type Writer = fn(&ClaudeAgent) -> Result<SettingsFile>;
        let writers: [(&str, Writer, bool); 2] = [
            ("exec", |a| a.write_exec_settings(), true),
            ("plan", |a| a.write_plan_settings(), false),
        ];
        for (name, write, sentinel_wanted) in writers {
            let path = write(&agent).unwrap().path;
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
                    .map(|e| hook_text(&e["hooks"][0]))
                    .filter(|c| c.ends_with("hook status"))
                    .count();
                assert_eq!(status, 1, "{name}: one status hook on {event}");
            }
            let sentinel = hooks["Stop"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| hook_text(&e["hooks"][0]))
                .any(|c| c.ends_with("hook stop"));
            assert_eq!(sentinel, sentinel_wanted, "{name}: sentinel presence");
            // The command names THIS binary. On Windows the shell form (no
            // `claude` ≥ 2.1.139 on the machine) writes the path with `/`.
            let cmd = hook_text(&hooks["SessionStart"][0]["hooks"][0]).replace('\\', "/");
            let exe = hook_exe().unwrap().to_string_lossy().replace('\\', "/");
            assert!(cmd.contains(&exe), "{name}: {cmd}");
        }
    }
    /// What a hook entry runs, whatever its form: `command`, then `args`
    /// joined by spaces.
    fn hook_text(hook: &serde_json::Value) -> String {
        let mut parts = vec![hook["command"].as_str().unwrap_or_default().to_string()];
        if let Some(args) = hook["args"].as_array() {
            parts.extend(args.iter().filter_map(|a| a.as_str().map(str::to_string)));
        }
        parts.join(" ")
    }

    fn read_settings(path: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// The guard command of a written settings file, if it carries one.
    fn guard_command_of(path: &Path) -> Option<String> {
        read_settings(path)["hooks"]["PreToolUse"]
            .as_array()?
            .iter()
            .map(|e| hook_text(&e["hooks"][0]))
            .find(|c| c.contains("hook guard"))
    }

    /// Every settings file the Claude adapter writes carries the guard, and
    /// only the execute file turns the cost gate on. The task sessions share
    /// one writer: no task builds its own settings file.
    #[test]
    fn every_settings_file_carries_the_guard() {
        let dir = tempfile::tempdir().unwrap();
        let agent = ClaudeAgent::new(None, None, dir.path().join("run"));
        std::fs::create_dir_all(dir.path().join("run")).unwrap();
        let exec = agent.write_exec_settings().unwrap().path;
        let exec_guard = guard_command_of(&exec);
        let plan = agent.write_plan_settings().unwrap().path;
        let plan_guard = guard_command_of(&plan);
        let task = write_task_settings(&dir.path().join("task").join(".ralphy"))
            .unwrap()
            .path;
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

    /// The waiver removes only the refusal: with it on, the exec, plan and
    /// task files still carry the guard and the status hooks.
    #[test]
    fn the_waiver_keeps_every_hook() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("run")).unwrap();
        let agent =
            ClaudeAgent::new(None, None, dir.path().join("run")).with_guard_proof_waiver(true);
        assert!(agent.exec.run_without_guard_proof);
        let exec = agent.write_exec_settings().unwrap().path;
        let exec = (guard_command_of(&exec), read_settings(&exec));
        let plan = agent.write_plan_settings().unwrap().path;
        let plan = (guard_command_of(&plan), read_settings(&plan));
        let task = write_task_settings(&dir.path().join("task").join(".ralphy"))
            .unwrap()
            .path;
        let task = (guard_command_of(&task), read_settings(&task));
        for (name, (guard, doc)) in [("exec", exec), ("plan", plan), ("task", task)] {
            assert!(guard.is_some(), "{name}: no guard hook");
            let status = doc["hooks"]["SessionStart"]
                .as_array()
                .unwrap_or_else(|| panic!("{name}: SessionStart"))
                .iter()
                .any(|e| hook_text(&e["hooks"][0]).ends_with("hook status"));
            assert!(status, "{name}: no status hook");
        }
    }

    fn expected_entry(args: &[&str]) -> serde_json::Value {
        hook_command(hook_exe().unwrap(), args).entry()
    }

    /// Each hook in a built file is exactly the entry the module chose.
    #[test]
    fn the_settings_carry_the_entry_the_module_chose() {
        let dir = tempfile::tempdir().unwrap();
        let agent = ClaudeAgent::new(None, None, dir.path().to_path_buf());
        let status = expected_entry(&["hook", "status"]);
        let exec = read_settings(&agent.write_exec_settings().unwrap().path);
        let want = [
            ("Stop", 0, expected_entry(&["hook", "stop"])),
            ("Stop", 1, status.clone()),
            (
                "PreToolUse",
                0,
                expected_entry(&["hook", "guard", "--cost-gate"]),
            ),
            ("PreToolUse", 1, status.clone()),
            ("PostToolUse", 0, expected_entry(&["hook", "post"])),
            ("PostToolUse", 1, status.clone()),
            ("SessionStart", 0, status.clone()),
        ];
        for (event, slot, entry) in want {
            assert_eq!(
                exec["hooks"][event][slot]["hooks"][0], entry,
                "{event} {slot}"
            );
        }
        let plan = read_settings(&agent.write_plan_settings().unwrap().path);
        assert_eq!(
            plan["hooks"]["PreToolUse"][0]["hooks"][0],
            expected_entry(&["hook", "guard"])
        );
        assert_eq!(plan["hooks"]["Stop"][0]["hooks"][0], status);
        let form = hook_command(hook_exe().unwrap(), &["hook", "status"]).form();
        let entry = &status;
        match form {
            HookForm::Exec => {
                let args = entry["args"].as_array().unwrap();
                assert_eq!(&args[args.len() - 2..], ["hook", "status"]);
            }
            HookForm::Shell => assert!(entry["command"].as_str().unwrap().contains("hook")),
        }
    }

    /// A task session proves its hooks on `SessionStart`: exactly one status
    /// hook there, besides the guard.
    #[test]
    fn the_task_settings_carry_the_session_start_hook() {
        let dir = tempfile::tempdir().unwrap();
        let file = write_task_settings(dir.path()).unwrap();
        let doc = read_settings(&file.path);
        let start = doc["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(start.len(), 1);
        assert_eq!(start[0]["matcher"], "");
        assert!(hook_text(&start[0]["hooks"][0]).ends_with("hook status"));
        let hooks = doc["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), 2, "only PreToolUse and SessionStart");
        assert_eq!(hooks["PreToolUse"].as_array().unwrap().len(), 1);
    }

    /// `unproven` carries the module's reason for a shell form, and is `None`
    /// for the exec form.
    #[test]
    fn a_shell_form_settings_file_says_it_is_unproven() {
        let exe = Path::new("ralphy");
        let shell = ralphy_proc_util::hook_form::hook_for_version(Some("2.1.138"), exe, &["hook"]);
        let exec = ralphy_proc_util::hook_form::hook_for_version(Some("2.1.139"), exe, &["hook"]);
        assert_eq!(shell.form(), HookForm::Shell);
        assert_eq!(exec.form(), HookForm::Exec);
        let path = PathBuf::from("x.json");
        let mixed = SettingsFile::new(path.clone(), [&exec, &shell]);
        assert_eq!(mixed.unproven.as_deref(), Some(shell.reason()));
        let proven = SettingsFile::new(path, [&exec, &exec]);
        assert_eq!(proven.unproven, None);
    }
}
