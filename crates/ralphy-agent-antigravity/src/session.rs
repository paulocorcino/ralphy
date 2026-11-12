//! The one way the crate starts a headless `agy` session (ADR-0086 D2, D7,
//! D9, D17): the owned root made ready, the fixed proof message first, the
//! task held until the status file proves the hooks run, the early kill on the
//! sign-in line, and a cooperative stop. The log of each session starts with
//! the `agy` version and the repository hooks file `agy` also runs (D10, D11).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result};
use ralphy_adapter_support::{HeadlessCall, HeadlessRun};
use ralphy_core::HooksUnproven;

use crate::command::{build_agy_command, user_message, RunFlags};
use crate::status::{self, Proof, SignedOut};

/// The proof turn's message: no task, one small model call. Its `SessionStart`
/// is the proof, and its usage counts in the run.
const PROOF_TEXT: &str = "Reply with the single word READY.";

/// [`PROOF_TEXT`] as the NDJSON line written before the task.
static PROOF_LINE: LazyLock<String> = LazyLock::new(|| user_message(PROOF_TEXT));

/// How long a stopped `agy` has to write its last `result` before the tree kill.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// How long `agy --version` may run.
const VERSION_LIMIT: Duration = Duration::from_secs(10);

/// The context of a failed start, unless the gate refused the task.
const SPAWN_ERR: &str = "failed to start the `agy` CLI (is it installed?)";

/// The proof turn's NDJSON line.
pub(crate) fn proof_line() -> &'static str {
    &PROOF_LINE
}

/// Where and how one `agy` session runs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Session<'a> {
    /// The `agy` program.
    pub(crate) program: &'a Path,
    /// The owned root, given as `--gemini_dir`.
    pub(crate) root: &'a Path,
    /// The child's working folder.
    pub(crate) work_dir: &'a Path,
    /// The folder of the session's status file.
    pub(crate) run_dir: &'a Path,
    /// Where the combined log is streamed.
    pub(crate) log_path: &'a Path,
    pub(crate) model: Option<&'a str>,
    pub(crate) effort: Option<&'a str>,
    /// The operator lets an unproven session go on.
    pub(crate) waiver: bool,
    /// The wall-clock budget.
    pub(crate) timeout: Duration,
    /// The idle window in minutes; `0` turns the watch off.
    pub(crate) idle_minutes: u64,
}

impl Session<'_> {
    /// Run `task` in a new session: the proof message, then the task as one
    /// NDJSON line once the proof came, then stdin closes.
    pub(crate) fn run(&self, task: &str) -> Result<HeadlessRun> {
        crate::prepare_root(self.root)?;
        let notes = self.launch_notes();
        let (cmd, status_file) = self.command()?;
        let task_line = user_message(task);
        let run = HeadlessCall::new(cmd, &task_line, self.timeout, self.log_path)
            .preamble(proof_line())
            .hold_prompt(self.proof(status_file).gate())
            .kill_on_stderr_line(crate::auth::is_auth_line)
            .cooperative_stop(STOP_GRACE)
            .idle_minutes(self.idle_minutes)
            .run()
            .map_err(spawn_context);
        // The run's own result matters more than its notes.
        if let Err(e) = head_log(self.log_path, &notes) {
            tracing::warn!("antigravity: the log has no launch notes: {e:#}");
        }
        run
    }

    /// The lines that head the session's log, read before the spawn: every
    /// launch but `--version` may update `agy` in the background.
    fn launch_notes(&self) -> String {
        let version = ralphy_proc_util::hook_form::read_version(self.program, VERSION_LIMIT);
        let version = version.as_deref().unwrap_or("unknown");
        tracing::info!("antigravity: agy version {version}");
        let mut notes = format!("agy version: {version}\n");
        let hooks = crate::root::workspace_hooks_file(self.work_dir, self.work_dir);
        if let Some(file) = hooks {
            tracing::warn!(
                "antigravity: agy also runs the repository hooks in {}",
                file.display()
            );
            notes.push_str("workspace hooks file: ");
            notes.push_str(&file.display().to_string());
            notes.push('\n');
        }
        notes
    }

    /// The child's command, pointed at a fresh status file, and that file.
    fn command(&self) -> Result<(Command, PathBuf)> {
        let status_file = status::fresh_status_file(self.run_dir)?;
        let flags = RunFlags {
            root: self.root,
            model: self.model,
            effort: self.effort,
        };
        let mut cmd = build_agy_command(self.program, self.work_dir, flags);
        cmd.env(status::STATUS_ENV, &status_file);
        Ok((cmd, status_file))
    }

    fn proof(&self, status_file: PathBuf) -> Proof {
        Proof {
            status_file,
            log_file: self.log_path.to_path_buf(),
            root: self.root.to_path_buf(),
            waiver: self.waiver,
            window: status::PROOF_WINDOW,
        }
    }
}

/// Put `notes` at the head of the log at `path`. The call starts its log
/// anew at the spawn, so the notes go in once it ended.
fn head_log(path: &Path, notes: &str) -> Result<()> {
    let body = match fs::read(path) {
        Ok(body) => body,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let mut text = notes.as_bytes().to_vec();
    text.extend(body);
    fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

/// Add [`SPAWN_ERR`] to `e`, unless the gate refused the task: the child had
/// started, and the refusal names its own cause.
fn spawn_context(e: anyhow::Error) -> anyhow::Error {
    let refused = e
        .chain()
        .any(|cause| cause.is::<HooksUnproven>() || cause.is::<SignedOut>());
    if refused {
        e
    } else {
        e.context(SPAWN_ERR)
    }
}

#[cfg(test)]
mod tests;
