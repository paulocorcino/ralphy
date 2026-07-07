//! Per-platform rendering of a [`TimerSpec`] into the exact command the OS
//! scheduler registers, plus the crontab read-modify helpers and the
//! `schtasks /Query` status parser. Every function here is host-independent and
//! takes an explicit [`Platform`] so a single host unit-tests BOTH backends'
//! output without a live scheduler (the injected-`Platform` mirror of
//! `runlock`'s injected liveness predicate).

// This module renders BOTH scheduler backends, but the host executor in
// `schedule.rs` is `#[cfg]`-split so any single-platform build calls only half
// of it (the other half is exercised only from tests, which dead-code analysis
// ignores). The unused half is live on the other OS — allow rather than lose it.
#![allow(dead_code)]

use std::path::Path;

use super::spec::{Schedule, TimerSpec};

/// Which scheduler backend to render for. Explicit (not `#[cfg]`) so both
/// outputs are testable from either host; only the executor in `schedule.rs`
/// binds this to the running platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Cron,
}

/// The trailing crontab comment prefix that marks a Ralphy-installed line.
/// `status` and `remove` match on `<prefix><target-slug>:<repo-root>` via
/// [`cron_tag`], so a given target's tag must stay byte-identical between
/// install and remove — it is the only handle we have on a user-global crontab
/// we must not otherwise disturb. Per-target (not per-repo alone) so `run` and
/// `triage` timers on the same repo never strip each other's line.
pub const CRON_TAG_PREFIX: &str = "# ralphy-schedule:";

/// The full crontab tag for `slug` (`"run"`/`"triage"`) anchored at `wd`.
pub fn cron_tag(slug: &str, wd: &Path) -> String {
    format!("{CRON_TAG_PREFIX}{slug}:{}", wd.display())
}

/// Render the install command for `spec` on `p`.
///
/// - Windows: the `schtasks /Create …` argv, each element one argument. The
///   `/TR` value wraps the invocation in `pwsh -NoProfile -Command "…"` so the
///   working-directory `Set-Location` and the `*>>` all-stream log redirect are
///   handled — the two traps `docs/scheduling.md` flags for Task Scheduler.
/// - Cron: a single-element vec holding the crontab line, `cd`-anchored, output
///   `>> … 2>&1`-redirected, and tagged for removal.
pub fn render_install(p: Platform, spec: &TimerSpec) -> Vec<String> {
    let args = spec.args.join(" ");
    let wd = spec.working_dir.display();
    let exe = spec.program.display();
    let log = spec.log_path.display();
    let pre = spec.pre_invocation.as_ref().map(|p| p.join(" "));
    match p {
        Platform::Windows => {
            let (sc, mo) = match spec.schedule {
                Schedule::Minutes(n) => ("MINUTE", n),
                Schedule::Hours(n) => ("HOURLY", n),
            };
            // Single-quote the paths so PowerShell tolerates spaces; the outer
            // double-quotes belong to the `-Command` argument, not to schtasks
            // shell-quoting (we pass this argv straight to CreateProcess).
            let tr = match &pre {
                Some(pre) => format!(
                    "pwsh -NoProfile -Command \"Set-Location '{wd}'; '{exe}' {pre} *>> '{log}'; '{exe}' {args} *>> '{log}'\""
                ),
                None => format!(
                    "pwsh -NoProfile -Command \"Set-Location '{wd}'; '{exe}' {args} *>> '{log}'\""
                ),
            };
            vec![
                "schtasks".into(),
                "/Create".into(),
                "/TN".into(),
                spec.task_name.clone(),
                "/SC".into(),
                sc.into(),
                "/MO".into(),
                mo.to_string(),
                "/TR".into(),
                tr,
                "/F".into(),
            ]
        }
        Platform::Cron => {
            let expr = match spec.schedule {
                Schedule::Minutes(n) => format!("*/{n} * * * *"),
                Schedule::Hours(n) => format!("0 */{n} * * *"),
            };
            let tag = &spec.cron_tag;
            let line = match &pre {
                Some(pre) => format!(
                    "{expr} cd '{wd}' && {{ '{exe}' {pre} ; '{exe}' {args} ; }} >> '{log}' 2>&1 {tag}"
                ),
                None => format!("{expr} cd '{wd}' && '{exe}' {args} >> '{log}' 2>&1 {tag}"),
            };
            vec![line]
        }
    }
}

