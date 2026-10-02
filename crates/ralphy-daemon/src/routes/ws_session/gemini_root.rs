//! A Gemini console needs the owned configuration root and its policy
//! document (ADR-0043 D4/D5). The daemon may not import the adapter that
//! writes them (ADR-0032 §10), so when they are missing it runs the CLI's
//! `gemini prepare-root`, the adapter's own generator, and checks again
//! (ADR-0040 Amendment 3).
//!
//! INVARIANT: the answer keys on the FILE, never on the exit code. A failed
//! command, a timeout, or a success that wrote nothing all end in `Err`, and
//! the caller refuses the launch before anything is spawned.

use std::path::{Path, PathBuf};

use crate::dispatch::{self, Collected};
use crate::routes::blocking_read;
use crate::routes::ws_command::collect_config;
use crate::session;

/// The longest reason passed on to the console, in characters.
const REASON_MAX: usize = 300;

/// Make sure `root`'s policy document exists, preparing it when it does not.
/// `Err` is the reason the console shows.
pub(crate) async fn ensure(root: &Path, daemon_id: &str) -> Result<(), String> {
    let policy = session::gemini_policy_path(root);
    if is_file(policy.clone()).await {
        return Ok(());
    }
    let argv = dispatch::gemini_root_argv();
    let daemon_id = Some(daemon_id.to_string()).filter(|id| !id.is_empty());
    match collect_config(argv, root.to_path_buf(), daemon_id).await {
        Collected::Done(Some(0), _) => {}
        Collected::Done(code, out) => {
            return Err(failure_reason(&out).unwrap_or_else(|| match code {
                Some(code) => format!("gemini: preparing its configuration failed (exit {code})"),
                None => "gemini: preparing its configuration was stopped".to_string(),
            }))
        }
        Collected::Failed(e) => {
            tracing::warn!(error = %format!("{e:#}"), "could not run `ralphy gemini prepare-root`");
            return Err("gemini: could not run the command that prepares its configuration".into());
        }
        Collected::StillRunning => {
            return Err("gemini: preparing its configuration did not finish in time".into())
        }
        Collected::NoSlot => {
            return Err("gemini: the daemon is busy; open the console again in a moment".into())
        }
    }
    if !is_file(policy.clone()).await {
        return Err(
            "gemini: its policy file is still missing after preparing its configuration".into(),
        );
    }
    tracing::info!(policy = %policy.display(), "prepared Gemini's own configuration for a console");
    Ok(())
}

/// A filesystem read, off the runtime like every other one in the routes. A
/// read that did not complete counts as "no file", which refuses the launch.
async fn is_file(path: PathBuf) -> bool {
    blocking_read(move || path.is_file()).await.unwrap_or(false)
}

/// The command's own reason: its last non-empty output line, without the
/// `Error: ` head the CLI prints, under a `gemini: ` head, cut to
/// [`REASON_MAX`] characters.
fn failure_reason(out: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(out);
    let line = text.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    let line = line.strip_prefix("Error: ").unwrap_or(line);
    let reason: String = format!("gemini: {line}").chars().take(REASON_MAX).collect();
    Some(reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reason_is_the_last_line_without_the_error_head() {
        let out = b"some log line\r\nError: autonomy is disabled by your administrator\r\n\r\n";
        assert_eq!(
            failure_reason(out).as_deref(),
            Some("gemini: autonomy is disabled by your administrator")
        );
        assert_eq!(failure_reason(b" \n\n"), None, "no line is no reason");
        let long = format!("Error: {}", "x".repeat(1000));
        assert_eq!(
            failure_reason(long.as_bytes()).unwrap().chars().count(),
            REASON_MAX
        );
    }
}
