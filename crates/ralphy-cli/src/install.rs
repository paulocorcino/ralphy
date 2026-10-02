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

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Args;

mod user_path;

#[derive(Args)]
pub struct InstallArgs {
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
        Ok(added) => {
            if added.places.is_empty() {
                println!("{} is already set to be on your PATH.", dir.display());
            } else {
                println!("Added {} to your PATH in:", dir.display());
                for place in &added.places {
                    println!("  {place}");
                }
            }
            match added.reload {
                Some(cmd) => println!(
                    "Open a new terminal to use `ralphy`, or run this in this one: {cmd}"
                ),
                None => println!("Open a new terminal to use `ralphy`."),
            }
        }
        Err(e) => println!(
            "Note: could not add {} to your PATH ({e:#}). Add it by hand so `ralphy` resolves from any directory.",
            dir.display()
        ),
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
            s.to_string()
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
            "Note: `ralphy` is not on your PATH. Run `{} install` so it works by name in a new terminal.",
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
    fn a_park_that_cannot_be_freed_steps_aside_instead_of_failing() {
        // The second consecutive install over a live daemon: the daemon is
        // executing the file the previous install parked, so `.old` can be
        // neither removed nor renamed over. A fixed name failed the install
        // outright — found by doing exactly this on a running daemon.
        let dir = scratch("held");
        let dest = dir.join(binary_name());
        let held = parked_path(&dest);
        std::fs::write(&dest, b"old").expect("old");
        std::fs::write(&held, b"still running").expect("held");

        // Hold the parked file open, which is what a running image does.
        let guard = std::fs::File::open(&held).expect("open");
        let next = parked_path(&dest);
        drop(guard);

        // On a platform that lets a held file be deleted (every Unix), the plain
        // name is reused; on Windows it is not, and a distinct one is chosen.
        assert!(
            next == held || next.to_string_lossy().starts_with(&*held.to_string_lossy()),
            "the park must stay beside the binary: {next:?}"
        );
        let new = dir.join("staged");
        std::fs::write(&new, b"new").expect("new");
        replace_binary(&dest, &new).expect("a held leftover must not fail the install");
        assert_eq!(std::fs::read(&dest).expect("read"), b"new");
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
    fn a_folder_on_path_is_found_with_or_without_a_trailing_separator() {
        let sep = if cfg!(windows) { ";" } else { ":" };
        let base = std::env::temp_dir().join("ralphy-on-path");
        let other = std::env::temp_dir().join("ralphy-elsewhere");
        let with_slash = format!("{}{}", base.display(), std::path::MAIN_SEPARATOR);
        let rows: &[(String, bool)] = &[
            (format!("{}{sep}{}", other.display(), base.display()), true),
            (format!("{}{sep}{with_slash}", other.display()), true),
            (other.display().to_string(), false),
            (String::new(), false),
        ];
        for (path, want) in rows {
            assert_eq!(
                on_path(std::ffi::OsStr::new(path), &base),
                *want,
                "{path:?}"
            );
        }
    }

    #[test]
    fn an_update_adds_the_install_folder_only_when_it_runs_this_binary() {
        let root = scratch("after-update");
        let bin = root.join("bin");
        let on = root.join("on-path");
        let away = root.join("unzipped");
        for d in [&bin, &on, &away] {
            std::fs::create_dir_all(d).expect("dir");
        }
        let put = |dir: &Path| {
            let file = dir.join(binary_name());
            std::fs::write(&file, b"ralphy").expect("binary");
            std::fs::canonicalize(&file).expect("canonical")
        };
        let path = std::ffi::OsString::from(on.as_os_str());

        // The install folder holds the binary, and nothing on PATH runs it.
        let in_bin = put(&bin);
        assert_eq!(path_fix(&in_bin, &path, &bin), PathFix::Add(bin.clone()));

        // A folder on PATH runs it: nothing to do, even with the install folder
        // holding a copy of its own.
        let in_on = put(&on);
        assert_eq!(path_fix(&in_on, &path, &bin), PathFix::Nothing);

        // Neither runs it: the binary was never installed.
        let in_away = put(&away);
        assert_eq!(path_fix(&in_away, &path, &bin), PathFix::Install);

        // A link in the install folder runs the unzipped binary.
        std::fs::remove_file(bin.join(binary_name())).expect("remove");
        if symlink(&away.join(binary_name()), &bin.join(binary_name())).is_ok() {
            assert_eq!(path_fix(&in_away, &path, &bin), PathFix::Add(bin.clone()));
        }
        let _ = std::fs::remove_dir_all(&root);
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
