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
                    "matcher": "Bash|PowerShell|Edit|Write|MultiEdit|NotebookEdit",
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
    /// Maximum number of `-p` calls before declaring `MaxCalls` (headless only).
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
mod tests;
