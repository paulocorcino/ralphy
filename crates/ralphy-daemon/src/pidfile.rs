//! `daemon.pid` — which process is serving, what it is running, and how it was
//! invoked, so `ralphy daemon restart` can end *that* daemon and bring back the
//! same one (ADR-0056 §8).
//!
//! Three things are recorded, and each earns its place. The **pid** names the
//! process. The **exe** is what makes the pid trustworthy: the file outlives a
//! crash, a `kill -9` and a reboot, after which the number is very likely alive
//! again as something else, so a reader that checked only liveness would end an
//! unrelated process tree. The **argv** is what makes the restart faithful: a
//! daemon started with `--port 8080` that came back on the default would be a
//! different daemon at a URL nobody has open.
//!
//! The format is one `key=value` per line, and it is read tolerantly: a file
//! written by an older build (a bare pid, then arguments) still yields its pid,
//! but carries no exe — and a caller that cannot prove identity must refuse to
//! kill rather than guess.
//!
//! Pure, sync and path-explicit like every other store module: tests pass a temp
//! path and never mutate the process-global env.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The `daemon.pid` path inside `dir`.
pub fn pid_path_in(dir: &Path) -> PathBuf {
    dir.join("daemon.pid")
}

/// The production path of `daemon.pid`, alongside the rest of the daemon store.
pub fn pid_path() -> Result<PathBuf> {
    Ok(pid_path_in(&crate::auth::store_dir()?))
}

/// Record this process as the serving daemon: its pid, the program it is
/// running, and the arguments it was given.
pub fn write_in(dir: &Path, pid: u32, exe: &Path, args: &[String]) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = pid_path_in(dir);
    let mut body = format!("pid={pid}\nexe={}\n", exe.display());
    for arg in args {
        body.push_str("arg=");
        body.push_str(arg);
        body.push('\n');
    }
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))
}

/// Forget the recorded daemon. Absent is the same as removed — an already-clean
/// store is not an error.
pub fn clear_in(dir: &Path) {
    let path = pid_path_in(dir);
    if let Err(e) = std::fs::remove_file(&path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = %path.display(), error = %e, "could not clear the daemon pid file");
        }
    }
}

fn read_lines(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(pid_path_in(dir))
        .map(|text| text.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// The pid a daemon last recorded, or `None` when there is no readable one.
/// Nothing here decides whether it is alive or whether it is ours.
pub fn read_in(dir: &Path) -> Option<u32> {
    let lines = read_lines(dir);
    let first = lines.first()?;
    // A file from an older build opens with a bare number.
    first
        .strip_prefix("pid=")
        .unwrap_or(first)
        .trim()
        .parse()
        .ok()
}

/// The program that daemon was running, when the file records one. `None` for a
/// file written by an older build — which a caller must read as "identity
/// unknown", never as a match.
pub fn read_exe_in(dir: &Path) -> Option<PathBuf> {
    read_lines(dir)
        .iter()
        .find_map(|line| line.strip_prefix("exe=").map(PathBuf::from))
}

/// The arguments that daemon was given. Empty when the file records none.
pub fn read_args_in(dir: &Path) -> Vec<String> {
    let lines = read_lines(dir);
    if read_in(dir).is_none() {
        // The first line is not a pid: nothing in this file is trustworthy.
        return Vec::new();
    }
    if lines.iter().any(|l| l.starts_with("arg=")) {
        return lines
            .iter()
            .filter_map(|l| l.strip_prefix("arg=").map(str::to_string))
            .collect();
    }
    // Older build: a bare pid, then one argument per line.
    lines
        .iter()
        .skip(1)
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ralphy-pidfile-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_recorded_daemon_reads_back_whole() {
        let dir = scratch("roundtrip");
        let args = vec![
            "daemon".to_string(),
            "--port".to_string(),
            "8080".to_string(),
        ];
        write_in(&dir, 4242, Path::new("/usr/local/bin/ralphy"), &args).expect("write");
        assert_eq!(read_in(&dir), Some(4242));
        assert_eq!(
            read_exe_in(&dir).as_deref(),
            Some(Path::new("/usr/local/bin/ralphy"))
        );
        assert_eq!(
            read_args_in(&dir),
            args,
            "a restart must bring back the same daemon, not the default one"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_argument_with_whitespace_or_an_empty_one_round_trips() {
        // `key=value` per line: the value is taken verbatim, so an argument
        // that is blank or padded survives — the newline-delimited-and-trimmed
        // format this replaced silently dropped both.
        let dir = scratch("verbatim");
        let args = vec![
            "daemon".to_string(),
            "--allowed-host".to_string(),
            "  spaced  ".to_string(),
            String::new(),
        ];
        write_in(&dir, 7, Path::new("/bin/ralphy"), &args).expect("write");
        assert_eq!(read_args_in(&dir), args);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_from_an_older_build_yields_its_pid_but_no_identity() {
        // The upgrade path: a daemon started by the previous build wrote a bare
        // pid and bare arguments. Its pid is still readable, but nothing in it
        // proves which program is running — so a caller must not kill on it.
        let dir = scratch("legacy");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(pid_path_in(&dir), "45236\ndaemon\n--port\n7999\n").expect("write");
        assert_eq!(read_in(&dir), Some(45236));
        assert_eq!(read_exe_in(&dir), None, "identity unknown, not matched");
        assert_eq!(
            read_args_in(&dir),
            vec![
                "daemon".to_string(),
                "--port".to_string(),
                "7999".to_string()
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_absent_or_unreadable_file_names_nobody() {
        let dir = scratch("absent");
        assert_eq!(read_in(&dir), None);
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(pid_path_in(&dir), "not a pid\narg=--port\n").expect("write");
        assert_eq!(read_in(&dir), None, "a corrupt file names nobody");
        assert_eq!(read_exe_in(&dir), None);
        assert!(
            read_args_in(&dir).is_empty(),
            "and its remaining lines are not an invocation to replay"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clearing_is_idempotent() {
        let dir = scratch("clear");
        write_in(&dir, 7, Path::new("/bin/ralphy"), &[]).expect("write");
        clear_in(&dir);
        clear_in(&dir);
        assert_eq!(read_in(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
