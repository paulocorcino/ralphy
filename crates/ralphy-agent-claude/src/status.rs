//! The agent's own state from Claude Code's hooks (docs/adr/0059 §1, §3).
//!
//! `ralphy hook status` appends one JSON line per hook event to
//! `$RALPHY_STATUS_FILE`; this module is the other end: [`claude_signal`] maps
//! a line to a fold signal, the shared fold applies the ADR-0085 D7 rules, and
//! [`Watcher`] tails the file from a thread while a child runs — plan,
//! interactive and headless alike — and calls `emit::agent_state` on a CHANGE
//! only. The map and the fold are pinned by
//! `tests/fixtures/agent_state_mapping.json`, which the daemon's own copy is
//! pinned by too (§5: the daemon never imports this crate).
//!
//! Nothing here reads the terminal: a state that is guessed is worse than one
//! that is absent (§7).

use std::path::{Path, PathBuf};
use std::time::Duration;

use ralphy_adapter_support::agent_state::{self, Signal, StatusLine, WaitKind};
use serde_json::Value;

/// The file name under the run dir, beside `status.flag`.
pub(crate) const STATUS_FILE: &str = "agent-status.jsonl";

/// The environment variable the hook reads the path from.
pub(crate) const STATUS_ENV: &str = "RALPHY_STATUS_FILE";

// The same map is in `ralphy-daemon/src/agent_state/claude.rs`, and it stays
// two copies: the daemon imports no adapter crate (ADR-0059 §5). The shared
// fixture test holds the two maps equal.
// jscpd:ignore-start
/// The Claude map (ADR-0059 §1 as amended by ADR-0085 D7): one status line
/// in, one fold signal out, or `None` for a line that changes no state
/// (`SubagentStop`: a subagent never owns the state; `Notification`, which
/// stays out by ADR-0085 D4; an event outside the table). Claude sends no
/// event on an interrupt (D7.6), so no line maps to `Interrupt`.
pub(crate) fn claude_signal(line: &StatusLine) -> Option<Signal> {
    let tool = line.tool_name.as_deref();
    let signal = match line.event.as_str() {
        "SessionStart" => Signal::SessionBoundary,
        "UserPromptSubmit" => Signal::TurnStart,
        "PreToolUse" if tool == Some("AskUserQuestion") => Signal::Wait {
            kind: WaitKind::Question,
            tool_use_id: line.tool_use_id.clone(),
            detail: named("AskUserQuestion", line.question.as_deref()),
        },
        "PreToolUse" => Signal::Work,
        // A tool that returned: the question was answered, the permission
        // granted. A wait ends here, not at the next tool (2026-09-16).
        "PostToolUse" | "ElicitationResult" => Signal::WaitEnd {
            tool_use_id: line.tool_use_id.clone(),
        },
        "PermissionRequest" => Signal::Wait {
            kind: WaitKind::Permission,
            tool_use_id: line.tool_use_id.clone(),
            detail: match tool {
                Some(t) => format!("permission: {t}"),
                None => "permission".to_string(),
            },
        },
        "Elicitation" => Signal::Wait {
            kind: WaitKind::Elicitation,
            tool_use_id: None,
            detail: named("MCP question", line.question.as_deref()),
        },
        "Stop" => Signal::TurnEnd,
        _ => return None,
    };
    Some(signal)
}

/// A wait's detail: `name`, and the question after it when there is one.
fn named(name: &str, question: Option<&str>) -> String {
    match question.map(str::trim).filter(|q| !q.is_empty()) {
        Some(q) => format!("{name}: {q}"),
        None => name.to_string(),
    }
}
// jscpd:ignore-end

/// The run dir's status file, tailed from a thread while a child runs; each
/// state change is emitted as an `agent state` event. `start` truncates the
/// file and returns the path to hand the child as `RALPHY_STATUS_FILE`.
pub(crate) struct Watcher(agent_state::Watcher);

impl Watcher {
    pub(crate) fn start(run_dir: &Path) -> anyhow::Result<Self> {
        agent_state::Watcher::start(run_dir.join(STATUS_FILE), claude_signal).map(Watcher)
    }

    pub(crate) fn path(&self) -> &Path {
        self.0.path()
    }

    pub(crate) fn stop(self) {
        self.0.stop();
    }
}

/// How long a session has to write its `SessionStart` line before the
/// prompt is refused (ADR-0072 D6, amendment of 2026-10-28). M5 and M6 of
/// `docs/research/claude-hook-probes-2026-10-28.md` measured 0.8 s to 3.2 s.
pub(crate) const PROOF_WINDOW: Duration = Duration::from_secs(30);

/// How often [`await_session_start`] reads the status file.
const PROOF_POLL: Duration = Duration::from_millis(200);

