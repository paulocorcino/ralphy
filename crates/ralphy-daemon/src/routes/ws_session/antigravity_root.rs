//! An Antigravity console runs in Ralphy's owned root, with Ralphy's hooks file
//! in it (ADR-0086 D8, D9). The daemon may not import the adapter that writes
//! the root (ADR-0032 §10), so when the hooks file is missing it runs the CLI's
//! `antigravity prepare-root` and looks again (ADR-0040 Amendment 3).
//!
//! INVARIANT: fail closed. Only the hooks FILE opens the console: a root that
//! cannot be resolved, a failed or slow command, or a success that wrote
//! nothing all end in `Err`, and the caller refuses before anything is spawned.

use std::path::{Path, PathBuf};

use crate::dispatch::{self, Collected};
use crate::routes::blocking_read;
use crate::routes::ws_command::collect_config;

/// The head of every reason, so the console names the agent it refused.
const HEAD: &str = "antigravity";

/// The longest reason passed on to the console, in characters.
const REASON_MAX: usize = 300;

/// Make sure the owned root has its hooks file, preparing the root when it
/// does not. The command runs in `repo`. `Err` is the reason the console shows.
pub(crate) async fn ensure(repo: &Path, daemon_id: &str) -> Result<(), String> {
    let hooks = hooks_file().map_err(|why| headed(&why))?;
    if exists(hooks.clone()).await {
        return Ok(());
    }
    let daemon_id = Some(daemon_id.to_owned()).filter(|id| !id.is_empty());
    let argv = dispatch::antigravity_root_argv();
    let ran = collect_config(argv, repo.to_path_buf(), daemon_id).await;
    prepared(ran).map_err(|why| headed(&why))?;
    if !exists(hooks.clone()).await {
        return Err(headed(
            "the hooks file is still missing after preparing the root",
        ));
    }
    tracing::info!(hooks = %hooks.display(), "prepared the Antigravity root for a console");
    Ok(())
}

/// `<owned root>/config/hooks.json`, the file `agy` reads its global hooks from.
fn hooks_file() -> Result<PathBuf, String> {
    ralphy_proc_util::antigravity::owned_root()
        .map(|root| root.join("config").join("hooks.json"))
        .map_err(|e| format!("{e:#}"))
}

/// A filesystem read off the runtime. A read that did not complete is "no
/// file", which refuses the launch.
async fn exists(path: PathBuf) -> bool {
    blocking_read(move || path.is_file()).await.unwrap_or(false)
}

/// `Ok` when `antigravity prepare-root` exited 0; else why it did not.
fn prepared(ran: Collected) -> Result<(), String> {
    match ran {
        Collected::Done(Some(0), _) => Ok(()),
        Collected::Done(code, out) => Err(cli_reason(&out).unwrap_or_else(|| match code {
            Some(code) => format!("preparing its root failed (exit {code})"),
            None => "preparing its root was stopped".to_owned(),
        })),
        Collected::Failed(e) => {
            let error = format!("{e:#}");
            tracing::warn!(%error, "could not run `ralphy antigravity prepare-root`");
            Err("could not run the command that prepares its root".to_owned())
        }
        Collected::StillRunning => Err("preparing its root did not finish in time".to_owned()),
        Collected::NoSlot => Err("the daemon is busy; open the console again soon".to_owned()),
    }
}

/// The CLI's own reason: its last line with text, without the `Error: ` head.
fn cli_reason(out: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(out);
    let last = text.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    Some(last.trim_start_matches("Error: ").to_owned())
}

/// `why` under the agent's head, cut to [`REASON_MAX`] characters.
fn headed(why: &str) -> String {
    format!("{HEAD}: {why}").chars().take(REASON_MAX).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The daemon cannot import the adapter, so it names the hooks file itself.
    #[test]
    fn the_hooks_file_layout_matches_the_adapters_own() {
        let root = include_str!("../../../../ralphy-agent-antigravity/src/root.rs");
        assert!(
            root.contains(r#"root.join("config").join("hooks.json")"#),
            "the adapter moved the hooks file the daemon's launch gate checks"
        );
    }

    #[test]
    fn a_reason_is_the_last_line_under_the_head_and_cut() {
        let out = b"a log line\r\nError: the program path has a space\r\n\r\n";
        let why = prepared(Collected::Done(Some(1), out.to_vec())).unwrap_err();
        assert_eq!(headed(&why), "antigravity: the program path has a space");
        let silent = prepared(Collected::Done(Some(2), Vec::new())).unwrap_err();
        assert_eq!(silent, "preparing its root failed (exit 2)");
        assert_eq!(headed(&"x".repeat(1000)).chars().count(), REASON_MAX);
    }
}
