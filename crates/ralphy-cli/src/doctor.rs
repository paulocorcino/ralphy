//! `ralphy doctor`: check this computer for the tools a run needs (ADR-0056
//! §12). This module owns that list: `ralphy init` reads its probes, and
//! `ralphy host add` reads `ralphy doctor --json` on the host.
//!
//! It checks presence only. A minimum version is added only after a measured
//! failure, with that failure in a comment.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Args;
use ralphy_adapter_support::{find_program, locate_program, run_headless};
use ralphy_daemon::agent_state::{state_source, StateSource};
use ralphy_proc_util::hook_form::{self, HookCheck};
use serde::{Deserialize, Serialize};

use crate::init::Agent;

#[derive(Args)]
pub(crate) struct DoctorArgs {
    /// Print the result as JSON, for a program to read.
    #[arg(long)]
    json: bool,
}

/// The tools `doctor` checks. A closed set: a new tool changes every match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Tool {
    Git,
    Gh,
    Python,
    Agent,
    Linger,
    /// Whether the hooks of an agent that gives its state through hooks run.
    Hooks,
}

/// One line of the result: one tool, what was found, and how to fix it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ToolLine {
    pub tool: Tool,
    /// The program name, such as `git` or `claude`.
    pub name: String,
    pub ok: bool,
    /// A missing required tool makes `doctor` exit non-zero.
    pub required: bool,
    /// The first line of `<tool> --version`, when it answered.
    pub version: Option<String>,
    pub text: String,
    /// A command that fixes a missing tool.
    pub fix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DoctorReport {
    pub tools: Vec<ToolLine>,
}

impl DoctorReport {
    pub(crate) fn missing_required(&self) -> usize {
        self.tools.iter().filter(|t| t.required && !t.ok).count()
    }
}

/// What the probes found on this computer. A version is `Some` when the
/// program exists; the text inside is empty when `--version` did not answer.
#[derive(Debug, Clone, Default)]
pub(crate) struct MachineFacts {
    pub git: Option<String>,
    pub gh: Option<String>,
    pub gh_signed_in: bool,
    pub python: Option<String>,
    /// The agent CLIs found, in `Agent::ALL` order, with their versions.
    pub agents: Vec<(Agent, String)>,
    /// `None` off Linux; on Linux, `None` inside when it could not be read.
    pub linger: Option<Option<bool>>,
    /// The hook check of each installed agent whose state comes from hooks.
    pub hooks: Vec<(Agent, HookCheck)>,
}

/// The lingering line, shared with the host check so both say the same thing.
pub(crate) fn linger_text(linger: Option<bool>) -> &'static str {
    match linger {
        Some(true) => "lingering is on: the daemon runs with no one signed in",
        Some(false) => "lingering is off: the daemon stops when you sign out",
        None => "could not read lingering: the daemon may stop when you sign out",
    }
}

fn version_text(version: &str) -> Option<String> {
    (!version.is_empty()).then(|| version.to_string())
}

fn present(tool: Tool, name: &str, version: &str) -> ToolLine {
    ToolLine {
        tool,
        name: name.to_string(),
        ok: true,
        required: true,
        version: version_text(version),
        text: if version.is_empty() {
            "found".to_string()
        } else {
            version.to_string()
        },
        fix: None,
    }
}

fn missing(tool: Tool, name: &str, text: &str, fix: Option<&str>) -> ToolLine {
    ToolLine {
        tool,
        name: name.to_string(),
        ok: false,
        required: true,
        version: None,
        text: text.to_string(),
        fix: fix.map(str::to_string),
    }
}

fn git_line(facts: &MachineFacts, windows: bool) -> ToolLine {
    match &facts.git {
        Some(v) => present(Tool::Git, "git", v),
        None if windows => missing(
            Tool::Git,
            "git",
            "not found. Ralphy needs it for every project",
            Some("winget install --id Git.Git -e"),
        ),
        None => missing(
            Tool::Git,
            "git",
            "not found. Ralphy needs it for every project: install it with your package manager",
            None,
        ),
    }
}

fn gh_line(facts: &MachineFacts) -> ToolLine {
    match (&facts.gh, facts.gh_signed_in) {
        (Some(v), true) => present(Tool::Gh, "gh", v),
        (Some(v), false) => ToolLine {
            version: version_text(v),
            ..missing(
                Tool::Gh,
                "gh",
                "installed, but not signed in",
                Some("gh auth login"),
            )
        },
        (None, _) => missing(
            Tool::Gh,
            "gh",
            "not found. Install it from https://cli.github.com, then run `gh auth login`",
            None,
        ),
    }
}

fn python_line(facts: &MachineFacts) -> ToolLine {
    match &facts.python {
        Some(v) => present(Tool::Python, "python", v),
        None => missing(
            Tool::Python,
            "python",
            "not found as `python` or `python3`",
            None,
        ),
    }
}

fn agent_lines(facts: &MachineFacts) -> Vec<ToolLine> {
    if facts.agents.is_empty() {
        let names: Vec<&str> = Agent::ALL.iter().map(Agent::cli_name).collect();
        return vec![missing(
            Tool::Agent,
            "agent CLI",
            &format!("none found. Install one of: {}", names.join(", ")),
            None,
        )];
    }
    facts
        .agents
        .iter()
        .map(|(agent, v)| present(Tool::Agent, agent.cli_name(), v))
        .collect()
}

