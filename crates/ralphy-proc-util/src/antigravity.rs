//! The Antigravity CLI (`agy`) facts that the adapter and the daemon share: where
//! the binary is, the root Ralphy owns for it, and the hook command line it runs.
//! They live here because the daemon may not import the adapter (ADR-0032 §10,
//! ADR-0042 D19); the decisions are ADR-0086 D8, D9 and D17.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::hook_form::{hook_for_version, HookCommand, EXEC_FORM_FROM};

/// The one name the CLI installs. The IDE launcher `antigravity.cmd` is another program.
const NAME: &str = "agy";

/// The folder of the owned root inside the Ralphy folder.
const OWNED_ROOT_DIR: &str = "antigravity-root";

/// The characters `cmd /c` reads in an unquoted program path, with the space.
const CMD_SPECIAL: &str = " &|<>^%()\"";

/// Locate `agy` against the real environment; `None` when it is not installed.
pub fn locate_agy() -> Option<PathBuf> {
    locate_agy_with(
        std::env::var_os("PATH"),
        std::env::var_os("PATHEXT"),
        crate::home_dir(),
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
    )
}

/// [`locate_agy`] over its inputs: the install folder first, because neither
/// install folder is on the `PATH` of a running process, then `PATH`.
pub fn locate_agy_with(
    path_var: Option<OsString>,
    pathext: Option<OsString>,
    home: Option<PathBuf>,
    localappdata: Option<PathBuf>,
) -> Option<PathBuf> {
    install_candidate(home, localappdata)
        .filter(|cand| crate::is_executable_file(cand))
        .or_else(|| crate::find_program(NAME, path_var, pathext))
}

/// `%LOCALAPPDATA%\agy\bin\agy.exe` on Windows, `~/.local/bin/agy` elsewhere.
fn install_candidate(home: Option<PathBuf>, localappdata: Option<PathBuf>) -> Option<PathBuf> {
    if cfg!(windows) {
        Some(
            localappdata?
                .join(NAME)
                .join("bin")
                .join(format!("{NAME}.exe")),
        )
    } else {
        Some(home?.join(".local").join("bin").join(NAME))
    }
}

/// The root every `agy` run gets as `--gemini_dir`: `antigravity-root` in the
/// Ralphy folder, as an absolute path, because `agy` refuses a relative one.
pub fn owned_root() -> Result<PathBuf> {
    owned_root_with(std::env::var_os("RALPHY_DAEMON_DIR"), crate::home_dir())
}

/// [`owned_root`] over its inputs.
fn owned_root_with(daemon_dir: Option<OsString>, home: Option<PathBuf>) -> Result<PathBuf> {
    let root = ralphy_dir(daemon_dir, home)?.join(OWNED_ROOT_DIR);
    std::path::absolute(&root)
        .with_context(|| format!("could not make {} an absolute path", root.display()))
}

/// The Ralphy folder by the daemon's rule: `RALPHY_DAEMON_DIR`, else `~/.ralphy`.
fn ralphy_dir(daemon_dir: Option<OsString>, home: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(dir) = daemon_dir {
        return Ok(PathBuf::from(dir));
    }
    let home = home.context("could not find the home folder for the Antigravity root")?;
    Ok(home.join(".ralphy"))
}

/// The arguments of `ralphy` that the hooks file runs on each status event.
pub const STATUS_HOOK_ARGS: [&str; 3] = ["hook", "status", "antigravity"];

/// The command `ralphy doctor` runs to check the `agy` hook: the program and
/// the arguments of the hooks file, run without a shell.
pub fn check_hook(exe: &Path) -> HookCommand {
    let (major, minor, patch) = EXEC_FORM_FROM;
    let version = format!("{major}.{minor}.{patch}");
    hook_for_version(Some(&version), exe, &STATUS_HOOK_ARGS)
}

/// The shell `agy` runs a hook command with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HookShell {
    /// `cmd /c` on Windows, given the line with Go's argument escaping.
    Cmd,
    /// `sh -c` on Unix.
    Sh,
}

/// The hook command line `agy` runs on this platform: `exe` then `args`.
/// An error names a Windows path that `cmd` cannot run unquoted.
pub fn hook_command(exe: &Path, args: &[&str]) -> Result<String> {
    let shell = if cfg!(windows) {
        HookShell::Cmd
    } else {
        HookShell::Sh
    };
    hook_command_for(exe, args, shell, short_path)
}

/// [`hook_command`] for one shell, with the lookup of a Windows short path.
fn hook_command_for(
    exe: &Path,
    args: &[&str],
    shell: HookShell,
    short: impl Fn(&Path) -> Option<PathBuf>,
) -> Result<String> {
    if let Some(arg) = args.iter().find(|a| !is_plain_word(a)) {
        bail!("the hook argument \"{arg}\" is not a plain word");
    }
    let exe = crate::exe::plain_path(exe.to_path_buf());
    let program = match shell {
        HookShell::Cmd => cmd_program(&exe, short)?,
        HookShell::Sh => sh_quote(&path_text(&exe)?),
    };
    Ok(std::iter::once(program)
        .chain(args.iter().map(|a| (*a).to_owned()))
        .collect::<Vec<_>>()
        .join(" "))
}

/// The program path for `cmd /c`, never quoted: a quoted path reaches `cmd` as
/// `\"…\"` and the hook fails. A path with a space or a `cmd` character takes
/// its short (8.3) form; with no usable short form it is an error.
fn cmd_program(exe: &Path, short: impl Fn(&Path) -> Option<PathBuf>) -> Result<String> {
    let text = path_text(exe)?;
    if cmd_safe(&text) {
        return Ok(text);
    }
    let short_text = short(exe).and_then(|p| p.to_str().map(str::to_owned));
    match short_text {
        Some(short_text) if cmd_safe(&short_text) => Ok(short_text),
        _ => bail!(
            "the program path {text} has a space or a character that cmd reads, \
             and it has no short form without one, so Antigravity cannot run its hooks"
        ),
    }
}

fn cmd_safe(text: &str) -> bool {
    !text.chars().any(|c| CMD_SPECIAL.contains(c))
}

fn path_text(exe: &Path) -> Result<String> {
    exe.to_str()
        .map(str::to_owned)
        .with_context(|| format!("the program path {} is not valid text", exe.display()))
}

/// POSIX single quotes for `sh -c`; a `'` inside closes, escapes and reopens.
fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn is_plain_word(arg: &str) -> bool {
    !arg.is_empty() && arg.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The short (8.3) form of an existing path, from `GetShortPathNameW`.
#[cfg(windows)]
#[expect(unsafe_code, reason = "FFI: GetShortPathNameW")]
fn short_path(path: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: `wide` ends in a null; a null buffer of size 0 asks for the size.
    let needed = unsafe { GetShortPathNameW(wide.as_ptr(), std::ptr::null_mut(), 0) };
    let mut buf = vec![0u16; usize::try_from(needed).ok().filter(|n| *n > 0)?];
    // SAFETY: `buf` holds `needed` units, the size the call above asked for.
    let written = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), needed) };
    if written == 0 || written >= needed {
        return None;
    }
    buf.truncate(usize::try_from(written).ok()?);
    Some(PathBuf::from(OsString::from_wide(&buf)))
}

#[cfg(not(windows))]
fn short_path(_path: &Path) -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests;
