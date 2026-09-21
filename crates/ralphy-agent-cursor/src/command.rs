//! Building the headless `cursor-agent` invocation: resolving a binary that is on
//! `PATH` on neither platform (ADR-0042 D14), seeding the scratch configuration
//! directory that keeps a run out of the operator's own Cursor state (D17), and
//! fixing the argv that refuses this vendor's default blast radius (D4/D7/D18).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

/// Mint the session id Ralphy hands the CLI with `--resume`. A v4 UUID: `--resume`
/// with an id that has never existed is accepted silently and echoed back as
/// `system/init.session_id`, so `create-chat` costs a process spawn for nothing
/// (ADR-0042 D10). Adoption is VERIFIED against this value, never assumed.
pub(crate) fn mint_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// The model value Ralphy sends when it has no preference. **Never omission**: on
/// this vendor an absent `--model` does not mean "the account default", it means
/// "whatever the last invocation left in `cli-config.json`" (ADR-0042 D4).
pub(crate) const AUTO_MODEL: &str = "auto";

/// The vendor's own name for the two shims it installs for one binary (D14).
const NAMES: [&str; 2] = ["cursor-agent", "agent"];

/// Locate the Cursor CLI against the real environment. `None` means the vendor is
/// not installed — `ralphy init`'s gate reports presence through this, never
/// through `locate_program("cursor")`, which would look for the wrong binary name.
///
/// The search itself lives in `ralphy-proc-util` (ADR-0042 D19): the daemon's
/// interactive launch needs the same resolution and may not import the core, which
/// this crate does.
pub fn locate_cursor() -> Option<PathBuf> {
    ralphy_proc_util::cursor::locate_cursor()
}

/// What a `Command` is constructed with. Falls back to the bare name so the spawn
/// failure names the vendor rather than an empty path.
pub(crate) fn resolve_cursor_program() -> OsString {
    locate_cursor()
        .map(PathBuf::into_os_string)
        .unwrap_or_else(|| NAMES[0].into())
}

/// The single configuration file that carries the operator's policy (D17). Their
/// `permissions.deny` list lives here, and D7 says that policy is deliberate — so
/// it flows IN to the scratch directory. Nothing flows back.
const CLI_CONFIG: &str = "cli-config.json";

/// Where the operator's own Cursor configuration lives: their explicit
/// `CURSOR_CONFIG_DIR` if they set one (an operator already isolating Cursor is
/// still entitled to their own policy), else `$XDG_CONFIG_HOME/cursor`, else
/// `~/.cursor`.
pub(crate) fn operator_config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CURSOR_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join("cursor"));
    }
    ralphy_adapter_support::home_dir().map(|h| h.join(".cursor"))
}

/// D17: seed the run's scratch configuration directory from the operator's own.
///
/// `--model` — successful or rejected — rewrites `cli-config.json`'s `model`,
/// `selectedModel` and `modelSelectionHistory` keys, so an unisolated run would
/// reassign the default model of the operator's interactive Cursor sessions. The
/// scratch directory contains that: policy flows in, mutations die with the run.
///
/// **Copies one file, one way.** A missing operator directory or a missing
/// `cli-config.json` is not an error — a fresh install has neither, and the run
/// proceeds against vendor defaults.
pub(crate) fn seed_cursor_config_dir(operator_dir: Option<&Path>, scratch: &Path) -> Result<()> {
    std::fs::create_dir_all(scratch)
        .with_context(|| format!("creating the scratch config dir {}", scratch.display()))?;
    let Some(src) = operator_dir.map(|d| d.join(CLI_CONFIG)) else {
        return Ok(());
    };
    if !src.is_file() {
        return Ok(());
    }
    std::fs::copy(&src, scratch.join(CLI_CONFIG))
        .with_context(|| format!("seeding {} from {}", scratch.display(), src.display()))?;
    Ok(())
}

