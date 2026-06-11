//! `ralphy install`: drop a `ralphy` entry into a PATH directory so the binary can
//! be invoked by name from any working directory. By default it symlinks the
//! running executable (so a rebuild is picked up with no re-install); on Windows,
//! where symlinks need Developer Mode or admin, it transparently falls back to a
//! copy. `--copy` forces the copy path on any platform.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Args;

#[derive(Args)]
pub struct InstallArgs {
    /// Directory to link/copy `ralphy` into. Defaults to `~/.cargo/bin` when it
    /// exists (already on PATH for Rust toolchains), otherwise `~/.local/bin`.
    #[arg(long)]
    dir: Option<PathBuf>,

    /// Copy the binary instead of symlinking. A copy is self-contained but goes
    /// stale on rebuild; a symlink always points at the latest build.
    #[arg(long)]
    copy: bool,

    /// Replace an existing `ralphy` at the destination instead of erroring.
    #[arg(long)]
    force: bool,
}

pub fn run(args: &InstallArgs) -> Result<()> {
    // Link to the real file, not to another link, so the installed entry survives
    // the build dir being a symlink farm (e.g. some CI layouts).
    let exe = std::env::current_exe().context("locating the running ralphy binary")?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);

    let dir = match &args.dir {
        Some(d) => d.clone(),
        None => default_bin_dir()?,
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    // Windows resolves bare names against `name.exe`; keep the extension so the
    // installed entry is invokable as `ralphy`.
    let name = if cfg!(windows) {
        "ralphy.exe"
    } else {
        "ralphy"
    };
    let dest = dir.join(name);

    // Re-running install onto our own location is a no-op, not an error.
    if std::fs::canonicalize(&dest).is_ok_and(|d| d == exe) {
        println!("ralphy is already installed at {}", dest.display());
        return warn_if_off_path(&dir);
    }

    // `symlink_metadata` catches a broken/dangling symlink that `exists()` misses.
    if dest.exists() || dest.symlink_metadata().is_ok() {
        if !args.force {
            bail!(
                "{} already exists; re-run with --force to replace it",
                dest.display()
            );
        }
        std::fs::remove_file(&dest)
            .with_context(|| format!("removing existing {}", dest.display()))?;
    }

    if args.copy {
        std::fs::copy(&exe, &dest).with_context(|| format!("copying to {}", dest.display()))?;
        println!("Copied ralphy to {}", dest.display());
    } else {
        match symlink(&exe, &dest) {
            Ok(()) => println!("Linked {} -> {}", dest.display(), exe.display()),
            // Windows symlinks require Developer Mode or an elevated prompt; rather
            // than fail, fall back to a copy so `install` works out of the box.
            Err(e) if cfg!(windows) => {
                std::fs::copy(&exe, &dest).with_context(|| {
                    format!(
                        "symlink failed ({e}); fallback copy to {} also failed",
                        dest.display()
                    )
                })?;
                println!(
                    "Symlink unavailable ({e}); copied ralphy to {} instead",
                    dest.display()
                );
            }
            Err(e) => {
                return Err(e).with_context(|| format!("symlinking {}", dest.display()));
            }
        }
    }

    warn_if_off_path(&dir)
}

/// Default install target: `~/.cargo/bin` when present (Rust users already have it
/// on PATH), else `~/.local/bin` (the XDG-conventional user bin dir).
fn default_bin_dir() -> Result<PathBuf> {
    let base = directories::BaseDirs::new().context("locating the home directory")?;
    let home = base.home_dir();
    let cargo_bin = home.join(".cargo").join("bin");
    if cargo_bin.is_dir() {
        return Ok(cargo_bin);
    }
    Ok(home.join(".local").join("bin"))
}

/// Print a hint when the install dir isn't on PATH — the link is useless until the
/// shell can find it. Never fails the install; it's advisory only.
fn warn_if_off_path(dir: &Path) -> Result<()> {
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d == dir))
        .unwrap_or(false);
    if !on_path {
        println!(
            "Note: {} is not on your PATH — add it so `ralphy` resolves from any directory.",
            dir.display()
        );
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(src: &Path, dst: &Path) -> Result<()> {
    std::os::unix::fs::symlink(src, dst).map_err(Into::into)
}

#[cfg(windows)]
fn symlink(src: &Path, dst: &Path) -> Result<()> {
    std::os::windows::fs::symlink_file(src, dst).map_err(Into::into)
}
