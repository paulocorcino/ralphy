//! `ralphy install`: drop a `ralphy` entry into a PATH directory so the binary can
//! be invoked by name from any working directory. By default it symlinks the
//! running executable (so a rebuild is picked up with no re-install); on Windows,
//! where symlinks need Developer Mode or admin, it transparently falls back to a
//! copy. `--copy` forces the copy path on any platform.
//!
//! Replacing an existing entry parks it rather than deleting it, which is what
//! makes `--force` work over a *running* daemon: Windows refuses to delete or
//! overwrite a live image but will happily rename one. `ralphy update` places its
//! download through the same primitive here, so both paths replace a binary the
//! same way (ADR-0056 §8).

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

    let dest = dir.join(binary_name());

    // Re-running install onto our own location is a no-op, not an error.
    if std::fs::canonicalize(&dest).is_ok_and(|d| d == exe) {
        println!("ralphy is already installed at {}", dest.display());
        return warn_if_off_path(&dir);
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

    warn_if_off_path(&dir)
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

/// Where a replaced entry goes: `ralphy.exe.old` beside itself.
fn parked_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    dest.with_file_name(name)
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
    let parked = parked_path(dest);
    // A leftover from a previous replacement, if the running image had still
    // been holding it. Best-effort: if it is *still* held, the rename below
    // fails and says so.
    let _ = std::fs::remove_file(&parked);
    std::fs::rename(dest, &parked)
        .with_context(|| format!("moving {} aside to {}", dest.display(), parked.display()))?;
    Ok(Some(parked))
}

/// Put a parked file back. Called only when the placement that followed failed:
/// an operator with no binary is worse off than one who did not update.
pub(crate) fn restore_parked(parked: Option<&Path>, dest: &Path) {
    if let Some(parked) = parked {
        let _ = std::fs::rename(parked, dest);
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
    let _ = std::fs::set_permissions(to, std::fs::Permissions::from_mode(mode | 0o111));
}

#[cfg(not(unix))]
fn copy_permissions(_from: Option<&Path>, _to: &Path) {}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ralphy-install-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[test]
    fn replacing_parks_the_old_binary_rather_than_deleting_it() {
        // The whole point: Windows cannot delete a running image, so a
        // replacement that started with `remove_file` could never work over a
        // live daemon — which is the ordinary case.
        let dir = scratch("replace");
        let dest = dir.join(binary_name());
        let new = dir.join("staged");
        std::fs::write(&dest, b"old").expect("old");
        std::fs::write(&new, b"new").expect("new");

        let parked = replace_binary(&dest, &new)
            .expect("replace")
            .expect("an existing binary is parked, not deleted");
        assert_eq!(std::fs::read(&dest).expect("read"), b"new");
        assert_eq!(std::fs::read(&parked).expect("read parked"), b"old");
        assert_eq!(parked.file_name().and_then(|n| n.to_str()), {
            let mut expected = binary_name().to_string();
            expected.push_str(".old");
            Some(expected).as_deref()
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_first_install_has_nothing_to_park() {
        let dir = scratch("fresh");
        let dest = dir.join(binary_name());
        let new = dir.join("staged");
        std::fs::write(&new, b"new").expect("new");
        assert_eq!(replace_binary(&dest, &new).expect("replace"), None);
        assert_eq!(std::fs::read(&dest).expect("read"), b"new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_put_restores_the_original() {
        let dir = scratch("restore");
        let dest = dir.join(binary_name());
        std::fs::write(&dest, b"old").expect("old");
        // A source that does not exist: the copy fails after the park.
        let err = replace_binary(&dest, &dir.join("missing")).expect_err("copy must fail");
        assert!(err.to_string().contains("putting the new binary"), "{err}");
        assert_eq!(
            std::fs::read(&dest).expect("read"),
            b"old",
            "a failed replacement must never leave the operator without a binary"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_leftover_park_from_last_time_does_not_block_this_one() {
        let dir = scratch("leftover");
        let dest = dir.join(binary_name());
        std::fs::write(&dest, b"old").expect("old");
        std::fs::write(parked_path(&dest), b"older still").expect("leftover");
        let new = dir.join("staged");
        std::fs::write(&new, b"new").expect("new");

        let parked = replace_binary(&dest, &new)
            .expect("replace")
            .expect("parked");
        assert_eq!(std::fs::read(&dest).expect("read"), b"new");
        assert_eq!(
            std::fs::read(&parked).expect("read parked"),
            b"old",
            "the park holds the binary just replaced, not the one before it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dangling_symlink_counts_as_occupied() {
        // `exists()` follows the link and answers false; the entry is still in
        // the way of a new one.
        let dir = scratch("dangling");
        let dest = dir.join(binary_name());
        if symlink(&dir.join("nowhere"), &dest).is_err() {
            // Windows without Developer Mode: nothing to assert.
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        assert!(
            !dest.exists(),
            "the target is missing, so exists() is false"
        );
        assert!(occupied(&dest), "but something is there");
        let parked = park_existing(&dest).expect("park").expect("parked");
        assert!(!occupied(&dest), "the way is clear now");
        assert!(parked.symlink_metadata().is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