/// Lingering keeps the daemon running after sign-out; it is worth a warning,
/// never a failure.
fn linger_line(linger: Option<bool>) -> ToolLine {
    ToolLine {
        tool: Tool::Linger,
        name: "lingering".to_string(),
        ok: linger == Some(true),
        required: false,
        version: None,
        text: linger_text(linger).to_string(),
        fix: (linger == Some(false)).then(|| "loginctl enable-linger".to_string()),
    }
}

/// The state source of an agent, from the owner of that fact. The daemon names
/// its agents by the same CLI name.
fn source_of(agent: Agent) -> StateSource {
    ralphy_daemon::session::Agent::from_query(agent.cli_name())
        .map_or(StateSource::None, state_source)
}

/// Whether the state source of `agent` is a plugin the vendor loads, with no
/// command the doctor can run: its first line in each console shows that it
/// runs (ADR-0085 D9).
fn checked_by_first_line(agent: Agent) -> bool {
    match agent {
        Agent::Opencode => true,
        Agent::Claude
        | Agent::Codex
        | Agent::Copilot
        | Agent::Cursor
        | Agent::Gemini
        | Agent::Kimi => false,
    }
}

/// One line for an installed agent: whether its state hooks run. A hook
/// that fails is a warning, never a missing tool.
fn hook_line(facts: &MachineFacts, agent: Agent) -> ToolLine {
    let (ok, text) = match source_of(agent) {
        StateSource::Hooks if checked_by_first_line(agent) => (
            true,
            "a plugin; checked by its first line in each console".to_string(),
        ),
        StateSource::Hooks => match facts.hooks.iter().find(|(a, _)| *a == agent) {
            Some((_, HookCheck::Ok)) => (
                true,
                "the hook command ran and wrote a status line".to_string(),
            ),
            Some((_, HookCheck::Failed { cause })) => (false, cause.clone()),
            Some((_, HookCheck::NotChecked { reason })) => {
                (false, format!("not checked: {reason}"))
            }
            None => (false, "not checked: the check gave no result".to_string()),
        },
        StateSource::None => (true, "no state source".to_string()),
    };
    ToolLine {
        tool: Tool::Hooks,
        name: format!("{} hooks", agent.cli_name()),
        ok,
        required: false,
        version: None,
        text,
        fix: None,
    }
}

/// The result for `facts`, one line per tool, in a fixed order.
pub(crate) fn evaluate(facts: &MachineFacts, windows: bool) -> DoctorReport {
    let mut tools = vec![git_line(facts, windows), gh_line(facts), python_line(facts)];
    tools.extend(agent_lines(facts));
    tools.extend(
        facts
            .agents
            .iter()
            .map(|(agent, _)| hook_line(facts, *agent)),
    );
    if let Some(linger) = facts.linger {
        tools.push(linger_line(linger));
    }
    DoctorReport { tools }
}

pub(crate) fn print_report(report: &DoctorReport, out: &mut impl std::io::Write) -> Result<()> {
    for line in &report.tools {
        let tag = match (line.ok, line.required) {
            (true, _) => "ok  ",
            (false, true) => "miss",
            (false, false) => "warn",
        };
        writeln!(out, "[{tag}] {}: {}", line.name, line.text)?;
        if let Some(fix) = &line.fix {
            writeln!(out, "       run: {fix}")?;
        }
    }
    match report.missing_required() {
        0 => writeln!(out, "result: every required tool is here")?,
        n => writeln!(out, "result: {n} required tool(s) missing")?,
    }
    Ok(())
}

pub(crate) fn run(args: &DoctorArgs) -> Result<()> {
    let report = evaluate(&probe(), cfg!(windows));
    let mut stdout = std::io::stdout();
    if args.json {
        serde_json::to_writer(&mut stdout, &report)?;
        println!();
    } else {
        print_report(&report, &mut stdout)?;
    }
    if report.missing_required() > 0 {
        // The report already says what is missing; an `Error:` line under it
        // would say it twice.
        stdout.flush().context("writing the report")?;
        std::process::exit(1);
    }
    Ok(())
}

// --- probes -----------------------------------------------------------------

/// Run every probe on this computer. The probes are independent and each one
/// starts a program, so they run at the same time.
pub(crate) fn probe() -> MachineFacts {
    let version = |p: Option<PathBuf>| p.map(|p| version_of(&p));
    std::thread::scope(|scope| {
        let git = scope.spawn(|| version(on_path("git")));
        let gh = scope.spawn(|| version(on_path("gh")));
        let signed_in = scope.spawn(gh_signed_in);
        let python = scope.spawn(|| version(on_path("python").or_else(|| on_path("python3"))));
        let agents: Vec<_> = Agent::ALL
            .iter()
            .map(|a| scope.spawn(move || agent_program(a).map(|p| (*a, version_of(&p)))))
            .collect();
        let hooks: Vec<_> = Agent::ALL
            .iter()
            .filter(|a| source_of(**a) == StateSource::Hooks)
            .map(|a| scope.spawn(move || hook_probe(*a)))
            .collect();
        let linger = linger();
        // A probe thread only runs a program and reads its output; a panic in
        // one is a bug in that code, not a fact about the computer.
        let join = "a doctor probe does not panic";
        MachineFacts {
            git: git.join().expect(join),
            gh: gh.join().expect(join),
            gh_signed_in: signed_in.join().expect(join),
            python: python.join().expect(join),
            agents: agents
                .into_iter()
                .filter_map(|h| h.join().expect(join))
                .collect(),
            linger,
            hooks: hooks
                .into_iter()
                .filter_map(|h| h.join().expect(join))
                .collect(),
        }
    })
}

