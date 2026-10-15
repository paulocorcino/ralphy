//! `ralphy uninstall`: undo `ralphy install` and `ralphy daemon install` on this
//! computer (ADR-0056 §12). It stops the daemon and removes its autostart,
//! removes the installed `ralphy`, and takes out the `PATH` change. It keeps
//! the store unless `--purge` is given, and it never touches a project's
//! `.ralphy/` folder or a GitHub label.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Args;

use crate::install::binary_name;

#[derive(Args)]
pub(crate) struct UninstallArgs {
    /// Also delete Ralphy's own folder (`~/.ralphy`, or `RALPHY_DAEMON_DIR`):
    /// the daemon settings, its access token, the peers and the logs. It
    /// shows the folder and its size, and asks first.
    #[arg(long)]
    purge: bool,

    /// Do not ask before `--purge` deletes the folder.
    #[arg(long)]
    yes: bool,

    /// The folder `ralphy install` put `ralphy` in, when it was not the
    /// default (`~/.cargo/bin` when it exists, otherwise `~/.local/bin`).
    #[arg(long)]
    dir: Option<PathBuf>,
}

pub(crate) fn run(args: &UninstallArgs) -> Result<()> {
    let store = ralphy_daemon::auth::store_dir()?;
    stop_daemon(&store)?;
    let dir = match &args.dir {
        Some(dir) => dir.clone(),
        None => crate::install::default_bin_dir()?,
    };
    for entry in [
        dir.join(binary_name()),
        store.join("bin").join(binary_name()),
    ] {
        remove_binary(&entry)?;
    }
    remove_from_path(&dir);
    keep_or_purge(&store, args.purge, args.yes)?;
    println!("Project `.ralphy/` folders and GitHub labels were not changed.");
    Ok(())
}

fn keep_or_purge(store: &Path, purge_it: bool, yes: bool) -> Result<()> {
    if purge_it {
        return purge(store, yes);
    }
    println!(
        "Kept Ralphy's folder {}. Delete it to remove the daemon settings and logs too.",
        store.display()
    );
    Ok(())
}

/// Remove the autostart, then end the daemon, so nothing starts it again.
fn stop_daemon(store: &Path) -> Result<()> {
    match ralphy_daemon::autostart::uninstall() {
        Ok(()) => println!("Removed the daemon autostart."),
        Err(e) => eprintln!("warning: could not remove the daemon autostart: {e:#}"),
    }
    stop_unit();
    if crate::daemon::restart::stop(store)? {
        println!("Stopped the daemon.");
    }
    Ok(())
}

/// `disable` does not stop a running unit, and a kill would look like a crash
/// to its `Restart=on-failure`, so systemd stops it.
#[cfg(all(unix, not(target_os = "macos")))]
fn stop_unit() {
    let unit = ralphy_daemon::autostart::UNIT_NAME;
    match std::process::Command::new("systemctl")
        .args(["--user", "stop", unit])
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => tracing::debug!(%status, "systemctl --user stop found no running unit"),
        Err(e) => tracing::debug!(error = %e, "systemctl is not available"),
    }
}

/// `launchctl bootout` in the autostart removal already stopped the agent.
#[cfg(not(all(unix, not(target_os = "macos"))))]
fn stop_unit() {}

/// Remove one installed `ralphy` and what `ralphy install` parked beside it.
fn remove_binary(entry: &Path) -> Result<()> {
    let mut removed = false;
    for file in with_parked(entry) {
        if file.symlink_metadata().is_err() {
            continue;
        }
        remove_file(&file)?;
        removed = true;
    }
    if removed {
        println!("Removed {}.", entry.display());
    }
    Ok(())
}

/// `entry`, then `entry.old` and `entry.old.N`, the names `install` parks a
/// replaced binary under.
fn with_parked(entry: &Path) -> Vec<PathBuf> {
    let mut files = vec![entry.to_path_buf()];
    let Some(name) = entry.file_name().and_then(|n| n.to_str()) else {
        return files;
    };
    let Some(dir) = entry.parent() else {
        return files;
    };
    let Ok(read) = std::fs::read_dir(dir) else {
        return files;
    };
    let mut others: Vec<PathBuf> = read
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| is_parked_name(n, name))
        })
        .collect();
    others.sort();
    files.extend(others);
    files
}

/// A name `install` parks `entry` under. The rule is the one `daemon restart`
/// reads a parked image with.
fn is_parked_name(name: &str, entry: &str) -> bool {
    name != entry && crate::daemon::restart::unparked(name) == entry
}

fn remove_file(file: &Path) -> Result<()> {
    match std::fs::remove_file(file) {
        Ok(()) => Ok(()),
        Err(e) if running_image(&e) => remove_running(file),
        Err(e) => Err(e).with_context(|| format!("removing {}", file.display())),
    }
}

/// Windows refuses to delete a file a process runs from: this program, or a
/// parked image a daemon still runs.
#[cfg(windows)]
fn running_image(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied
}

#[cfg(not(windows))]
fn running_image(_error: &std::io::Error) -> bool {
    false
}