/// Wait until the status file at `path` holds a `SessionStart` line: the
/// proof that the hooks of `settings` run in this session. `project` is the
/// session's working folder, read for a `disableAllHooks` setting when the
/// line does not come. After `window` with no line, the error is the
/// [`ralphy_core::HooksUnproven`] cause, for [`refuse_or_go_on`].
pub(crate) fn await_session_start(
    path: &Path,
    settings: &Path,
    project: &Path,
    window: Duration,
) -> Result<(), ralphy_core::HooksUnproven> {
    let deadline = std::time::Instant::now() + window;
    loop {
        if session_started(path, 0) {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(no_session_start(settings, project, window));
        }
        std::thread::sleep(PROOF_POLL);
    }
}

/// Whether the status file at `path` holds a `SessionStart` line after its
/// first `from` bytes. A respawned child shares the file with the first one,
/// so it passes the length the file had before its spawn. Never blocks.
pub(crate) fn session_started(path: &Path, from: u64) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let start = usize::try_from(from).map_or(bytes.len(), |f| f.min(bytes.len()));
    String::from_utf8_lossy(&bytes[start..])
        .lines()
        .any(|line| {
            serde_json::from_str::<Value>(line.trim())
                .is_ok_and(|v| v.get("event").and_then(Value::as_str) == Some("SessionStart"))
        })
}

/// The cause of a session that wrote no `SessionStart` line in `window`.
pub(crate) fn no_session_start(
    settings: &Path,
    project: &Path,
    window: Duration,
) -> ralphy_core::HooksUnproven {
    ralphy_core::HooksUnproven {
        cause: no_proof_cause(settings, project, window),
    }
}

/// The cause of a missing proof. A `disableAllHooks` file is named first: it
/// alone explains the silence.
fn no_proof_cause(settings: &Path, project: &Path, window: Duration) -> String {
    let home = ralphy_proc_util::home_dir();
    let mut cause = String::new();
    if let Some(file) =
        ralphy_proc_util::hook_form::disable_all_hooks(home.as_deref(), Some(project))
    {
        cause.push_str(&format!(
            "{} sets disableAllHooks to true, so Claude Code runs no hook; ",
            file.display()
        ));
    }
    let program = ralphy_proc_util::exe::running_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|e| format!("the ralphy program (not found: {e:#})"));
    cause.push_str(&format!(
        "no SessionStart hook event came in {} s; the hooks in {} run {}. Fix the cause and start again",
        window.as_secs(),
        settings.display(),
        program
    ));
    cause
}

/// The cause of a session whose hooks would run through a shell: such a
/// hook cannot prove the program it runs. `reason` is the hook form module's.
pub(crate) fn shell_form(reason: &str) -> ralphy_core::HooksUnproven {
    ralphy_core::HooksUnproven {
        cause: format!("{reason}; update Claude Code to 2.1.139 or later"),
    }
}

/// The settings key that lets an unproven session go on.
pub(crate) const WAIVER_KEY: &str = "claude.run_without_guard_proof_i_understand_the_risk";

/// Every unproven session ends here. With the waiver (a local-only setting,
/// ADR-0072 D6) the session goes on and one warning event carries the
/// cause; without it, the refusal names the cause and the way out.
pub(crate) fn refuse_or_go_on(
    cause: ralphy_core::HooksUnproven,
    waiver: bool,
) -> anyhow::Result<()> {
    if waiver {
        ralphy_core::emit::hooks_unproven_allowed(&cause.cause);
        return Ok(());
    }
    Err(anyhow::Error::new(ralphy_core::HooksUnproven {
        cause: format!(
            "{}. To run without the proof on this machine, run `ralphy config set {WAIVER_KEY} true`",
            cause.cause
        ),
    }))
}

/// The gate of a headless session: refuse a shell form now, before the
/// spawn, else hold the prompt until the session's `SessionStart` line lands
/// in `status_file`. With the waiver, a shell form sends the prompt at once
/// and a missing line sends it after the window.
pub(crate) fn proof_gate<'a>(
    settings: &'a crate::settings::SettingsFile,
    status_file: PathBuf,
    project: &'a Path,
    waiver: bool,
) -> anyhow::Result<ralphy_adapter_support::PromptGate<'a>> {
    if let Some(reason) = &settings.unproven {
        refuse_or_go_on(shell_form(reason), waiver)?;
        return Ok(Box::new(|| Ok(())));
    }
    Ok(Box::new(move || {
        match await_session_start(&status_file, &settings.path, project, PROOF_WINDOW) {
            Ok(()) => Ok(()),
            Err(cause) => refuse_or_go_on(cause, waiver),
        }
    }))
}

/// Add `ctx` to a spawn error `e`, unless `e` is a refusal for hooks that
/// did not prove they run. The spawn context of the plan asks whether the
/// program is installed and on PATH, and that is the wrong place to look
/// for a refusal; the refusal names its own cause.
pub(crate) fn spawn_context(e: anyhow::Error, ctx: &'static str) -> anyhow::Error {
    if e.downcast_ref::<ralphy_core::HooksUnproven>().is_some() {
        e
    } else {
        e.context(ctx)
    }
}

#[cfg(test)]
mod tests;
