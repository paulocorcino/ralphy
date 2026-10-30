//! The question that comes before a daemon restart ends the open consoles.
//!
//! Every workbench console is a child of the daemon, so ending the daemon ends
//! them and stops the agents inside. `ralphy update` and `ralphy daemon
//! restart` ask first. The decision takes every input as a parameter, so no
//! test needs a daemon, a terminal, or a person.

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// How long the daemon has to list its sessions. It is on this machine, so a
/// slow answer is a stuck daemon.
const FETCH_TIMEOUT: Duration = Duration::from_secs(3);

/// One open console, as much as the question needs to name it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct OpenSession {
    pub(crate) repo: String,
    pub(crate) agent: String,
    #[serde(default)]
    pub(crate) name: Option<String>,
}

/// What this machine's daemon said about its consoles.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Sessions {
    /// No daemon is running: nothing to close.
    NoDaemon,
    /// A daemon answered with no consoles.
    None,
    Open(Vec<OpenSession>),
    /// A daemon is running, but it did not give a list.
    Unknown,
}

/// Whether the command may go on and end the daemon. `ask` gets the question
/// and returns the raw answer line; only `y` and `yes` agree.
pub(crate) fn confirm_close(
    sessions: Sessions,
    yes: bool,
    is_terminal: bool,
    ask: impl FnOnce(&str) -> Result<String>,
    out: &mut impl Write,
) -> Result<bool> {
    let sessions = match sessions {
        Sessions::NoDaemon | Sessions::None => return Ok(true),
        Sessions::Open(open) if open.is_empty() => return Ok(true),
        other @ (Sessions::Open(_) | Sessions::Unknown) => other,
    };
    if yes {
        return Ok(true);
    }
    if !is_terminal {
        bail!(
            "open consoles may be closed and nobody can answer here: run again with --yes to go on"
        );
    }
    match sessions {
        Sessions::Open(open) => {
            let (plural, them) = if open.len() == 1 {
                ("", "it")
            } else {
                ("s", "them")
            };
            writeln!(
                out,
                "{} console{plural} open on the daemon. This closes {them} and stops the agents inside:",
                open.len(),
            )?;
            for session in &open {
                writeln!(out, "  {}", label(session))?;
            }
        }
        Sessions::NoDaemon | Sessions::None | Sessions::Unknown => writeln!(
            out,
            "The open consoles could not be counted. This closes them and stops the agents inside."
        )?,
    }
    let answer = ask("Continue?")?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn label(session: &OpenSession) -> String {
    match session.name.as_deref().filter(|n| !n.is_empty()) {
        Some(name) => name.to_string(),
        None => format!("{} ({})", session.repo, session.agent),
    }
}

/// Ask the daemon of this machine for its consoles. Any failure is `Unknown`:
/// a daemon that is up but silent still gets the question.
fn fetch_sessions(store: &Path, port: u16) -> Sessions {
    if !super::describe::answers(store, port) {
        return Sessions::NoDaemon;
    }
    match read_sessions(port) {
        Ok(open) if open.is_empty() => Sessions::None,
        Ok(open) => Sessions::Open(open),
        Err(e) => {
            tracing::debug!(error = %format!("{e:#}"), "could not read the open consoles");
            Sessions::Unknown
        }
    }
}

fn read_sessions(port: u16) -> Result<Vec<OpenSession>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .proxy(None)
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent.get(&format!("http://127.0.0.1:{port}/api/sessions?local=1"));
    if let Some(token) = ralphy_daemon::auth::effective_token()?.filter(|t| !t.is_empty()) {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }
    let mut resp = req.call().context("asking the daemon for its sessions")?;
    let status = resp.status().as_u16();
    if status != 200 {
        bail!("the daemon answered {status}");
    }
    resp.body_mut()
        .read_json()
        .context("reading the session list")
}

