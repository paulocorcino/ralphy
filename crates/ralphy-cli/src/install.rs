//! `ralphy install`: drop a `ralphy` entry into a PATH directory so the binary can
//! be invoked by name from any working directory. By default it symlinks the
//! running executable (so a rebuild is picked up with no re-install); on Windows,
//! where symlinks need Developer Mode or admin, it transparently falls back to a
//! copy. `--copy` forces the copy path on any platform. When the folder is not
//! on `PATH`, it is added to the user's own `PATH` (see [`user_path`]), so a
//! user without root or administrator rights can run `ralphy` in a new terminal.
//!
//! Replacing an existing entry parks it rather than deleting it, which is what
//! makes `--force` work over a *running* daemon: Windows refuses to delete or
//! overwrite a live image but will happily rename one. `ralphy update` places its
//! download through the same primitive here, so both paths replace a binary the
//! same way (ADR-0056 §8).

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Args;

mod user_path;
pub(crate) use user_path::remove_from_user_path;

#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is one independent command-line flag, not a state"
)]
#[derive(Args)]
pub(crate) struct InstallArgs {
    /// The folder to put `ralphy` in. Default: `~/.cargo/bin` when it exists,
    /// otherwise `~/.local/bin`.
    #[arg(long)]
    dir: Option<PathBuf>,

    /// Copy the program instead of linking it. A copy works on its own but does
    /// not change when you rebuild; a link always runs the latest build.
    #[arg(long)]
    copy: bool,

    /// Replace a `ralphy` that is already there, instead of stopping with an
    /// error.
    #[arg(long)]
    force: bool,

    /// Do not add the folder to your PATH. Without this, when the folder is
    /// not on PATH, it is added to your shell start-up files (Linux, macOS) or
    /// to your user Path (Windows).
    #[arg(long)]
    no_modify_path: bool,

    /// Print where `ralphy` would go, and change nothing. The install scripts
    /// read it, so the default folder is decided only here.
    #[arg(long, hide = true)]
    print_path: bool,
}

/// The warning for a run as root or as administrator, or `None`. It never asks
/// and never stops: `host add` runs these commands over ssh with no terminal,
/// and maybe as root (ADR-0056 §12).
fn elevated_warning(elevated: bool) -> Option<&'static str> {
    elevated.then_some(
        "warning: ralphy runs as root or as administrator. Ralphy does not need this: \
         it installs for one user, and the files it writes now belong to that account. \
         Use a normal user when you can.",
    )
}

pub(crate) fn warn_if_elevated() {
    if let Some(warning) = elevated_warning(ralphy_proc_util::elevated::runs_elevated()) {
        eprintln!("{warning}");
    }
}

pub(crate) fn run(args: &InstallArgs) -> Result<()> {
    let dir = match &args.dir {
        Some(d) => d.clone(),
        None => default_bin_dir()?,
    };
    if args.print_path {
        println!("{}", dir.join(binary_name()).display());
        return Ok(());
    }
    warn_if_elevated();
    // Link to the real file, not to another link, so the installed entry survives
    // the build dir being a symlink farm (e.g. some CI layouts).
    let exe = std::env::current_exe().context("locating the running ralphy binary")?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);

    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let dest = dir.join(binary_name());

    // Re-running install onto our own location is a no-op, not an error.
    if std::fs::canonicalize(&dest).is_ok_and(|d| d == exe) {
        println!("ralphy is already installed at {}", dest.display());
        put_on_path(&dir, args.no_modify_path);
        return Ok(());
    }

    // `symlink_metadata` catches a broken/dangling symlink that `exists()` misses.
    if occupied(&dest) && !args.force {
        bail!(
            "{} already exists; re-run with --force to replace it",
            dest.display()
        );
    }

    let parked = if args.copy {
        let parked = replace_binary(&dest, &exe)?;
        println!("Copied ralphy to {}", dest.display());
        parked
    } else {
        link_over(&dest, &exe)?
    };

    if let Some(parked) = parked {
        report_parked(&parked);
    }

    put_on_path(&dir, args.no_modify_path);
    Ok(())
}

/// The binary's name on this host. Windows resolves bare names against
/// `name.exe`, so the extension is what makes the entry invokable as `ralphy`.
pub(crate) fn binary_name() -> &'static str {
    if cfg!(windows) {
        "ralphy.exe"
    } else {
        "ralphy"
    }
}

