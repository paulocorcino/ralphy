//! `gate`: run every check of the CI gate locally, in the order of
//! [`steps::STEPS`]. It does not stop at the first failure, writes the output
//! of each step to `target/gate/<step>.log`, and prints one row per step. A
//! tool that is not installed is `MISSING`, never a pass.

mod steps;

use std::fs::File;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{Context, Result};

use steps::{Step, STEPS};

/// What one step came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Passed,
    /// The exit code, or `None` when a signal ended it.
    Failed(Option<i32>),
    /// The program, or the binary of a cargo subcommand, is not on `PATH`.
    Missing,
    /// `--fast` left it out.
    Skipped,
}

impl Outcome {
    fn label(self) -> String {
        match self {
            Outcome::Passed => "ok".to_string(),
            Outcome::Failed(Some(code)) => format!("FAILED ({code})"),
            Outcome::Failed(None) => "FAILED (signal)".to_string(),
            Outcome::Missing => "MISSING".to_string(),
            Outcome::Skipped => "skipped".to_string(),
        }
    }

    fn is_bad(self) -> bool {
        matches!(self, Outcome::Failed(_) | Outcome::Missing)
    }
}

/// The flags of `gate`.
#[derive(Debug, PartialEq, Eq)]
struct Options {
    fast: bool,
}

fn parse_args(args: &[String]) -> Result<Options> {
    let mut options = Options { fast: false };
    for arg in args {
        match arg.as_str() {
            "--fast" => options.fast = true,
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }
    Ok(options)
}

/// The program to start. A step whose program is [`steps::XTASK`] runs
/// `xtask`, the program of this gate. On Windows `npx` is a `.cmd` file,
/// which `Command` finds only by its full name.
fn program_path(program: &str, xtask: &Path) -> PathBuf {
    match program {
        steps::XTASK => xtask.to_path_buf(),
        "npx" if cfg!(windows) => PathBuf::from("npx.cmd"),
        _ => PathBuf::from(program),
    }
}

/// Move `exe` to `parked`, over a copy an earlier gate left there.
fn park(exe: &Path, parked: &Path) -> Result<()> {
    match std::fs::remove_file(parked) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("deleting {}", parked.display())),
    }
    std::fs::rename(exe, parked).with_context(|| format!("moving {} aside", exe.display()))
}

/// The program of this gate, for the steps that run xtask. On Windows a
/// running program cannot be deleted, and the step that builds xtask must
/// replace `target/debug/xtask.exe`: the gate moves its own file into `dir`
/// first, and runs it from there.
fn own_program(dir: &Path) -> Result<PathBuf> {
    let exe = std::env::current_exe().context("finding the running xtask")?;
    if !cfg!(windows) {
        return Ok(exe);
    }
    let parked = dir.join("xtask-running.exe");
    park(&exe, &parked)?;
    Ok(parked)
}

/// The command line, as the first line of the log says it.
fn command_line(step: &Step) -> String {
    let env = step.env.iter().map(|(k, v)| format!("{k}=\"{v}\" "));
    let mut line: String = env.collect();
    line.push_str(step.program);
    for arg in step.args {
        line.push(' ');
        line.push_str(arg);
    }
    line
}

/// Whether `program` can be started at all.
fn on_path(program: &str) -> bool {
    let probe = Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    !matches!(probe, Err(e) if e.kind() == ErrorKind::NotFound)
}

/// Run one step in `root`, its output in `log`.
fn run_step(root: &Path, step: &Step, xtask: &Path, log: &Path) -> Result<Outcome> {
    let mut file = File::create(log).with_context(|| format!("creating {}", log.display()))?;
    writeln!(file, "$ {}", command_line(step)).context("writing the gate log")?;
    writeln!(file, "# the local form of CI step: {}", step.ci.join(", "))
        .context("writing the gate log")?;
    if let Some(absent) = step.needs.filter(|p| !on_path(p)) {
        writeln!(file, "{absent} is not on PATH").context("writing the gate log")?;
        return Ok(Outcome::Missing);
    }
    let stderr = file.try_clone().context("sharing the gate log")?;
    let status = Command::new(program_path(step.program, xtask))
        .args(step.args)
        .envs(step.env.iter().copied())
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(file)
        .stderr(stderr)
        .status();
    match status {
        Ok(s) if s.success() => Ok(Outcome::Passed),
        Ok(s) => Ok(Outcome::Failed(s.code())),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Outcome::Missing),
        Err(e) => Err(e).with_context(|| format!("starting {}", step.program)),
    }
}

/// `<root>/target/gate`, where the logs go.
fn log_dir(root: &Path) -> PathBuf {
    root.join("target").join("gate")
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("xtask lives at <root>/crates/xtask")
        .to_path_buf()
}

/// Run every step of `steps` in `root` and print one row for each. Returns
/// the outcomes in order.
fn run_all(root: &Path, steps: &[Step], options: &Options, xtask: &Path) -> Result<Vec<Outcome>> {
    let dir = log_dir(root);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut outcomes = Vec::with_capacity(steps.len());
    let mut out = std::io::stdout();
    for step in steps {
        write!(out, "{:<16}", step.name).context("writing the gate table")?;
        out.flush().context("writing the gate table")?;
        let start = Instant::now();
        let log = dir.join(format!("{}.log", step.name));
        let outcome = if options.fast && step.slow {
            Outcome::Skipped
        } else {
            run_step(root, step, xtask, &log)?
        };
        let secs = start.elapsed().as_secs_f64();
        writeln!(out, "{:<18}{secs:>7.1}s", outcome.label()).context("writing the gate table")?;
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

pub(crate) fn gate_cmd(args: &[String]) -> Result<()> {
    let options = parse_args(args)?;
    let root = workspace_root();
    let dir = log_dir(&root);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let xtask = own_program(&dir)?;
    let outcomes = run_all(&root, STEPS, &options, &xtask)?;
    let bad: Vec<&str> = STEPS
        .iter()
        .zip(&outcomes)
        .filter(|(_, o)| o.is_bad())
        .map(|(s, _)| s.name)
        .collect();
    if options.fast {
        println!("--fast: the workspace tests did not run; the gate is not done");
    }
    if !bad.is_empty() {
        let dir = log_dir(&root);
        anyhow::bail!(
            "the gate did not pass: {}; logs in {}",
            bad.join(", "),
            dir.display()
        );
    }
    println!("the gate passed; logs in {}", log_dir(&root).display());
    Ok(())
}

#[cfg(test)]
mod tests;