/// The real inputs: this machine's daemon, this terminal, a prompt on stdout.
/// Returns whether to go on; on `false` it has said that nothing changed.
pub(crate) fn confirm_with_daemon(store: &Path, port: u16, yes: bool) -> Result<bool> {
    let go_on = confirm_close(
        fetch_sessions(store, port),
        yes,
        std::io::stdin().is_terminal(),
        |q| crate::init::ask_yes_no(q, false),
        &mut std::io::stdout(),
    )?;
    if !go_on {
        println!("nothing changed");
    }
    Ok(go_on)
}

/// [`confirm_with_daemon`] for the daemon of the recorded store and port.
pub(crate) fn confirm_recorded_daemon(yes: bool) -> Result<bool> {
    let store = ralphy_daemon::auth::store_dir()?;
    let args = ralphy_daemon::pidfile::read_args_in(&store);
    confirm_with_daemon(&store, super::describe::port_from_args(&args), yes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(n: usize) -> Sessions {
        Sessions::Open(
            (0..n)
                .map(|i| OpenSession {
                    repo: format!("repo{i}"),
                    agent: "claude".into(),
                    name: (i == 0).then(|| "fincal #1".to_string()),
                })
                .collect(),
        )
    }

    fn run(sessions: Sessions, yes: bool, tty: bool, answer: &str) -> (Result<bool>, String, bool) {
        let mut out = Vec::new();
        let mut asked = false;
        let r = confirm_close(
            sessions,
            yes,
            tty,
            |_| {
                asked = true;
                Ok(answer.to_string())
            },
            &mut out,
        );
        (
            r,
            String::from_utf8(out).expect("the prompt is text"),
            asked,
        )
    }

    #[test]
    fn no_daemon_and_no_sessions_go_on_without_a_question() {
        for s in [Sessions::NoDaemon, Sessions::None, Sessions::Open(vec![])] {
            let (r, out, asked) = run(s, false, false, "");
            assert!(r.expect("no question"));
            assert!(!asked && out.is_empty());
        }
    }

    #[test]
    fn open_sessions_are_listed_and_only_yes_agrees() {
        let (r, out, asked) = run(open(2), false, true, "y\n");
        assert!(r.expect("answered") && asked);
        assert!(out.contains("2 consoles open"), "{out}");
        assert!(out.contains("fincal #1"), "{out}");
        assert!(out.contains("repo1 (claude)"), "{out}");
        for no in ["", "n\n", "maybe\n", "\n"] {
            assert!(
                !run(open(1), false, true, no).0.expect("answered"),
                "{no:?}"
            );
        }
        assert!(run(open(1), false, true, " YES \n").0.expect("answered"));
    }

    #[test]
    fn yes_skips_the_question() {
        let (r, _, asked) = run(open(3), true, false, "");
        assert!(r.expect("yes") && !asked);
    }

    #[test]
    fn no_terminal_and_no_yes_is_an_error_that_names_yes() {
        let (r, _, asked) = run(open(1), false, false, "y");
        assert!(format!("{:#}", r.expect_err("must refuse")).contains("--yes"));
        assert!(!asked);
    }

    #[test]
    fn a_silent_daemon_says_so_and_still_asks() {
        let (r, out, asked) = run(Sessions::Unknown, false, true, "y");
        assert!(r.expect("answered") && asked);
        assert!(out.contains("could not be counted"), "{out}");
        assert!(!run(Sessions::Unknown, false, true, "n")
            .0
            .expect("answered"));
    }

    #[test]
    fn both_commands_show_yes_in_their_help() {
        use clap::CommandFactory;
        let cli = crate::cli::Cli::command();
        let update = cli.find_subcommand("update").expect("update is registered");
        let daemon = cli.find_subcommand("daemon").expect("daemon is registered");
        let restart = daemon
            .find_subcommand("restart")
            .expect("daemon restart is registered");
        for cmd in [update, restart] {
            let yes = cmd
                .get_arguments()
                .find(|a| a.get_long() == Some("yes"))
                .expect("--yes exists");
            assert!(!yes.is_hide_set(), "--yes must show in --help");
        }
    }
}
