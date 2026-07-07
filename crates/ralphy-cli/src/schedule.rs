//! `ralphy schedule` (ADR-0026): register / inspect / remove a native OS timer
//! that re-invokes `ralphy run --if-idle`. The timer lives in the OS scheduler
//! (Windows Task Scheduler or cron); ralphy only writes and removes its
//! registration — it never becomes the scheduler. `run` target only for now.
//!
//! The per-platform *rendering* (`schtasks` argv / crontab line) and the status
//! parser are pure and host-independent so BOTH backends are unit-tested from
//! either host (`platform`); only the thin executor here is `#[cfg]`-gated to
//! the running platform, exactly as `runlock`'s liveness predicate is.

mod platform;
mod spec;

use std::path::{Path, PathBuf};
use std::process::Command as ProcCommand;

use anyhow::{Context, Result};
use clap::{Subcommand, ValueEnum};
use ralphy_core::{git, Workspace};

use platform::Platform;
use spec::{parse_interval, Schedule, TimerSpec};

/// The `ralphy schedule` command group. `status`/`remove` need a noun `run`
/// could never host (ADR-0026 §1).
#[derive(Subcommand)]
pub(crate) enum ScheduleCommand {
    /// Register a native OS timer for the given target that fires on a cadence.
    Install {
        /// What to schedule. Only `run` today (registers `ralphy run --if-idle`).
        #[arg(value_enum)]
        target: ScheduleTarget,
        /// Firing cadence: `<N>m` (minutes) or `<N>h` (hours).
        #[arg(long, default_value = "30m")]
        every: String,
        /// Where the timer appends run output (default:
        /// `<repo>/.ralphy/schedule.log`).
        #[arg(long)]
        log: Option<PathBuf>,
        /// Any path inside the target repo; resolved to its git toplevel.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// Show the Ralphy timer registered for this repo and its firing history.
    Status {
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// Unregister the timer for the given target.
    Remove {
        #[arg(value_enum)]
        target: ScheduleTarget,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
}

/// The `install`/`remove` target noun. Scoped to `run` for the tracer bullet;
/// `triage` / `--all` land in later slices (ADR-0026 §3–4).
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum ScheduleTarget {
    Run,
}

/// Dispatch a `schedule` subcommand.
pub(crate) fn run(cmd: ScheduleCommand) -> Result<()> {
    match cmd {
        ScheduleCommand::Install {
            target: ScheduleTarget::Run,
            every,
            log,
            repo,
        } => install(&repo, &every, log),
        ScheduleCommand::Status { repo } => status(&repo),
        ScheduleCommand::Remove {
            target: ScheduleTarget::Run,
            repo,
        } => remove(&repo),
    }
}

fn workspace(repo: &Path) -> Result<Workspace> {
    let root = git::resolve_toplevel(repo)?;
    Ok(Workspace::new(root))
}

/// The canonicalized absolute path to the running binary, so the registered
/// timer resolves `ralphy` regardless of the scheduler's stripped PATH (the
/// `docs/scheduling.md` minimal-PATH trap). Mirrors `install.rs`.
fn current_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("locating the running ralphy binary")?;
    Ok(std::fs::canonicalize(&exe).unwrap_or(exe))
}

fn describe(s: Schedule) -> String {
    match s {
        Schedule::Minutes(n) => format!("every {n} min"),
        Schedule::Hours(n) => format!("every {n}h"),
    }
}

fn install(repo: &Path, every: &str, log: Option<PathBuf>) -> Result<()> {
    let ws = workspace(repo)?;
    let exe = current_exe()?;
    let schedule = parse_interval(every)?;
    let spec = spec::run_spec(&ws, &exe, schedule, log);
    host_install(&spec)?;
    println!(
        "Registered timer {} ({}), logging to {}.",
        spec.task_name,
        describe(schedule),
        spec.log_path.display()
    );
    Ok(())
}

fn remove(repo: &Path) -> Result<()> {
    let ws = workspace(repo)?;
    let exe = current_exe()?;
    // schedule/log are irrelevant to removal — only task_name/working_dir are used.
    let spec = spec::run_spec(&ws, &exe, Schedule::Minutes(30), None);
    host_remove(&spec)?;
    println!("Removed timer {}.", spec.task_name);
    Ok(())
}

fn status(repo: &Path) -> Result<()> {
    let ws = workspace(repo)?;
    let exe = current_exe()?;
    let spec = spec::run_spec(&ws, &exe, Schedule::Minutes(30), None);
    host_status(&spec)
}

// --- host executor (the only `#[cfg]`-gated seam) --------------------------

#[cfg(windows)]
fn host_install(spec: &TimerSpec) -> Result<()> {
    run_argv(&platform::render_install(Platform::Windows, spec))
}

#[cfg(not(windows))]
fn host_install(spec: &TimerSpec) -> Result<()> {
    let line = platform::render_install(Platform::Cron, spec).remove(0);
    // Idempotent: drop any prior tagged line for this repo, then append fresh.
    let mut base = platform::strip_cron_line(&read_crontab()?, &spec.working_dir);
    if !base.is_empty() && !base.ends_with('\n') {
        base.push('\n');
    }
    base.push_str(&line);
    base.push('\n');
    write_crontab(&base)
}

#[cfg(windows)]
fn host_remove(spec: &TimerSpec) -> Result<()> {
    run_argv(&platform::render_remove(Platform::Windows, &spec.task_name))
}

#[cfg(not(windows))]
fn host_remove(spec: &TimerSpec) -> Result<()> {
    let stripped = platform::strip_cron_line(&read_crontab()?, &spec.working_dir);
    write_crontab(&stripped)
}

#[cfg(windows)]
fn host_status(spec: &TimerSpec) -> Result<()> {
    let out = ProcCommand::new("schtasks")
        .args(["/Query", "/TN", &spec.task_name, "/FO", "LIST", "/V"])
        .output()
        .context("querying schtasks")?;
    let st = platform::parse_status(&String::from_utf8_lossy(&out.stdout));
    if !st.registered {
        println!("{}: not registered.", spec.task_name);
        return Ok(());
    }
    println!("{}: registered.", spec.task_name);
    println!(
        "  last firing: {}",
        st.last_run.as_deref().unwrap_or("unknown")
    );
    println!(
        "  last result: {}",
        st.last_result.as_deref().unwrap_or("unknown")
    );
    println!(
        "  next tick:   {}",
        st.next_run.as_deref().unwrap_or("unknown")
    );
    Ok(())
}

#[cfg(not(windows))]
fn host_status(spec: &TimerSpec) -> Result<()> {
    let existing = read_crontab().unwrap_or_default();
    let tag = format!(
        "{}{}",
        platform::CRON_TAG_PREFIX,
        spec.working_dir.display()
    );
    match existing.lines().find(|l| l.trim_end().ends_with(&tag)) {
        Some(line) => {
            let expr: String = line
                .split_whitespace()
                .take(5)
                .collect::<Vec<_>>()
                .join(" ");
            println!("{}: registered ({expr}).", spec.task_name);
            // cron keeps no run history — an honest asymmetry, not a gap to fake.
            println!("  last firing: unavailable (cron keeps no run history)");
            println!("  last result: unavailable (cron keeps no run history)");
            println!("  next tick:   unavailable (cron keeps no run history)");
        }
        None => println!("{}: not registered.", spec.task_name),
    }
    Ok(())
}

#[cfg(windows)]
fn run_argv(argv: &[String]) -> Result<()> {
    let (prog, rest) = argv.split_first().context("empty scheduler command")?;
    let status = ProcCommand::new(prog)
        .args(rest)
        .status()
        .with_context(|| format!("running {prog}"))?;
    if !status.success() {
        anyhow::bail!("{prog} exited with {status}");
    }
    Ok(())
}

#[cfg(not(windows))]
fn read_crontab() -> Result<String> {
    let out = ProcCommand::new("crontab")
        .arg("-l")
        .output()
        .context("reading crontab (is cron installed?)")?;
    // `crontab -l` exits non-zero with "no crontab for user" when empty — an
    // empty crontab is not an error for us.
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Ok(String::new())
    }
}

#[cfg(not(windows))]
fn write_crontab(content: &str) -> Result<()> {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = ProcCommand::new("crontab")
        .arg("-")
        .stdin(Stdio::piped())
        .spawn()
        .context("writing crontab")?;
    child
        .stdin
        .as_mut()
        .context("crontab stdin unavailable")?
        .write_all(content.as_bytes())
        .context("writing crontab content")?;
    let status = child.wait().context("waiting for crontab")?;
    if !status.success() {
        anyhow::bail!("crontab - exited with {status}");
    }
    Ok(())
}