/// Run the hook command of `agent` as a console of the daemon would get it,
/// so the check answers for the command a console gets. `None` when the
/// agent is not installed or gives no state through hooks.
fn hook_probe(agent: Agent) -> Option<(Agent, HookCheck)> {
    let cli = agent_program(&agent)?;
    let hook = hook_command(agent, &cli)?;
    // A `disableAllHooks` file of Claude Code says nothing about Codex.
    let home = (agent == Agent::Claude)
        .then(ralphy_proc_util::home_dir)
        .flatten();
    Some((
        agent,
        hook_form::check(&hook, hook_form::CHECK_LIMIT, home.as_deref()),
    ))
}

/// The hook command a console of `agent` gets, whose program is `cli`: for
/// Claude the same command as `agent_state::run_start_check`; for Codex the
/// program and the arguments its inline hooks run. `None` for an agent that
/// gives no state through hooks, or whose source is a plugin with no command.
fn hook_command(agent: Agent, cli: &Path) -> Option<hook_form::HookCommand> {
    match agent {
        Agent::Claude => Some(ralphy_daemon::agent_state::console_hook(cli)),
        Agent::Codex => Some(ralphy_daemon::agent_state::codex_check_hook()),
        Agent::Copilot | Agent::Cursor | Agent::Gemini | Agent::Kimi | Agent::Opencode => None,
    }
}

fn on_path(name: &str) -> Option<PathBuf> {
    find_program(name, std::env::var_os("PATH"), std::env::var_os("PATHEXT"))
}

/// How long one probe may run. `doctor` runs on a host over ssh and before
/// `init`, so a CLI that waits for input or for the network must not hold it.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Run `cmd` with no input, killed after [`PROBE_TIMEOUT`]. `None` when it
/// did not start, timed out, or failed; else what it printed.
fn probe_output(mut cmd: std::process::Command) -> Option<String> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let program = cmd.get_program().to_string_lossy().into_owned();
    let out = match run_headless(cmd, "", PROBE_TIMEOUT) {
        Ok(out) => out,
        Err(e) => {
            tracing::debug!(error = %e, %program, "probe did not start");
            return None;
        }
    };
    if out.timed_out {
        tracing::debug!(%program, "probe timed out");
    }
    out.exit
        .is_some_and(|status| status.success())
        .then_some(out.stdout)
}

/// The first non-empty line `<program> --version` prints, or an empty string.
fn version_of(program: &Path) -> String {
    let mut cmd = std::process::Command::new(program);
    cmd.arg("--version");
    probe_output(cmd)
        .and_then(|stdout| {
            stdout
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_default()
}

/// `gh auth status` exits 0 only when signed in.
pub(crate) fn gh_signed_in() -> bool {
    let mut cmd = std::process::Command::new("gh");
    cmd.args(["auth", "status"]);
    probe_output(cmd).is_some()
}

// Each CLI resolves through the same locator the adapters spawn through, so a
// CLI under `~/.local/bin` but off `PATH` is found, as a run would find it.
pub(crate) fn agent_program(a: &Agent) -> Option<PathBuf> {
    match a {
        // Cursor installs as `cursor-agent` or `agent`, not as its selector
        // (ADR-0042 D14).
        Agent::Cursor => ralphy_agent_cursor::locate_cursor(),
        _ => locate_program(a.cli_name()),
    }
}

#[cfg(target_os = "linux")]
fn linger() -> Option<Option<bool>> {
    let Some(user) = std::env::var("USER").ok().filter(|u| !u.is_empty()) else {
        return Some(None);
    };
    let out = std::process::Command::new("loginctl")
        .args(["show-user", &user, "--property=Linger"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    Some(match out {
        Ok(o) => parse_linger(&String::from_utf8_lossy(&o.stdout)),
        Err(e) => {
            tracing::debug!(error = %e, "loginctl is not available");
            None
        }
    })
}

#[cfg(not(target_os = "linux"))]
fn linger() -> Option<Option<bool>> {
    None
}

/// `loginctl show-user <user> --property=Linger` prints `Linger=yes` or `no`.
#[cfg(any(target_os = "linux", test))]
fn parse_linger(text: &str) -> Option<bool> {
    text.lines().find_map(|l| match l.trim() {
        "Linger=yes" => Some(true),
        "Linger=no" => Some(false),
        _ => None,
    })
}

#[cfg(test)]
mod tests;
