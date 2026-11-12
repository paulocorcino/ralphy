//! The session's status file and the proof that the hooks run (ADR-0086 D9):
//! `ralphy hook status antigravity` appends one line per hook event to the
//! file named by `RALPHY_STATUS_FILE`, and a session holds its task until the
//! file holds a `SessionStart` line.
#![expect(dead_code, reason = "used once the Agent impl lands")]

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use ralphy_adapter_support::agent_state::StatusLine;
use ralphy_adapter_support::PromptGate;
use ralphy_core::HooksUnproven;

/// The file name of a session's status file, in the session's run dir.
pub(crate) const STATUS_FILE: &str = "agent-status.jsonl";

/// The environment variable `hook status` reads the file path from.
pub(crate) const STATUS_ENV: &str = "RALPHY_STATUS_FILE";

/// How long a session has to write its `SessionStart` line (ADR-0072 D6).
pub(crate) const PROOF_WINDOW: Duration = Duration::from_secs(30);

/// How often the gate reads the status file and the log.
const POLL: Duration = Duration::from_millis(100);

/// The local-only setting that lets an unproven session go on.
pub(crate) const WAIVER_KEY: &str = "antigravity.run_without_guard_proof_i_understand_the_risk";

/// The hook event that proves the hooks run: `agy` fires it once per session,
/// on the first prompt.
const SESSION_START: &str = "SessionStart";

/// An empty status file in `dir`, made with its folder: the lines of an
/// earlier session are not this session's proof.
pub(crate) fn fresh_status_file(dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(STATUS_FILE);
    fs::write(&path, b"").with_context(|| format!("resetting {}", path.display()))?;
    Ok(path)
}

/// Whether the status file at `path` holds a `SessionStart` line. A file that
/// is missing holds none.
pub(crate) fn has_session_start(path: &Path) -> bool {
    any_line(path, |line| {
        StatusLine::parse(line.trim()).is_some_and(|status| status.event == SESSION_START)
    })
}

/// Whether a line of the text file at `path` matches `pred`.
fn any_line(path: &Path, pred: impl Fn(&str) -> bool) -> bool {
    fs::read(path).is_ok_and(|bytes| String::from_utf8_lossy(&bytes).lines().any(pred))
}

/// The gate's error when `agy` asked for a sign-in before the proof came:
/// the session ends with the sign-in message, not as unproven hooks.
#[derive(Debug)]
pub(crate) struct SignedOut(pub(crate) String);

impl fmt::Display for SignedOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SignedOut {}

/// What the gate of one session reads and how it ends.
#[derive(Debug, Clone)]
pub(crate) struct Proof {
    /// The session's status file.
    pub(crate) status_file: PathBuf,
    /// The session's streamed log, read for the sign-in line.
    pub(crate) log_file: PathBuf,
    /// The owned root, whose hooks file the refusal names.
    pub(crate) root: PathBuf,
    /// The operator set [`WAIVER_KEY`].
    pub(crate) waiver: bool,
    /// How long to wait for the line.
    pub(crate) window: Duration,
}

impl Proof {
    /// The prompt gate of the session. With the waiver, an unproven session
    /// goes on with one `hooks not proven` event.
    pub(crate) fn gate(self) -> PromptGate<'static> {
        Box::new(move || self.hold(ralphy_core::emit::hooks_unproven_allowed))
    }

    /// Wait for the proof. Past the window, refuse; with the waiver, give the
    /// cause to `warn` once and go on.
    fn hold(&self, warn: impl FnOnce(&str)) -> Result<()> {
        match self.wait() {
            Waited::Proven => Ok(()),
            Waited::SignedOut => Err(SignedOut(self.sign_in_message()).into()),
            Waited::Unproven if self.waiver => {
                warn(&self.cause());
                Ok(())
            }
            Waited::Unproven => {
                let cause = format!(
                    "{}. Fix the cause and start again. To run without this proof on this \
                     machine, run `ralphy config set {WAIVER_KEY} true`",
                    self.cause()
                );
                Err(HooksUnproven { cause }.into())
            }
        }
    }

    fn wait(&self) -> Waited {
        let deadline = Instant::now() + self.window;
        loop {
            if has_session_start(&self.status_file) {
                return Waited::Proven;
            }
            if any_line(&self.log_file, crate::auth::is_auth_line) {
                return Waited::SignedOut;
            }
            if Instant::now() >= deadline {
                return Waited::Unproven;
            }
            std::thread::sleep(POLL);
        }
    }

    fn cause(&self) -> String {
        format!(
            "no SessionStart hook event came in {} s, so the hooks Ralphy wrote in {} \
             did not prove they run",
            self.window.as_secs(),
            self.root.display()
        )
    }

    fn sign_in_message(&self) -> String {
        format!(
            "{} (see {})",
            crate::auth::auth_message(&self.root),
            self.log_file.display()
        )
    }
}

/// How the wait for the proof ended.
enum Waited {
    Proven,
    SignedOut,
    Unproven,
}

#[cfg(test)]
mod tests;
