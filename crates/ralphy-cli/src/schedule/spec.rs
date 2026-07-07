//! The host-independent scheduling spec: the firing cadence ([`Schedule`]) and
//! the fully-resolved [`TimerSpec`] a platform renderer turns into an OS timer.
//! Nothing here touches the OS scheduler — it is pure data so both platforms'
//! output is unit-testable from either host.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use ralphy_core::Workspace;

/// A firing cadence in whole minutes or whole hours — the two granularities the
/// tracer-bullet recipes need (`30m`, `2h`). Both scheduler backends map this to
/// their native cadence: minutes → `/SC MINUTE` / `*/N * * * *`, hours →
/// `/SC HOURLY` / `0 */N * * *`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schedule {
    Minutes(u32),
    Hours(u32),
}

/// Parse `--every` (`<N>m` or `<N>h`) into a [`Schedule`]. Rejects an empty
/// string, a zero interval, a missing numeric head, and any unit other than
/// `m`/`h` with a clear message.
pub fn parse_interval(s: &str) -> Result<Schedule> {
    let s = s.trim();
    let unit = match s.chars().last() {
        Some(c) => c,
        None => bail!("empty interval: use <N>m or <N>h (e.g. 30m, 2h)"),
    };
    let head = &s[..s.len() - unit.len_utf8()];
    let n: u32 = head.parse().map_err(|_| {
        anyhow::anyhow!("invalid interval {s:?}: expected <N>m or <N>h (e.g. 30m, 2h)")
    })?;
    if n == 0 {
        bail!("interval must be positive, got {s:?}");
    }
    match unit {
        // Bounds keep the two backends in agreement: a cron `*/N` step above the
        // field maximum silently caps (e.g. `*/90 * * * *` fires only at minute
        // 0, hourly — NOT every 90 min) while Task Scheduler's `/MO 90` is valid.
        // Rejecting the divergent range is honester than emitting a timer that
        // fires at a different cadence than the operator asked for.
        'm' if n > 59 => bail!("minute interval must be 1–59, got {s:?}; use hours (e.g. 2h)"),
        'm' => Ok(Schedule::Minutes(n)),
        'h' if n > 23 => bail!("hour interval must be 1–23, got {s:?}"),
        'h' => Ok(Schedule::Hours(n)),
        other => bail!("unknown interval unit {other:?} in {s:?}: use m (minutes) or h (hours)"),
    }
}

/// A fully-resolved timer registration: the program + args to invoke, where to
/// run it, where to capture its output, and how often. Platform-neutral; the
/// `platform` renderers turn it into a `schtasks` argv or a crontab line.
#[derive(Debug, Clone)]
pub struct TimerSpec {
    pub task_name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub working_dir: PathBuf,
    pub log_path: PathBuf,
    pub schedule: Schedule,
}

/// Build the `run` target's timer spec: `ralphy run --if-idle`, anchored at the
/// repo root, logging to `<repo>/.ralphy/schedule.log` unless `log` overrides.
/// The task name is keyed by the repo directory so two repos never collide on a
/// machine-global Task Scheduler name or a user-global crontab.
pub fn run_spec(ws: &Workspace, exe: &Path, schedule: Schedule, log: Option<PathBuf>) -> TimerSpec {
    let repo_root = ws.repo_root();
    let repo_name = repo_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());
    let log_path = log.unwrap_or_else(|| ws.ralphy_dir().join("schedule.log"));
    TimerSpec {
        task_name: format!("ralphy-run-{repo_name}"),
        program: exe.to_path_buf(),
        args: vec!["run".into(), "--if-idle".into()],
        working_dir: repo_root.to_path_buf(),
        log_path,
        schedule,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_interval_minutes_and_hours() {
        assert_eq!(parse_interval("30m").unwrap(), Schedule::Minutes(30));
        assert_eq!(parse_interval("2h").unwrap(), Schedule::Hours(2));
        // Surrounding whitespace is tolerated.
        assert_eq!(parse_interval(" 5m ").unwrap(), Schedule::Minutes(5));
    }

    #[test]
    fn parse_interval_rejects_garbage() {
        assert!(parse_interval("foo").is_err());
        assert!(parse_interval("").is_err());
        assert!(parse_interval("30").is_err(), "no unit must be rejected");
        assert!(
            parse_interval("0m").is_err(),
            "zero interval must be rejected"
        );
        assert!(
            parse_interval("5d").is_err(),
            "unknown unit must be rejected"
        );
        // Out-of-range: a cron `*/N` step above the field max would silently cap.
        assert!(
            parse_interval("90m").is_err(),
            "minutes >59 must be rejected (cron caps the step)"
        );
        assert!(parse_interval("59m").is_ok(), "59m is the top valid minute");
        assert!(parse_interval("24h").is_err(), "hours >23 must be rejected");
        assert!(parse_interval("23h").is_ok(), "23h is the top valid hour");
    }

    #[test]
    fn run_spec_keys_task_by_repo_and_defaults_log() {
        let ws = Workspace::new("/home/me/myrepo");
        let spec = run_spec(
            &ws,
            Path::new("/usr/local/bin/ralphy"),
            Schedule::Minutes(30),
            None,
        );
        assert_eq!(spec.task_name, "ralphy-run-myrepo");
        assert_eq!(spec.args, vec!["run".to_string(), "--if-idle".to_string()]);
        assert_eq!(spec.working_dir, Path::new("/home/me/myrepo"));
        assert!(spec.log_path.ends_with("schedule.log"));
    }
}