/// A running image can be renamed but not deleted. It moves to the temp
/// folder, out of the folders this command removes, and a detached `cmd`
/// deletes it once this program has ended.
///
/// Measured on Windows 11 (build 26300) on 2026-10-15 with a running copy:
/// `remove_file` fails with access denied, the rename to `%TEMP%` succeeds,
/// and one delayed `del` that runs before the process ends fails too. So `cmd`
/// tries again every second, for up to 30 s; the file was gone about one
/// second after the process ended.
#[cfg(windows)]
fn remove_running(file: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;

    let aside = std::env::temp_dir().join(format!(
        "ralphy-removed-{}-{}.exe",
        std::process::id(),
        file.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("ralphy")
    ));
    // The temp folder may be on another drive, where a rename fails; the file
    // is then deleted where it is.
    let target = match std::fs::rename(file, &aside) {
        Ok(()) => aside,
        Err(e) => {
            tracing::debug!(error = %e, "the running image stays in its folder");
            file.to_path_buf()
        }
    };
    ralphy_proc_util::keep_std_handles_from_children();
    let target = target.display();
    std::process::Command::new("cmd")
        .raw_arg(format!(
            "/d /c for /l %i in (1,1,30) do (if exist \"{target}\" \
             (ping -n 2 127.0.0.1 >nul & del /f /q \"{target}\" 2>nul))"
        ))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn()
        .with_context(|| format!("starting the removal of {target}"))?;
    Ok(())
}

#[cfg(not(windows))]
fn remove_running(file: &Path) -> Result<()> {
    bail!("{} is in use and cannot be removed", file.display())
}

fn remove_from_path(dir: &Path) {
    match crate::install::remove_from_user_path(dir) {
        Ok(places) if places.is_empty() => {}
        Ok(places) => {
            println!("Took ralphy off your PATH in:");
            for place in places {
                println!("  {place}");
            }
        }
        Err(e) => eprintln!(
            "warning: could not take {} off your PATH ({e:#}); remove it by hand",
            dir.display()
        ),
    }
}

/// Files only the daemon writes in its store. A folder with none of them is
/// not Ralphy's folder, whatever `RALPHY_DAEMON_DIR` says.
const STORE_MARKERS: [&str; 5] = [
    "daemon.toml",
    "daemon-token",
    "repos.toml",
    "daemon.log",
    "peers",
];

/// A path as Windows compares it: without case. Other systems compare it as
/// it is.
fn same_path_key(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = text.trim_end_matches(std::path::is_separator);
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text.to_string()
    }
}

/// Whether `inner` is `outer` or a folder inside it, compared component by
/// component.
fn is_within(inner: &Path, outer: &Path) -> bool {
    let inner = same_path_key(inner);
    let outer = same_path_key(outer);
    inner == outer
        || inner
            .strip_prefix(&outer)
            .is_some_and(|rest| rest.starts_with(std::path::is_separator))
}

/// Why `store` must not be deleted as Ralphy's folder, or `None`. Paths are
/// canonical. A `RALPHY_DAEMON_DIR` set by mistake could name a root, the home
/// folder or a folder above it, a project, or a project's own `.ralphy/`,
/// which holds that project's runs. The default store is accepted even when
/// the home folder is a git repository.
fn purge_refusal(
    store: &Path,
    home: Option<&Path>,
    default_store: Option<&Path>,
) -> Option<&'static str> {
    let Some(parent) = store.parent() else {
        return Some("it is the root of a drive");
    };
    if !store.is_absolute() || parent.as_os_str().is_empty() {
        return Some("it is a relative path");
    }
    if home.is_some_and(|h| is_within(h, store)) {
        return Some("it is the home folder or a folder above it");
    }
    if store.join(".git").exists() {
        return Some("it is a project");
    }
    let is_default = default_store.is_some_and(|d| same_path_key(d) == same_path_key(store));
    if !is_default && parent.join(".git").exists() {
        return Some("it is the `.ralphy` folder of a project");
    }
    if !STORE_MARKERS.iter().any(|m| store.join(m).exists()) {
        return Some("it holds none of the files the daemon writes");
    }
    None
}

fn purge(store: &Path, yes: bool) -> Result<()> {
    if !store.exists() {
        println!("Ralphy's folder {} does not exist.", store.display());
        return Ok(());
    }
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let home = directories::BaseDirs::new().map(|b| canonical(b.home_dir()));
    let default_store = home.as_ref().map(|h| canonical(&h.join(".ralphy")));
    let refusal = purge_refusal(&canonical(store), home.as_deref(), default_store.as_deref());
    if let Some(why) = refusal {
        bail!("will not delete {}: {why}", store.display());
    }
    println!(
        "This deletes {} ({}): the daemon settings, its access token, the peers and the logs.",
        store.display(),
        readable_size(folder_size(store))
    );
    if !yes && !confirmed()? {
        println!("Kept {}.", store.display());
        return Ok(());
    }
    std::fs::remove_dir_all(store).with_context(|| format!("deleting {}", store.display()))?;
    println!("Deleted {}.", store.display());
    Ok(())
}

fn confirmed() -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("there is no terminal to ask in: run `ralphy uninstall --purge --yes` to delete it");
    }
    print!("Delete it? [y/N] ");
    std::io::stdout().flush().context("writing the question")?;
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("reading the answer")?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes"))
}

/// The bytes of every file under `dir`. A file that cannot be read counts as
/// zero: the number only tells the operator what goes.
fn folder_size(dir: &Path) -> u64 {
    let Ok(read) = std::fs::read_dir(dir) else {
        return 0;
    };
    read.filter_map(Result::ok)
        .map(|entry| match entry.file_type() {
            Ok(t) if t.is_dir() => folder_size(&entry.path()),
            Ok(t) if t.is_file() => entry.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

fn readable_size(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if bytes < MB {
        format!("{} KB", bytes.div_ceil(1024))
    } else {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    }
}

#[cfg(test)]
mod tests;