/// Render the removal command. Windows: the `schtasks /Delete /TN <task> /F`
/// argv. Cron: removal is a crontab read-modify-write via [`strip_cron_line`],
/// so there is no argv to render — an empty vec.
pub fn render_remove(p: Platform, task_name: &str) -> Vec<String> {
    match p {
        Platform::Windows => vec![
            "schtasks".into(),
            "/Delete".into(),
            "/TN".into(),
            task_name.into(),
            "/F".into(),
        ],
        Platform::Cron => Vec::new(),
    }
}

/// Drop only the crontab line tagged `tag` (a [`cron_tag`] value), preserving
/// every other line verbatim — including another target's tagged line on the
/// same repo — and the input's trailing-newline shape. This is the
/// read-modify half of cron install/remove; the write half pipes the result
/// back through `crontab -`.
pub fn strip_cron_line(existing: &str, tag: &str) -> String {
    let kept: Vec<&str> = existing
        .lines()
        .filter(|line| !line.trim_end().ends_with(tag))
        .collect();
    let mut out = kept.join("\n");
    if existing.ends_with('\n') && !out.is_empty() {
        out.push('\n');
    }
    out
}

/// What a `schtasks /Query /TN <task> /FO LIST /V` inspection found. Cron has no
/// equivalent (see the module docs), so its status path never builds one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimerStatus {
    pub registered: bool,
    pub last_run: Option<String>,
    pub last_result: Option<String>,
    pub next_run: Option<String>,
}

/// Parse the `schtasks … /FO LIST /V` LIST format. A non-empty `TaskName:` field
/// means the task exists; the three history fields are extracted when present.
/// Empty/error output (task absent) yields `registered == false`.
pub fn parse_status(fo_list_output: &str) -> TimerStatus {
    let mut status = TimerStatus {
        registered: false,
        last_run: None,
        last_result: None,
        next_run: None,
    };
    for raw in fo_list_output.lines() {
        let line = raw.trim();
        if field(line, "TaskName:").is_some() {
            status.registered = true;
        } else if let Some(v) = field(line, "Last Run Time:") {
            status.last_run = Some(v);
        } else if let Some(v) = field(line, "Last Result:") {
            status.last_result = Some(v);
        } else if let Some(v) = field(line, "Next Run Time:") {
            status.next_run = Some(v);
        }
    }
    status
}

