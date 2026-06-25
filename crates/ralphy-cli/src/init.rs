//! `ralphy init`: deterministic environment gate — validates all prerequisites
//! for running ralphy on a repo (ADR-0012 stage 1). Stages 2–10 are stubbed.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{bail, Result};
use clap::Args;
use ralphy_adapter_support::find_program;
use ralphy_core::git;

#[derive(Args)]
pub struct InitArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    Opencode,
}

impl Agent {
    pub const ALL: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::Opencode];

    pub fn cli_name(&self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Opencode => "opencode",
        }
    }
}

pub struct EnvFindings {
    pub python: bool,
    pub gh_authenticated: bool,
    pub github_remote: bool,
    pub agents_present: Vec<Agent>,
    pub agents_logged_in: Vec<Agent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HardFail {
    MissingPython,
    GhNotAuthenticated,
    NoGithubRemote,
    NoAgentCli,
    NoAgentLoggedIn,
}

/// Pure gate evaluation: returns all hard failures given the environment findings.
/// The agent-login rule fires only when ≥1 agent is present.
pub fn evaluate_gate(f: &EnvFindings) -> Vec<HardFail> {
    let mut fails = Vec::new();
    if !f.python {
        fails.push(HardFail::MissingPython);
    }
    if !f.gh_authenticated {
        fails.push(HardFail::GhNotAuthenticated);
    }
    if !f.github_remote {
        fails.push(HardFail::NoGithubRemote);
    }
    if f.agents_present.is_empty() {
        fails.push(HardFail::NoAgentCli);
    } else if f.agents_logged_in.is_empty() {
        fails.push(HardFail::NoAgentLoggedIn);
    }
    fails
}

/// Pure report formatter. Produces a human-readable string with one line per
/// prerequisite and a summary. The substrings `"<name>: logged in"` and
/// `"<name>: not logged in"` are guaranteed for present agents so tests can
/// assert them literally.
pub fn format_report(f: &EnvFindings, fails: &[HardFail]) -> String {
    let mut out = String::new();

    let py = if f.python { "ok" } else { "MISSING" };
    out.push_str(&format!("python:        {py}\n"));

    let gh = if f.gh_authenticated {
        "ok"
    } else {
        "NOT AUTHENTICATED"
    };
    out.push_str(&format!("gh auth:       {gh}\n"));

    let remote = if f.github_remote {
        "ok"
    } else {
        "NO GITHUB REMOTE"
    };
    out.push_str(&format!("github remote: {remote}\n"));

    out.push_str("agents:\n");
    for agent in &Agent::ALL {
        let name = agent.cli_name();
        let present = f.agents_present.contains(agent);
        if present {
            let logged_in = f.agents_logged_in.contains(agent);
            if logged_in {
                out.push_str(&format!("  {name}: logged in\n"));
            } else {
                out.push_str(&format!("  {name}: not logged in\n"));
            }
        } else {
            out.push_str(&format!("  {name}: absent\n"));
        }
    }

    let blocker_count = fails.len();
    if blocker_count == 0 {
        out.push_str("result: all checks passed\n");
    } else {
        out.push_str(&format!("result: {blocker_count} blocker(s)\n"));
    }
    out
}

// ── impure probes ──────────────────────────────────────────────────────────

fn python_present() -> bool {
    let path = std::env::var_os("PATH");
    let pathext = std::env::var_os("PATHEXT");
    find_program("python", path.clone(), pathext.clone()).is_some()
        || find_program("python3", path, pathext).is_some()
}

fn gh_authenticated() -> bool {
    std::process::Command::new("gh")
        .args(["auth", "status"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn github_remote(repo: &Path) -> bool {
    git::origin_url(repo)
        .map(|url| url.contains("github.com"))
        .unwrap_or(false)
}

fn agent_present(a: &Agent) -> bool {
    let path = std::env::var_os("PATH");
    let pathext = std::env::var_os("PATHEXT");
    find_program(a.cli_name(), path, pathext).is_some()
}

fn agent_logged_in(a: &Agent) -> bool {
    let hello = "hello";
    let mut cmd = match a {
        Agent::Claude => {
            let mut c = std::process::Command::new("claude");
            c.args(["-p", hello]);
            c
        }
        Agent::Codex => {
            let mut c = std::process::Command::new("codex");
            c.args(["exec", hello]);
            c.env_remove("OPENAI_API_KEY");
            c
        }
        Agent::Opencode => {
            let mut c = std::process::Command::new("opencode");
            c.args(["run", hello]);
            c
        }
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn run(args: &InitArgs) -> Result<()> {
    let repo = git::resolve_toplevel(&args.repo)?;

    let agents_present: Vec<Agent> = Agent::ALL.iter().copied().filter(agent_present).collect();

    let agents_logged_in: Vec<Agent> = agents_present
        .iter()
        .copied()
        .filter(agent_logged_in)
        .collect();

    let findings = EnvFindings {
        python: python_present(),
        gh_authenticated: gh_authenticated(),
        github_remote: github_remote(&repo),
        agents_present,
        agents_logged_in,
    };

    let fails = evaluate_gate(&findings);
    print!("{}", format_report(&findings, &fails));

    if fails.is_empty() {
        println!("Environment gate passed. (Further init stages not yet implemented.)");
        Ok(())
    } else {
        bail!(
            "ralphy init: environment gate failed ({} blocker(s)) — see report above",
            fails.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_green() -> EnvFindings {
        EnvFindings {
            python: true,
            gh_authenticated: true,
            github_remote: true,
            agents_present: vec![Agent::Claude, Agent::Codex],
            agents_logged_in: vec![Agent::Claude],
        }
    }

    // (a) All-green: evaluate_gate returns empty vec when ≥1 agent is logged in.
    #[test]
    fn evaluate_gate_all_green_returns_empty() {
        assert!(evaluate_gate(&all_green()).is_empty());
    }

    // (b) Missing python.
    #[test]
    fn evaluate_gate_missing_python() {
        let f = EnvFindings {
            python: false,
            ..all_green()
        };
        let fails = evaluate_gate(&f);
        assert!(fails.contains(&HardFail::MissingPython));
    }

    // (c) gh not authenticated.
    #[test]
    fn evaluate_gate_gh_not_authenticated() {
        let f = EnvFindings {
            gh_authenticated: false,
            ..all_green()
        };
        let fails = evaluate_gate(&f);
        assert!(fails.contains(&HardFail::GhNotAuthenticated));
    }

    // (d) No github remote.
    #[test]
    fn evaluate_gate_no_github_remote() {
        let f = EnvFindings {
            github_remote: false,
            ..all_green()
        };
        let fails = evaluate_gate(&f);
        assert!(fails.contains(&HardFail::NoGithubRemote));
    }

    // (e) No agent CLI present.
    #[test]
    fn evaluate_gate_no_agent_cli() {
        let f = EnvFindings {
            agents_present: vec![],
            agents_logged_in: vec![],
            ..all_green()
        };
        let fails = evaluate_gate(&f);
        assert!(fails.contains(&HardFail::NoAgentCli));
    }

    // (f) Two agents present, none logged in → NoAgentLoggedIn.
    #[test]
    fn evaluate_gate_agents_present_none_logged_in() {
        let f = EnvFindings {
            agents_present: vec![Agent::Claude, Agent::Codex],
            agents_logged_in: vec![],
            ..all_green()
        };
        let fails = evaluate_gate(&f);
        assert!(fails.contains(&HardFail::NoAgentLoggedIn));
        assert!(!fails.contains(&HardFail::NoAgentCli));
    }

    // (g) Two present, one logged in → empty vec (≥1 passes rule).
    #[test]
    fn evaluate_gate_one_of_two_logged_in_passes() {
        let f = EnvFindings {
            agents_present: vec![Agent::Claude, Agent::Codex],
            agents_logged_in: vec![Agent::Codex],
            ..all_green()
        };
        assert!(evaluate_gate(&f).is_empty());
    }

    // (h) format_report literal substring assertions.
    #[test]
    fn format_report_logged_in_and_not_logged_in_substrings() {
        let f = EnvFindings {
            python: true,
            gh_authenticated: true,
            github_remote: true,
            agents_present: vec![Agent::Claude, Agent::Codex],
            agents_logged_in: vec![Agent::Claude],
        };
        let fails = evaluate_gate(&f);
        let report = format_report(&f, &fails);
        assert!(
            report.contains("claude: logged in"),
            "expected 'claude: logged in' in:\n{report}"
        );
        assert!(
            report.contains("codex: not logged in"),
            "expected 'codex: not logged in' in:\n{report}"
        );
    }
}
