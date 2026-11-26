//! The agent's own state from `OpenCode`'s bus events (ADR-0085 D2, D7).
//!
//! The run writes Ralphy's state plugin into the run dir and names it in
//! `OPENCODE_CONFIG_CONTENT`; the plugin appends one status line per state
//! event to `$RALPHY_STATUS_FILE`. This module is the other end:
//! [`opencode_signal`] maps a line to a fold signal, the shared fold applies
//! the D7 rules, and [`Watcher`] tails the file from a thread while a child
//! runs. The map and the fold are pinned by
//! `tests/fixtures/agent_state_mapping.json`, which the daemon's own copy is
//! pinned by too (ADR-0059 §5).

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use ralphy_adapter_support::agent_state::{self, Signal, StatusLine, WaitKind};
use ralphy_proc_util::opencode::OpenCodeMajor;

/// The file name of the status file under the run dir.
pub(crate) const STATUS_FILE: &str = "agent-status.jsonl";

/// The environment variable the plugin reads the status path from.
pub(crate) const STATUS_ENV: &str = "RALPHY_STATUS_FILE";

/// The file name of the state plugin under the run dir.
pub(crate) const PLUGIN_FILE: &str = "ralphy-state-plugin.js";

/// The plugin source. The daemon writes the same file for a console.
const STATE_PLUGIN: &str = include_str!("../../../assets/opencode/ralphy-state-plugin.js");

/// The folder name of the 2.x state plugin under the run dir (2.x loads a
/// folder, never a file).
pub(crate) const PLUGIN_DIR_V2: &str = "ralphy-state-plugin-v2";

/// The files of the 2.x plugin folder. The daemon writes the same files for a
/// console.
const STATE_PLUGIN_V2: [(&str, &str); 2] = [
    (
        "index.js",
        include_str!("../../../assets/opencode/ralphy-state-plugin-v2/index.js"),
    ),
    (
        "package.json",
        include_str!("../../../assets/opencode/ralphy-state-plugin-v2/package.json"),
    ),
];

// The same map is in `ralphy-daemon/src/agent_state/opencode.rs`, and it
// stays two copies: the daemon imports no adapter crate (ADR-0059 §5). The
// shared fixture test holds the two maps equal.
// jscpd:ignore-start
/// The tool `OpenCode` asks the operator a question with.
const QUESTION_TOOL: &str = "question";

/// The `OpenCode` map (ADR-0085 D7): one line of the state plugin in, one fold
/// signal out, or `None` for a line that changes no state. The plugin's
/// first line, `plugin.loaded`, is only the proof that it runs. A
/// `session.created` line is a new lead session: the plugin writes none for
/// a subagent's session, and it names a subagent's other lines with an
/// `agent_id`. A reply carries the id of the request it answers, so it ends
/// only that request's wait.
pub(crate) fn opencode_signal(line: &StatusLine) -> Option<Signal> {
    let request = line.tool_use_id.clone();
    let signal = match line.event.as_str() {
        "session.created" => Signal::SessionBoundary,
        "session.status" => Signal::Work,
        "permission.asked" => Signal::Wait {
            kind: WaitKind::Permission,
            tool_use_id: request,
            detail: match line.tool_name.as_deref() {
                Some(t) => format!("permission: {t}"),
                None => "permission".to_owned(),
            },
        },
        "question.asked" => Signal::Wait {
            kind: WaitKind::Question,
            tool_use_id: request,
            detail: match line.question.as_deref().map(str::trim) {
                Some(q) if !q.is_empty() => format!("{QUESTION_TOOL}: {q}"),
                _ => QUESTION_TOOL.to_owned(),
            },
        },
        "permission.replied" | "question.replied" | "question.rejected" => Signal::WaitEnd {
            tool_use_id: request,
        },
        "session.idle" => Signal::TurnEnd,
        _ => return None,
    };
    Some(signal)
}
// jscpd:ignore-end

/// The run dir's status file, tailed from a thread while a child runs; each
/// state change is emitted as an `agent state` event. `start` writes the
/// state plugin of the major (a file on 1.x, a folder on 2.x), truncates the
/// status file, and gives both paths to hand the child.
pub(crate) struct Watcher {
    inner: agent_state::Watcher,
    plugin: PathBuf,
}

impl Watcher {
    pub(crate) fn start(run_dir: &Path, major: OpenCodeMajor) -> anyhow::Result<Self> {
        let plugin = match major {
            OpenCodeMajor::V1 => write_plugin_file(run_dir)?,
            OpenCodeMajor::V2 => write_plugin_folder(run_dir)?,
        };
        let inner = agent_state::Watcher::start(run_dir.join(STATUS_FILE), opencode_signal)?;
        Ok(Watcher { inner, plugin })
    }

    /// The status file, for `RALPHY_STATUS_FILE`.
    pub(crate) fn path(&self) -> &Path {
        self.inner.path()
    }

    /// The plugin file or folder, for the `plugin` entry of the config.
    pub(crate) fn plugin(&self) -> &Path {
        &self.plugin
    }

    pub(crate) fn stop(self) {
        self.inner.stop();
    }
}

fn write_plugin_file(run_dir: &Path) -> anyhow::Result<PathBuf> {
    let plugin = run_dir.join(PLUGIN_FILE);
    std::fs::write(&plugin, STATE_PLUGIN)
        .with_context(|| format!("writing {}", plugin.display()))?;
    Ok(plugin)
}

fn write_plugin_folder(run_dir: &Path) -> anyhow::Result<PathBuf> {
    let folder = run_dir.join(PLUGIN_DIR_V2);
    std::fs::create_dir_all(&folder).with_context(|| format!("creating {}", folder.display()))?;
    for (name, text) in STATE_PLUGIN_V2 {
        let file = folder.join(name);
        std::fs::write(&file, text).with_context(|| format!("writing {}", file.display()))?;
    }
    Ok(folder)
}

#[cfg(test)]
mod tests;
