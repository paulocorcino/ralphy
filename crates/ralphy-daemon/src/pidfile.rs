//! `daemon.pid` — which process is serving and how it was invoked, so
//! `ralphy daemon restart` can end it and bring back *the same daemon*
//! (ADR-0056 §8).
//!
//! The invocation matters: a daemon started with `--port 8080` that came back on
//! the default port would be a different daemon at a URL nobody has open. So the
//! file is the pid on the first line and the arguments, one per line, after it.
//!
//! Pure, sync and path-explicit like every other store module: tests pass a temp
//! path and never mutate the process-global env. The file is advisory, never a
//! lock — a stale one names a pid that is simply not alive, and the reader
//! checks liveness rather than trusting the file.

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

/// Record this process as the serving daemon, with the arguments it was given.
pub fn write_in(dir: &Path, pid: u32, args: &[String]) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = pid_path_in(dir);
    let mut body = pid.to_string();
    for arg in args {
        body.push('\n');
        body.push_str(arg);
    }
    body.push('\n');
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))
}

/// Forget the recorded pid. Absent is the same as removed — an already-clean
/// store is not an error.
pub fn clear_in(dir: &Path) {
    let path = pid_path_in(dir);
    if let Err(e) = std::fs::remove_file(&path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = %path.display(), error = %e, "could not clear the daemon pid file");
        }
    }
}

/// The pid a daemon last recorded, or `None` when there is no readable one.
/// Nothing here decides whether it is alive; that is the caller's syscall.
pub fn read_in(dir: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(pid_path_in(dir)).ok()?;
    text.lines().next()?.trim().parse().ok()
}

/// The arguments that daemon was given. Empty when the file records none — an
/// older file, or a daemon started with no flags at all.
pub fn read_args_in(dir: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(pid_path_in(dir)) else {
        return Vec::new();
    };
    if text
        .lines()
        .next()
        .and_then(|l| l.trim().parse::<u32>().ok())
        .is_none()
    {
        // A file whose first line is not a pid records nothing we can trust.
        return Vec::new();
    }
    text.lines()
        .skip(1)
        .map(str::trim)
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
    fn a_recorded_pid_and_its_invocation_read_back() {
        let dir = scratch("roundtrip");
        let args = vec![
            "daemon".to_string(),
            "--port".to_string(),
            "8080".to_string(),
        ];
        write_in(&dir, 4242, &args).expect("write");
        assert_eq!(read_in(&dir), Some(4242));
        assert_eq!(
            read_args_in(&dir),
            args,
            "a restart must bring back the same daemon, not the default one"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_daemon_with_no_flags_records_just_the_verb() {
        let dir = scratch("bare");
        write_in(&dir, 7, &["daemon".to_string()]).expect("write");
        assert_eq!(read_args_in(&dir), vec!["daemon".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_absent_or_unreadable_file_is_no_pid() {
        let dir = scratch("absent");
        assert_eq!(read_in(&dir), None);
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(pid_path_in(&dir), "not a pid\n--port\n8080\n").expect("write");
        assert_eq!(read_in(&dir), None, "a corrupt file names nobody");
        assert!(
            read_args_in(&dir).is_empty(),
            "and its remaining lines are not an invocation to replay"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clearing_is_idempotent() {
        let dir = scratch("clear");
        write_in(&dir, 7, &[]).expect("write");
        clear_in(&dir);
        clear_in(&dir);
        assert_eq!(read_in(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
