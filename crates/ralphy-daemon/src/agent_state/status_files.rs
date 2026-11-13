//! The files one console's hooks need.

use std::path::{Path, PathBuf};

use ralphy_proc_util::hook_form::HookCommand;

use super::{console_settings_json, opencode, HookVendor};
use crate::session::Agent;

/// The files one console's hooks need, under `<store>/sessions/`: the
/// settings file a Claude console is launched with, and the status file the
/// hooks append to. A Codex console takes its hooks as arguments, so it has
/// no settings file. An `OpenCode` console has the state plugin file instead.
/// The files are the session's and go with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusFiles {
    pub vendor: HookVendor,
    pub settings: Option<PathBuf>,
    pub status: PathBuf,
    /// The state plugin an `OpenCode` console loads.
    pub plugin: Option<PathBuf>,
}

impl StatusFiles {
    /// The files of a Claude console.
    pub fn for_session(sessions_dir: &Path, id: u64) -> Self {
        StatusFiles {
            vendor: HookVendor::Claude,
            settings: Some(sessions_dir.join(format!("{id}.settings.json"))),
            status: sessions_dir.join(format!("{id}.agent-status.jsonl")),
            plugin: None,
        }
    }

    /// The files of a console of `agent`, or `None` when its state does not
    /// come from hooks.
    pub fn for_agent(agent: Agent, sessions_dir: &Path, id: u64) -> Option<Self> {
        let claude = StatusFiles::for_session(sessions_dir, id);
        match HookVendor::of(agent)? {
            HookVendor::Claude => Some(claude),
            // Codex takes its hooks as arguments; Antigravity's are in its owned root.
            vendor @ (HookVendor::Codex | HookVendor::Antigravity) => Some(StatusFiles {
                vendor,
                settings: None,
                status: claude.status,
                plugin: None,
            }),
            HookVendor::OpenCode => Some(StatusFiles {
                vendor: HookVendor::OpenCode,
                settings: None,
                status: claude.status,
                plugin: Some(sessions_dir.join(format!("{id}.state-plugin.js"))),
            }),
        }
    }

    /// Write the settings file (the §4 hook set, each the `hook` entry) and an
    /// empty status file. The settings are the hooks and NOTHING
    /// else: the vendor merges `--settings` over the operator's own file, so
    /// their permissions and hooks stay (ADR-0059 §5).
    pub fn write(&self, hook: &HookCommand) -> std::io::Result<()> {
        self.create_status()?;
        match &self.settings {
            Some(settings) => std::fs::write(settings, console_settings_json(hook)),
            None => Ok(()),
        }
    }

    /// Write the empty status file, and the folder it is in, and the state
    /// plugin when the console has one.
    pub fn create_status(&self) -> std::io::Result<()> {
        if let Some(dir) = self.status.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.status, b"")?;
        match &self.plugin {
            Some(plugin) => std::fs::write(plugin, opencode::STATE_PLUGIN),
            None => Ok(()),
        }
    }

    pub fn remove(&self) {
        // Best effort, and quiet: a session that ended has nothing to report
        // about a file the daemon itself created.
        if let Some(settings) = &self.settings {
            let _settings = std::fs::remove_file(settings);
        }
        if let Some(plugin) = &self.plugin {
            let _plugin = std::fs::remove_file(plugin);
        }
        let _status = std::fs::remove_file(&self.status);
    }
}