/// Symlink `exe` at `dest`, parking whatever was there. Falls back to a copy on
/// Windows, where symlinks need Developer Mode or an elevated prompt.
fn link_over(dest: &Path, exe: &Path) -> Result<Option<PathBuf>> {
    let parked = park_existing(dest)?;
    match symlink(exe, dest) {
        Ok(()) => {
            println!("Linked {} -> {}", dest.display(), exe.display());
            Ok(parked)
        }
        Err(e) if cfg!(windows) => match std::fs::copy(exe, dest) {
            Ok(_) => {
                println!(
                    "Symlink unavailable ({e}); copied ralphy to {} instead",
                    dest.display()
                );
                Ok(parked)
            }
            Err(copy_err) => {
                restore_parked(parked.as_deref(), dest);
                Err(copy_err).with_context(|| {
                    format!(
                        "symlink failed ({e}); fallback copy to {} also failed",
                        dest.display()
                    )
                })
            }
        },
        Err(e) => {
            restore_parked(parked.as_deref(), dest);
            Err(e).with_context(|| format!("symlinking {}", dest.display()))
        }
    }
}

/// Whether something is already at `dest` — including a dangling symlink, which
/// `exists()` alone misses.
fn occupied(dest: &Path) -> bool {
    dest.exists() || dest.symlink_metadata().is_ok()
}

/// Where a replaced entry goes: `ralphy.exe.old` beside itself, or the first
/// free `ralphy.exe.old.N` when that name is taken by something still running.
///
/// The second consecutive install over a live daemon reaches this: the daemon is
/// executing the file the *previous* install parked, so `.old` can be neither
/// removed nor renamed over, and a fixed name would fail the install outright.
fn parked_path(dest: &Path) -> PathBuf {
    let base = {
        let mut name = dest.file_name().unwrap_or_default().to_os_string();
        name.push(".old");
        dest.with_file_name(name)
    };
    // Free, or freeable: reuse the plain name so the directory stays tidy.
    if !base.exists() || std::fs::remove_file(&base).is_ok() {
        return base;
    }
    // Held by something still running. Step aside rather than fail the install.
    for n in 1..1_000 {
        let candidate = PathBuf::from(format!("{}.{n}", base.display()));
        if !candidate.exists() || std::fs::remove_file(&candidate).is_ok() {
            return candidate;
        }
    }
    base
}

/// Move whatever is at `dest` aside, returning where it went, or `None` when
/// there was nothing there.
///
/// Rename rather than delete: Windows refuses to delete or overwrite a running
/// executable but will happily rename one, and replacing the binary a live daemon
/// is executing is the ordinary case, not the exotic one.
pub(crate) fn park_existing(dest: &Path) -> Result<Option<PathBuf>> {
    if !occupied(dest) {
        return Ok(None);
    }
    // `parked_path` has already cleared or stepped around any leftover, so a
    // failure below is a real one.
    let parked = parked_path(dest);
    std::fs::rename(dest, &parked)
        .with_context(|| format!("moving {} aside to {}", dest.display(), parked.display()))?;
    Ok(Some(parked))
}

/// Put a parked file back. Called only when the placement that followed failed:
/// an operator with no binary is worse off than one who did not update.
pub(crate) fn restore_parked(parked: Option<&Path>, dest: &Path) {
    if let Some(parked) = parked {
        // The caller returns the error that made it restore, so this one is only
        // printed.
        if let Err(e) = std::fs::rename(parked, dest) {
            eprintln!(
                "Could not put the previous binary back from {}: {e}",
                parked.display()
            );
        }
    }
}

/// Copy `src` over `dest`, parking whatever was there first. Returns the parked
/// path, or `None` when the destination was free.
///
/// The shared primitive: `ralphy install --copy` and `ralphy update` both place a
/// binary this way, so both work over a running image.
pub(crate) fn replace_binary(dest: &Path, src: &Path) -> Result<Option<PathBuf>> {
    let parked = park_existing(dest)?;
    match std::fs::copy(src, dest) {
        Ok(_) => {
            copy_permissions(parked.as_deref(), dest);
            Ok(parked)
        }
        Err(e) => {
            restore_parked(parked.as_deref(), dest);
            Err(e).with_context(|| format!("putting the new binary at {}", dest.display()))
        }
    }
}

/// Drop the parked file, or say where it is when it cannot go — on Windows that
/// is exactly the case where a daemon is still executing it.
fn report_parked(parked: &Path) {
    if std::fs::remove_file(parked).is_err() {
        println!(
            "The previous binary is parked at {} — still in use, so it goes on the next install.",
            parked.display()
        );
    }
}

/// Carry the old binary's mode across on Unix, so a replacement does not land a
/// file nobody can execute. A no-op on Windows and when there is nothing parked.
#[cfg(unix)]
fn copy_permissions(from: Option<&Path>, to: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mode = from
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.permissions().mode())
        .unwrap_or(0o755);
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the copy is already in place, and a mode that cannot be set leaves the \
                  file as copied"
    )]
    let _ = std::fs::set_permissions(to, std::fs::Permissions::from_mode(mode | 0o111));
}