/// Build the headless `cursor-agent` command both `plan` and `execute` go through.
///
/// The charter is NEVER on argv: `prompt.plan.staged.md` is 25 917 bytes before any
/// issue body against a Windows argv ceiling of ~32 KB, and the spike verified a
/// 26 372-byte payload arriving whole on stdin with markers intact on its first and
/// last line (D2). `-p` here is the vendor's *print mode* switch, which takes no
/// value — there is no prompt word in the argv at all.
///
/// `--model` is ALWAYS present, `auto` when Ralphy has no preference (D4 — see
/// [`AUTO_MODEL`]; omitting it is never correct on this vendor).
/// `--force` is required for non-interactive operation, and the operator's own
/// `permissions.deny` still wins over it (D7).
/// `--output-format stream-json` selects the record stream the fold reads.
/// `--resume` carries Ralphy's minted id, so the session is addressable before the
/// child is spawned (D10).
///
/// The refused flags are refused by ABSENCE, and each is a capability: `--auto-review`
/// (a server-side classifier that prompts — fatal headless — and ships tool-call
/// decisions to a Cursor service), `--approve-mcps` (`.cursor/mcp.json` is
/// repo-local, so a cloned repository could propose servers), `-w`/`--worktree`/
/// `--worktree-base` (Ralphy owns its branches, and `.cursor/worktrees.json`
/// executes repo-local setup scripts), `--mode plan`/`--plan` (hard read-only and
/// it overrides the charter, D9). `--sandbox` is deliberately left unset: forcing a
/// sandbox mode is a capability decision this spike gathered no evidence for.
///
/// The environment is set with care. `CURSOR_CONFIG_DIR` is the D17 containment.
/// `CURSOR_AGENT_DISABLE_DEBUG_LOG` turns off a debug log the CLI writes for every
/// invocation, unasked, into the OS temp directory (D18) — a queue run produces
/// hundreds of invocations and the files name the operator's repositories.
/// On **Windows only**, `SHELL` (plus `MSYSTEM`) is pinned to a located git-bash
/// (D20) so the vendor runs shell tool calls under bash, not PowerShell — where a
/// POSIX heredoc commit message (`-m "$(cat <<'EOF' … EOF)"`) ParserErrors. `MSYSTEM`
/// is required with it: without the MSYS runtime flag git-bash returns "no exit
/// status" for every command. Both are left untouched when the operator already set
/// `SHELL` or no git-bash is found (see [`git_bash_shell_pin`]).
/// `CURSOR_API_KEY`/`CURSOR_AUTH_TOKEN` are left untouched: Ralphy sets neither,
/// and scrubbing them would break an operator who authenticates that way (D8).
pub(crate) fn build_cursor_command(
    session_id: &str,
    model: Option<&str>,
    work_dir: &Path,
    config_dir: &Path,
) -> Command {
    let mut cmd = Command::new(resolve_cursor_program());
    cmd.current_dir(work_dir)
        .arg("-p")
        .arg("--model")
        .arg(model.unwrap_or(AUTO_MODEL))
        .arg("--force")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--resume")
        .arg(session_id)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("CURSOR_CONFIG_DIR", config_dir)
        .env("CURSOR_AGENT_DISABLE_DEBUG_LOG", "1");
    // D20: on Windows, pin SHELL to git-bash so the vendor's shell classifier picks
    // bash over PowerShell and POSIX heredoc commits stop ParserError-ing. Windows
    // only — on Linux/macOS SHELL is already a POSIX shell and must be left alone.
    // `MSYSTEM` rides along: git-bash spawned without its MSYS runtime flag flips the
    // classifier but then returns "no exit status" for every command (the vendor's
    // persistent shell cannot read an exit code back) — verified live, SHELL alone is
    // a worse break than the papercut it fixes, SHELL+MSYSTEM runs commands cleanly.
    #[cfg(windows)]
    if let Some(shell) = git_bash_shell_pin(
        std::env::var_os("SHELL"),
        ralphy_proc_util::locate_git_bash(),
    ) {
        cmd.env("SHELL", shell).env("MSYSTEM", GIT_BASH_MSYSTEM);
    }
    cmd
}

/// The MSYS runtime flavour git-bash sets for a 64-bit Git-for-Windows shell. Ralphy
/// sets it alongside `SHELL` (D20) because `cursor-agent`'s persistent shell cannot
/// read an exit code back from git-bash unless the MSYS runtime is initialised, which
/// this flag triggers.
#[cfg(windows)]
const GIT_BASH_MSYSTEM: &str = "MINGW64";

/// The `SHELL` a Windows run pins so `cursor-agent` runs its shell tool calls under
/// git-bash, not PowerShell (ADR-0042 D20). The vendor's classifier reads `SHELL`;
/// native `ralphy.exe` spawns the CLI with `MSYSTEM` absent and `SHELL` empty, so it
/// falls through to PowerShell and a POSIX heredoc commit ParserErrors on the first
/// try of every execute session.
///
/// `Some(path)` says set it; `None` leaves `SHELL` exactly as the parent env has it,
/// in the two cases the fix must not touch:
/// - the operator already set `SHELL` (`existing_shell` is `Some`) — a deliberate
///   choice Ralphy never overrides, the same stance as D8's credential vars;
/// - no git-bash was located — pinning `SHELL` to a missing binary would break the
///   spawn entirely, so today's self-healing PowerShell fallback stands instead.
///
/// Pure over its inputs so every branch unit-tests without touching the real env.
#[cfg(windows)]
fn git_bash_shell_pin(
    existing_shell: Option<std::ffi::OsString>,
    located_git_bash: Option<PathBuf>,
) -> Option<PathBuf> {
    match existing_shell {
        Some(_) => None,
        None => located_git_bash,
    }
}

/// The one-shot builder (`init` / `triage` / `consolidate` / `diagnose`).
///
/// Identical argv and environment hygiene to [`build_cursor_command`] — the same
/// D4/D7/D17/D18 stance applies to a one-shot, which walks the same repository the
/// run path does. The only difference is the session id: a one-shot is never
/// resumed and nothing looks it up afterwards, so it gets a fresh minted id rather
/// than one the caller has to thread through.
pub(crate) fn build_cursor_init_command(
    model: Option<&str>,
    cwd: &Path,
    config_dir: &Path,
) -> Command {
    build_cursor_command(&mint_session_id(), model, cwd, config_dir)
}

#[cfg(test)]
mod tests;
