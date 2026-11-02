//! Host checks (ADR-0067 §11): a pure evaluator from what the probe and
//! `ralphy daemon describe` printed to an ordered list. Each check passes, is
//! fixed by `ralphy host add`, or gives the operator a command to copy.

use std::io::Write;

use anyhow::{bail, Result};
use ralphy_daemon::identity::{default_name, validate_name};
use ralphy_daemon::peer::{DaemonDescription, PEER_PROTOCOL_VERSION};

use super::shell::{keys_path, HostOp, HostOs};
use super::ssh::HostOutput;

/// What `ralphy daemon describe` said about Ralphy on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RalphyOnHost {
    Missing,
    /// Another peer protocol; `None` when the host's Ralphy is too old to say.
    Old {
        protocol: Option<u32>,
    },
    Described(DaemonDescription),
}

impl RalphyOnHost {
    /// The host speaks a newer peer protocol than this computer, so installing
    /// this computer's version would be a downgrade.
    pub(crate) fn is_newer(&self) -> bool {
        matches!(self, RalphyOnHost::Old { protocol: Some(p) } if *p > PEER_PROTOCOL_VERSION)
    }
}

/// Read the answer to `ralphy daemon describe`. An older Ralphy does not know
/// the command, so clap refuses it; that is how an old Ralphy is recognized.
pub(crate) fn classify_describe(out: &HostOutput) -> Result<RalphyOnHost> {
    let err = &out.stderr;
    if out.ok() {
        return Ok(
            // A login shell may print a banner first; the JSON is the last line.
            match serde_json::from_str::<DaemonDescription>(
                out.stdout
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or(""),
            ) {
                Ok(d) if d.protocol_version == PEER_PROTOCOL_VERSION => RalphyOnHost::Described(d),
                Ok(d) => RalphyOnHost::Old {
                    protocol: Some(d.protocol_version),
                },
                Err(_) => RalphyOnHost::Old { protocol: None },
            },
        );
    }
    if err.contains("unrecognized subcommand") || err.contains("unexpected argument") {
        return Ok(RalphyOnHost::Old { protocol: None });
    }
    // 9009 is cmd.exe's exit code for an unknown command, in every language.
    if out.code == Some(127)
        || out.code == Some(9009)
        || err.contains("not found")
        || err.contains("is not recognized as an internal or external command")
    {
        return Ok(RalphyOnHost::Missing);
    }
    bail!(
        "`ralphy daemon describe` failed on the host: {}",
        err.trim()
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckId {
    Ralphy,
    Name,
    Autostart,
    Linger,
    SignIn,
    Sleep,
    RequireToken,
    User,
    Git,
    Gh,
    Python,
    Agent,
    Hooks,
}

impl CheckId {
    /// The stable `id` of a `check` progress event.
    pub(crate) fn key(self) -> &'static str {
        match self {
            CheckId::Ralphy => "ralphy",
            CheckId::Name => "name",
            CheckId::Autostart => "autostart",
            CheckId::Linger => "linger",
            CheckId::SignIn => "sign_in",
            CheckId::Sleep => "sleep",
            CheckId::RequireToken => "require_token",
            CheckId::User => "user",
            CheckId::Git => "git",
            CheckId::Gh => "gh",
            CheckId::Python => "python",
            CheckId::Agent => "agent",
            CheckId::Hooks => "hooks",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            CheckId::Ralphy => "Ralphy",
            CheckId::Name => "Name",
            CheckId::Autostart => "Start at boot",
            CheckId::Linger => "Lingering",
            CheckId::SignIn => "Sign-in",
            CheckId::Sleep => "Sleep",
            CheckId::RequireToken => "Access token",
            CheckId::User => "User",
            CheckId::Git => "Git",
            CheckId::Gh => "GitHub CLI",
            CheckId::Python => "Python",
            CheckId::Agent => "Agent CLI",
            CheckId::Hooks => "Agent hooks",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheckStatus {
    Pass,
    /// `ralphy host add` does this itself.
    Fix(HostOp),
    /// The operator runs this command.
    Copy(String),
    Warn,
    /// Waits for an earlier check.
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostCheck {
    pub id: CheckId,
    pub status: CheckStatus,
    pub text: String,
}

impl CheckStatus {
    /// The `status` of a `check` progress event.
    pub(crate) fn key(&self) -> &'static str {
        match self {
            CheckStatus::Pass => "pass",
            CheckStatus::Fix(_) => "fix",
            CheckStatus::Copy(_) => "copy",
            CheckStatus::Warn => "warn",
            CheckStatus::Pending => "pending",
        }
    }
}

impl HostCheck {
    fn new(id: CheckId, status: CheckStatus, text: impl Into<String>) -> Self {
        HostCheck {
            id,
            status,
            text: text.into(),
        }
    }

    /// `ralphy host add` cannot go on while this check stands.
    pub(crate) fn is_blocking(&self) -> bool {
        match self.id {
            CheckId::Ralphy => self.status != CheckStatus::Pass,
            CheckId::Name => !matches!(self.status, CheckStatus::Pass | CheckStatus::Fix(_)),
            CheckId::Autostart
            | CheckId::Linger
            | CheckId::SignIn
            | CheckId::Sleep
            | CheckId::RequireToken
            | CheckId::User
            | CheckId::Git
            | CheckId::Gh
            | CheckId::Python
            | CheckId::Agent
            | CheckId::Hooks => false,
        }
    }
}

/// What installs this computer's version on `destination`.
pub(crate) fn install_command(destination: &str) -> String {
    format!("ralphy host install {destination}")
}

/// The checks for one host, in the order they are shown and fixed.
/// `fleet_names` holds the names of every other daemon in the local fleet.
pub(crate) fn evaluate(
    facts: &HostFacts,
    ralphy: &RalphyOnHost,
    fleet_names: &[String],
    destination: &str,
    wanted_name: Option<&str>,
) -> Vec<HostCheck> {
    use CheckStatus::{Fix, Pass, Pending, Warn};
    let os = facts.os;
    let (ralphy_check, described) = ralphy_check(ralphy, destination);
    let mut checks = vec![ralphy_check];
    let pending = || (Pending, "waits for Ralphy on the host".to_string());

    let (status, text) = match described {
        None => pending(),
        Some(d) => name_check(d, facts, fleet_names, destination, wanted_name),
    };
    checks.push(HostCheck::new(CheckId::Name, status, text));

    let (status, text) = match described {
        None => pending(),
        Some(d) if d.autostart => (Pass, "the daemon starts by itself".to_string()),
        Some(_) => (
            Fix(HostOp::InstallAutostart),
            "the daemon does not start by itself: Ralphy installs its autostart".to_string(),
        ),
    };
    checks.push(HostCheck::new(CheckId::Autostart, status, text));

    checks.extend(wake_checks(facts, os));

    let (status, text) = match described {
        None => pending(),
        Some(d) if d.require_token => (Pass, "the daemon asks for its access token".to_string()),
        Some(_) => (
            Fix(HostOp::RequireTokenOn),
            "the daemon does not ask for its access token on this computer's tunnel: \
             Ralphy turns this on"
                .to_string(),
        ),
    };
    checks.push(HostCheck::new(CheckId::RequireToken, status, text));

    checks.push(user_check(facts, os));
    checks.push(git_check(facts, os));

    let (status, text) = match facts.gh {
        Some(true) => (Pass, "gh is installed"),
        Some(false) => (
            Warn,
            "gh is not installed. A run that reads or changes GitHub needs it",
        ),
        None => (Warn, "could not read whether gh is installed"),
    };
    checks.push(HostCheck::new(CheckId::Gh, status, text));
    checks
}

/// The Ralphy check, and the host's description when its Ralphy speaks this
/// computer's peer protocol.
fn ralphy_check<'a>(
    ralphy: &'a RalphyOnHost,
    destination: &str,
) -> (HostCheck, Option<&'a DaemonDescription>) {
    use CheckStatus::{Copy, Pass};
    match ralphy {
        RalphyOnHost::Described(d) => (
            HostCheck::new(
                CheckId::Ralphy,
                Pass,
                format!(
                    "Ralphy is installed and speaks peer protocol {}",
                    d.protocol_version
                ),
            ),
            Some(d),
        ),
        RalphyOnHost::Missing => (
            HostCheck::new(
                CheckId::Ralphy,
                Copy(install_command(destination)),
                "Ralphy is not installed on the host",
            ),
            None,
        ),
        old if old.is_newer() => (
            HostCheck::new(
                CheckId::Ralphy,
                Copy("ralphy update".to_string()),
                "the Ralphy on the host is newer than this one: update Ralphy on this computer",
            ),
            None,
        ),
        RalphyOnHost::Old { .. } => (
            HostCheck::new(
                CheckId::Ralphy,
                Copy(install_command(destination)),
                "the Ralphy on the host is too old to connect to this one",
            ),
            None,
        ),
    }
}

fn user_check(facts: &HostFacts, os: HostOs) -> HostCheck {
    use CheckStatus::{Pass, Warn};
    let (status, text) = if facts.uid == Some(0) {
        (Warn, "signed in as root: use a normal user".to_string())
    } else if os == HostOs::Windows && facts.windows_admin {
        (
            Pass,
            format!(
                "signed in as an administrator: a key for this user goes in {}",
                keys_path(os, true)
            ),
        )
    } else {
        (Pass, "signed in as a normal user".to_string())
    };
    HostCheck::new(CheckId::User, status, text)
}

/// Neither Git nor gh blocks: the host can be added first and get Git later.
fn git_check(facts: &HostFacts, os: HostOs) -> HostCheck {
    use CheckStatus::{Copy, Pass, Warn};
    let (status, text) = match (facts.git, os) {
        (Some(true), _) => (Pass, "Git is installed"),
        (Some(false), HostOs::MacOs) => (
            Copy("xcode-select --install".to_string()),
            "Git is not installed. Ralphy needs it for every project",
        ),
        (Some(false), HostOs::Windows) => (
            Copy("winget install --id Git.Git -e".to_string()),
            "Git is not installed. Ralphy needs it for every project",
        ),
        (Some(false), HostOs::Linux) => (
            Warn,
            "Git is not installed. Ralphy needs it for every project: install it with \
             the package manager of the host",
        ),
        (None, _) => (Warn, "could not read whether Git is installed"),
    };
    HostCheck::new(CheckId::Git, status, text)
}

fn name_check(
    d: &DaemonDescription,
    facts: &HostFacts,
    fleet_names: &[String],
    destination: &str,
    wanted_name: Option<&str>,
) -> (CheckStatus, String) {
    let taken = |n: &str| fleet_names.iter().any(|f| f == n);
    let used = |n: &str| {
        (
            CheckStatus::Copy("ralphy daemon setup --name <other-name> --avatar 1".to_string()),
            format!("another daemon in the fleet is already named {n}"),
        )
    };
    match d.name.as_deref().filter(|n| !n.is_empty()) {
        Some(n) if taken(n) => used(n),
        Some(n) => (CheckStatus::Pass, format!("the daemon is named {n}")),
        None => match wanted_name.map(validate_name) {
            None => match facts
                .host
                .as_deref()
                .and_then(|h| default_name(h, facts.user.as_deref(), fleet_names))
            {
                Some(n) => (
                    CheckStatus::Fix(HostOp::SetName {
                        name: n.clone(),
                        avatar: 1,
                    }),
                    format!("the daemon has no name: Ralphy names it {n}"),
                ),
                None => (
                    CheckStatus::Copy(format!("ralphy host add {destination} --name <name>")),
                    "the daemon on the host has no name".to_string(),
                ),
            },
            Some(Err(e)) => (
                CheckStatus::Copy(format!("ralphy host add {destination} --name <name>")),
                format!("the name is not valid: {e}"),
            ),
            Some(Ok(n)) if taken(&n) => used(&n),
            Some(Ok(n)) => (
                CheckStatus::Fix(HostOp::SetName {
                    name: n.clone(),
                    avatar: 1,
                }),
                format!("the daemon has no name: Ralphy names it {n}"),
            ),
        },
    }
}

/// Print the checks, one line each, with the command to copy under it.
pub(crate) fn print_checks(checks: &[HostCheck], out: &mut impl Write) -> Result<()> {
    for c in checks {
        let tag = match c.status {
            CheckStatus::Pass => "ok  ",
            CheckStatus::Fix(_) => "fix ",
            CheckStatus::Copy(_) => "todo",
            CheckStatus::Warn => "warn",
            CheckStatus::Pending => "wait",
        };
        writeln!(out, "[{tag}] {}: {}", c.id.label(), c.text)?;
        if let CheckStatus::Copy(cmd) = &c.status {
            writeln!(out, "       run: {cmd}")?;
        }
    }
    Ok(())
}

mod facts;
mod tools;
mod wake;
pub(crate) use facts::{parse_facts, HostFacts};
pub(crate) use tools::{parse_doctor, with_doctor};
use wake::wake_checks;

#[cfg(test)]
pub(crate) mod tests;
