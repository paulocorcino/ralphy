//! The probes: what programs this computer has, found by running them.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use ralphy_adapter_support::{find_program, locate_program, run_headless};
use ralphy_daemon::agent_state::StateSource;
use ralphy_daemon::telegram_state::{self, FleetTelegram};
use ralphy_proc_util::hook_form::{self, HookCheck};

use super::{gh_signed_in, source_of, MachineFacts};
use crate::init::Agent;

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
        let running = scope.spawn(daemon_telegram);
        let linger = linger();
        // A probe thread only runs a program and reads its output; a panic in
        // one is a bug in that code, not a fact about the computer.
        let join = "a doctor probe does not panic";
        // The running daemon knows whether a main daemon sends for it; with
        // no daemon, the config it would read says it all.
        let telegram = running.join().expect(join).or_else(|| {
            telegram_state::config_path()
                .ok()
                .map(|path| FleetTelegram::alone(telegram_state::state_at(&path)))
        });
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
            telegram,
        }
    })
}

/// How long `doctor` waits for the daemon of this machine.
const DAEMON_TIMEOUT: Duration = Duration::from_secs(3);

/// How long `doctor` waits for the Telegram state of the local fleet: the
/// daemon asks its peers first, each for at most `PEER_TIMEOUT`.
const FLEET_TIMEOUT: Duration = Duration::from_secs(
    DAEMON_TIMEOUT.as_secs() + ralphy_daemon::peer::client::PEER_TIMEOUT.as_secs(),
);

/// The Telegram state of the daemon that runs on this machine and of its
/// peers, from the daemon (`GET /api/telegram`); `None` when no daemon
/// answers.
fn daemon_telegram() -> Option<FleetTelegram> {
    let store = ralphy_daemon::auth::store_dir().ok()?;
    let args = ralphy_daemon::pidfile::read_args_in(&store);
    let port = crate::daemon::describe::port_from_args(&args);
    if !crate::daemon::describe::answers(&store, port) {
        return None;
    }
    match read_daemon_telegram(port) {
        Ok(state) => Some(state),
        Err(e) => {
            tracing::debug!(
                error = %format!("{e:#}"),
                "could not read the daemon's Telegram state"
            );
            None
        }
    }
}

fn read_daemon_telegram(port: u16) -> Result<FleetTelegram> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(FLEET_TIMEOUT))
        .proxy(None)
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent.get(&format!("http://127.0.0.1:{port}/api/telegram"));
    if let Some(token) = ralphy_daemon::auth::effective_token()?.filter(|t| !t.is_empty()) {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }
    let mut resp = req
        .call()
        .context("asking the daemon for its Telegram state")?;
    let status = resp.status().as_u16();
    if status != 200 {
        anyhow::bail!("the daemon answered {status}");
    }
    resp.body_mut()
        .read_json()
        .context("reading the Telegram state")
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
pub(super) fn hook_command(agent: Agent, cli: &Path) -> Option<hook_form::HookCommand> {
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
pub(super) fn probe_output(mut cmd: std::process::Command) -> Option<String> {
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
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

// Each CLI resolves through the same locator the adapters spawn through, so a
// CLI under `~/.local/bin` but off `PATH` is found, as a run would find it.
pub(crate) fn agent_program(a: &Agent) -> Option<PathBuf> {
    match a {
        // Cursor installs as `cursor-agent` or `agent`, not as its selector
        // (ADR-0042 D14).
        Agent::Cursor => ralphy_agent_cursor::locate_cursor(),
        Agent::Claude
        | Agent::Codex
        | Agent::Copilot
        | Agent::Gemini
        | Agent::Kimi
        | Agent::Opencode => locate_program(a.cli_name()),
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
pub(super) fn parse_linger(text: &str) -> Option<bool> {
    text.lines().find_map(|l| match l.trim() {
        "Linger=yes" => Some(true),
        "Linger=no" => Some(false),
        _ => None,
    })
}