#[cfg(not(unix))]
fn copy_permissions(_from: Option<&Path>, _to: &Path) {}

/// Default install target: `~/.cargo/bin` when present (Rust users already have it
/// on PATH), else `~/.local/bin` (the XDG-conventional user bin dir).
pub(crate) fn default_bin_dir() -> Result<PathBuf> {
    let base = directories::BaseDirs::new().context("locating the home directory")?;
    let home = base.home_dir();
    let cargo_bin = home.join(".cargo").join("bin");
    if cargo_bin.is_dir() {
        return Ok(cargo_bin);
    }
    Ok(home.join(".local").join("bin"))
}

/// Make `ralphy` resolve by name in new terminals. Never fails the install: the
/// binary is in place, and the operator can still add the folder by hand.
fn put_on_path(dir: &Path, no_modify_path: bool) {
    let path = std::env::var_os("PATH").unwrap_or_default();
    if on_path(&path, dir) {
        return;
    }
    if no_modify_path {
        println!(
            "Note: {} is not on your PATH — add it so `ralphy` resolves from any directory.",
            dir.display()
        );
        return;
    }
    match user_path::add_to_user_path(dir) {
        Ok(added) => print_path_added(dir, &added),
        Err(e) => println!(
            "Note: could not add {} to your PATH ({e:#}). \
             Add it by hand so `ralphy` resolves from any directory.",
            dir.display()
        ),
    }
}

/// Tell the operator where `dir` was added to `PATH` and how to use it now.
fn print_path_added(dir: &Path, added: &user_path::Added) {
    if added.places.is_empty() {
        println!("{} is already set to be on your PATH.", dir.display());
    } else {
        println!("Added {} to your PATH in:", dir.display());
        for place in &added.places {
            println!("  {place}");
        }
    }
    match &added.reload {
        Some(cmd) => {
            writeln!(
                std::io::stdout(),
                "Open a new terminal to use `ralphy`, or run this in this one: {cmd}"
            )
            .expect("the report goes to stdout, as println! sent it");
        }
        None => println!("Open a new terminal to use `ralphy`."),
    }
}

/// Whether `dir` is one of the folders in the `PATH` value `path`. A trailing
/// separator names the same folder, and Windows compares without case.
fn on_path(path: &std::ffi::OsStr, dir: &Path) -> bool {
    let norm = |p: &Path| {
        let s = p.to_string_lossy();
        let s = s.trim_end_matches(std::path::is_separator);
        if cfg!(windows) {
            s.to_lowercase()
        } else {
            s.to_owned()
        }
    };
    let want = norm(dir);
    std::env::split_paths(path).any(|d| norm(&d) == want)
}

/// What `ralphy update` does about `PATH` once the new binary is in place.
#[derive(Debug, PartialEq, Eq)]
enum PathFix {
    /// A folder on `PATH` already runs this binary by name.
    Nothing,
    /// The install folder runs this binary, but it is not on `PATH`.
    Add(PathBuf),
    /// No install folder runs this binary: only `ralphy install` can fix it.
    Install,
}

/// `dest` is the canonical path of the binary the update replaced. An entry is
/// one of ours when it resolves to `dest`: a link to it, or `dest` itself.
fn path_fix(dest: &Path, path: &std::ffi::OsStr, default_dir: &Path) -> PathFix {
    let runs_dest = |dir: &Path| {
        std::fs::canonicalize(dir.join(binary_name())).is_ok_and(|entry| entry == dest)
    };
    if std::env::split_paths(path).any(|d| runs_dest(&d)) {
        return PathFix::Nothing;
    }
    if runs_dest(default_dir) {
        return PathFix::Add(default_dir.to_path_buf());
    }
    PathFix::Install
}

/// After `ralphy update`: an operator who installed before `ralphy install`
/// edited `PATH` gets the same repair. Never fails the update.
pub(crate) fn put_on_path_after_update(dest: &Path) {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let default_dir = match default_bin_dir() {
        Ok(dir) => dir,
        Err(e) => {
            println!("Note: could not check your PATH ({e:#}).");
            return;
        }
    };
    match path_fix(dest, &path, &default_dir) {
        PathFix::Nothing => {}
        PathFix::Add(dir) => put_on_path(&dir, false),
        PathFix::Install => println!(
            "Note: `ralphy` is not on your PATH. \
             Run `{} install` so it works by name in a new terminal.",
            dest.display()
        ),
    }
}

#[cfg(unix)]
fn symlink(src: &Path, dst: &Path) -> Result<()> {
    std::os::unix::fs::symlink(src, dst).map_err(Into::into)
}

#[cfg(windows)]
fn symlink(src: &Path, dst: &Path) -> Result<()> {
    std::os::windows::fs::symlink_file(src, dst).map_err(Into::into)
}

#[cfg(test)]
mod tests;