/// The trimmed value after `prefix` on a LIST line, or `None` when the prefix
/// does not match or the value is empty.
fn field(line: &str, prefix: &str) -> Option<String> {
    line.strip_prefix(prefix)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn spec(schedule: Schedule) -> TimerSpec {
        TimerSpec {
            task_name: "ralphy-run-myrepo".into(),
            program: PathBuf::from("/usr/local/bin/ralphy"),
            args: vec!["run".into(), "--if-idle".into()],
            working_dir: PathBuf::from("/home/me/myrepo"),
            log_path: PathBuf::from("/home/me/myrepo/.ralphy/schedule.log"),
            schedule,
            cron_tag: "# ralphy-schedule:run:/home/me/myrepo".into(),
            pre_invocation: None,
        }
    }

    fn triage_spec(schedule: Schedule) -> TimerSpec {
        TimerSpec {
            task_name: "ralphy-triage-myrepo".into(),
            program: PathBuf::from("/usr/local/bin/ralphy"),
            args: vec!["triage".into(), "--if-idle".into(), "--yes".into()],
            working_dir: PathBuf::from("/home/me/myrepo"),
            log_path: PathBuf::from("/home/me/myrepo/.ralphy/schedule.log"),
            schedule,
            cron_tag: "# ralphy-schedule:triage:/home/me/myrepo".into(),
            pre_invocation: None,
        }
    }

    #[test]
    fn render_install_windows_minutes() {
        let argv = render_install(Platform::Windows, &spec(Schedule::Minutes(30)));
        let joined = argv.join(" ");
        for needle in [
            "/Create",
            "/SC",
            "MINUTE",
            "/MO",
            "30",
            "run",
            "--if-idle",
            "Set-Location",
            "*>>",
        ] {
            assert!(joined.contains(needle), "missing {needle:?} in {joined:?}");
        }
    }

    #[test]
    fn render_install_windows_hours() {
        let argv = render_install(Platform::Windows, &spec(Schedule::Hours(2)));
        let joined = argv.join(" ");
        assert!(joined.contains("/SC"));
        assert!(joined.contains("HOURLY"));
        assert!(joined.contains("/MO 2"), "hours interval must map to /MO 2");
    }

    #[test]
    fn render_install_cron_minutes() {
        let argv = render_install(Platform::Cron, &spec(Schedule::Minutes(30)));
        assert_eq!(argv.len(), 1, "cron renders exactly one crontab line");
        let line = &argv[0];
        for needle in [
            "*/30 * * * *",
            "cd ",
            "run --if-idle",
            ">> ",
            "2>&1",
            "# ralphy-schedule:run:",
        ] {
            assert!(line.contains(needle), "missing {needle:?} in {line:?}");
        }
    }

    #[test]
    fn render_install_cron_hours() {
        let argv = render_install(Platform::Cron, &spec(Schedule::Hours(2)));
        assert!(
            argv[0].contains("0 */2 * * *"),
            "hours must map to 0 */2 * * *"
        );
    }

    #[test]
    fn render_remove_windows() {
        let argv = render_remove(Platform::Windows, "ralphy-run-myrepo");
        let joined = argv.join(" ");
        assert!(joined.contains("/Delete"));
        assert!(joined.contains("/TN"));
        assert!(joined.contains("ralphy-run-myrepo"));
    }

    #[test]
    fn strip_cron_line_removes_only_tagged() {
        let s = spec(Schedule::Minutes(30));
        let installed = render_install(Platform::Cron, &s).remove(0);
        let crontab = format!("# keep me\n{installed}\n");
        let out = strip_cron_line(&crontab, &s.cron_tag);
        assert!(out.contains("# keep me"), "unrelated line must survive");
        assert!(
            !out.contains("run --if-idle"),
            "the tagged ralphy line must be gone"
        );
    }

    #[test]
    fn strip_cron_line_is_target_scoped() {
        let run = spec(Schedule::Minutes(30));
        let triage = triage_spec(Schedule::Minutes(30));
        let run_line = render_install(Platform::Cron, &run).remove(0);
        let triage_line = render_install(Platform::Cron, &triage).remove(0);
        let crontab = format!("{run_line}\n{triage_line}\n");

        let out = strip_cron_line(&crontab, &triage.cron_tag);
        assert!(
            out.contains("run --if-idle"),
            "removing the triage line must leave the run line intact: {out:?}"
        );
        assert!(
            !out.contains("triage --if-idle --yes"),
            "the tagged triage line must be gone: {out:?}"
        );
    }

    #[test]
    fn render_install_windows_triage() {
        let argv = render_install(Platform::Windows, &triage_spec(Schedule::Minutes(30)));
        let joined = argv.join(" ");
        for needle in [
            "triage",
            "--if-idle",
            "--yes",
            "/SC",
            "MINUTE",
            "ralphy-triage-myrepo",
        ] {
            assert!(joined.contains(needle), "missing {needle:?} in {joined:?}");
        }
    }

    #[test]
    fn render_install_cron_triage() {
        let argv = render_install(Platform::Cron, &triage_spec(Schedule::Minutes(30)));
        let line = &argv[0];
        for needle in [
            "triage --if-idle --yes",
            "*/30 * * * *",
            "# ralphy-schedule:triage:",
        ] {
            assert!(line.contains(needle), "missing {needle:?} in {line:?}");
        }
    }

    #[test]
    fn render_install_windows_run_with_triage() {
        let mut s = spec(Schedule::Minutes(30));
        s.pre_invocation = Some(vec!["triage".into(), "--yes".into()]);
        let argv = render_install(Platform::Windows, &s);
        let joined = argv.join(" ");
        assert!(joined.contains("triage --yes"), "{joined:?}");
        assert!(joined.contains("run --if-idle"), "{joined:?}");
        assert!(joined.contains("ralphy-run-myrepo"), "{joined:?}");
        let triage_idx = joined.find("triage --yes").unwrap();
        let run_idx = joined.find("run --if-idle").unwrap();
        assert!(
            triage_idx < run_idx,
            "triage must render before run: {joined:?}"
        );
        assert!(
            joined.contains("; '"),
            "phases must be `;`-chained: {joined:?}"
        );
        assert!(
            !joined.contains("triage --yes &&"),
            "chain must use `;`, not `&&`: {joined:?}"
        );
    }

    #[test]
    fn render_install_cron_run_with_triage() {
        let mut s = spec(Schedule::Minutes(30));
        s.pre_invocation = Some(vec!["triage".into(), "--yes".into()]);
        let argv = render_install(Platform::Cron, &s);
        assert_eq!(argv.len(), 1);
        let line = &argv[0];
        for needle in [
            "*/30 * * * *",
            "cd '",
            "triage --yes ;",
            "run --if-idle",
            "2>&1",
            "# ralphy-schedule:run:",
        ] {
            assert!(line.contains(needle), "missing {needle:?} in {line:?}");
        }
        let triage_idx = line.find("triage --yes").unwrap();
        let run_idx = line.find("run --if-idle").unwrap();
        assert!(
            triage_idx < run_idx,
            "triage must render before run: {line:?}"
        );
        assert!(
            !line.contains("triage --yes && "),
            "chain must use `;`, not `&&`: {line:?}"
        );
    }

    #[test]
    fn strip_cron_line_removes_chained_run_line() {
        let mut s = spec(Schedule::Minutes(30));
        s.pre_invocation = Some(vec!["triage".into(), "--yes".into()]);
        let installed = render_install(Platform::Cron, &s).remove(0);
        let crontab = format!("# keep me\n{installed}\n");
        let out = strip_cron_line(&crontab, &s.cron_tag);
        assert!(out.contains("# keep me"), "unrelated line must survive");
        assert!(
            !out.contains("triage --yes"),
            "chained triage phase must be gone"
        );
        assert!(
            !out.contains("run --if-idle"),
            "chained run phase must be gone"
        );
    }

    #[test]
    fn parse_status_extracts_fields() {
        let fixture = "\
Folder: \\
TaskName:                             \\ralphy-run-myrepo
Next Run Time:                        7/7/2026 12:00:00 AM
Status:                               Ready
Last Run Time:                        7/6/2026 11:30:00 PM
Last Result:                          0
";
        let st = parse_status(fixture);
        assert!(st.registered);
        assert_eq!(st.last_run.as_deref(), Some("7/6/2026 11:30:00 PM"));
        assert_eq!(st.last_result.as_deref(), Some("0"));
        assert_eq!(st.next_run.as_deref(), Some("7/7/2026 12:00:00 AM"));
    }

    #[test]
    fn parse_status_empty_is_unregistered() {
        let st = parse_status("");
        assert!(!st.registered);
        assert!(st.last_run.is_none());
    }
}
