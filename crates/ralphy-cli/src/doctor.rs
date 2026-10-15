//! `ralphy doctor`: check this computer for the tools a run needs (ADR-0056
//! §12). This module owns that list: `ralphy init` reads its probes, and
//! `ralphy host add` reads `ralphy doctor --json` on the host.
//!
//! It checks presence only. A minimum version is added only after a measured
//! failure, with that failure in a comment.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{bail, Result};
use clap::Args;
use ralphy_adapter_support::{find_program, locate_program};
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

/// The result for `facts`, one line per tool, in a fixed order.
pub(crate) fn evaluate(facts: &MachineFacts, windows: bool) -> DoctorReport {
    let mut tools = vec![git_line(facts, windows), gh_line(facts), python_line(facts)];
    tools.extend(agent_lines(facts));
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
    match report.missing_required() {
        0 => Ok(()),
        n => bail!("{n} required tool(s) missing"),
    }
}

// --- probes -----------------------------------------------------------------

/// Run every probe on this computer.
pub(crate) fn probe() -> MachineFacts {
    MachineFacts {
        git: on_path("git").map(|p| version_of(&p)),
        gh: on_path("gh").map(|p| version_of(&p)),
        gh_signed_in: gh_signed_in(),
        python: on_path("python")
            .or_else(|| on_path("python3"))
            .map(|p| version_of(&p)),
        agents: Agent::ALL
            .iter()
            .filter_map(|a| agent_program(a).map(|p| (*a, version_of(&p))))
            .collect(),
        linger: linger(),
    }
}

fn on_path(name: &str) -> Option<PathBuf> {
    find_program(name, std::env::var_os("PATH"), std::env::var_os("PATHEXT"))
}

/// The first non-empty line `<program> --version` prints, or an empty string.
fn version_of(program: &Path) -> String {
    std::process::Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_default()
}

/// `gh auth status` exits 0 only when signed in.
pub(crate) fn gh_signed_in() -> bool {
    std::process::Command::new("gh")
        .args(["auth", "status"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
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
