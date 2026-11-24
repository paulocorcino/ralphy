//! `ralphyw`: the Windows launcher the daemon's Startup shortcut opens
//! (ADR-0032, amendment of 2026-11-23). A console program opened by a
//! shortcut gets a window before its own code runs; this one is built for the
//! GUI subsystem, so it has none. It starts `ralphy.exe` from its own folder
//! with the arguments after `--log <file>`, no console, outside its job, and
//! the output appended to `<file>`, then exits.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

/// The log file and the arguments for `ralphy.exe`, from `--log <file> <args…>`.
fn parse(mut args: impl Iterator<Item = OsString>) -> Option<(PathBuf, Vec<OsString>)> {
    if args.next()? != "--log" {
        return None;
    }
    let log = PathBuf::from(args.next()?);
    Some((log, args.collect()))
}

#[cfg(windows)]
fn launch(log: &std::path::Path, args: &[OsString]) -> std::io::Result<()> {
    use std::fs::OpenOptions;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    let exe = std::env::current_exe()?.with_file_name("ralphy.exe");
    let out = OpenOptions::new().create(true).append(true).open(log)?;
    let spawn = |flags: u32| {
        Command::new(&exe)
            .args(args)
            .stdin(Stdio::null())
            .stdout(out.try_clone()?)
            .stderr(out.try_clone()?)
            .creation_flags(flags)
            .spawn()
    };
    // The daemon is not this launcher's child to wait on: it is dropped.
    ralphy_proc_util::detach::spawn_detached(spawn).map(drop)
}

#[cfg(not(windows))]
fn launch(_log: &std::path::Path, _args: &[OsString]) -> std::io::Result<()> {
    Err(std::io::Error::other("ralphyw runs only on Windows"))
}

fn main() -> ExitCode {
    // A GUI program has no console to print to: the exit code is its report.
    let Some((log, args)) = parse(std::env::args_os().skip(1)) else {
        return ExitCode::from(2);
    };
    match launch(&log, &args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> impl Iterator<Item = OsString> {
        list.iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn the_log_comes_first_and_the_rest_goes_to_ralphy() {
        let (log, rest) = parse(args(&["--log", r"C:\a b\daemon.log", "daemon"])).expect("parsed");
        assert_eq!(log, PathBuf::from(r"C:\a b\daemon.log"));
        assert_eq!(rest, vec![OsString::from("daemon")]);
    }

    #[test]
    fn without_a_log_it_starts_nothing() {
        assert!(parse(args(&["daemon"])).is_none());
        assert!(
            parse(args(&["--port", "8080", "daemon"])).is_none(),
            "the log comes first"
        );
        assert!(parse(args(&["--log"])).is_none());
    }
}
