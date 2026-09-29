//! Host checks (ADR-0067 §11): a pure evaluator from what the probe and
//! `ralphy daemon describe` printed to an ordered list. Each check passes, is
//! fixed by `ralphy host add`, or gives the operator a command to copy.

use std::io::Write;

use anyhow::{bail, Result};
use ralphy_daemon::identity::validate_name;
use ralphy_daemon::peer::{DaemonDescription, PEER_PROTOCOL_VERSION};

use super::shell::{keys_path, HostOp, HostOs};
use super::ssh::HostOutput;

/// What the probe found on the host. `None` means the probe printed nothing
/// readable for that fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostFacts {
    pub os: HostOs,
    pub uid: Option<u32>,
    pub user: Option<String>,
    pub linger: Option<bool>,
    pub filevault: Option<bool>,
    pub autologin: Option<bool>,
    pub sleeps: Option<bool>,
    pub windows_admin: bool,
}

/// The probe's output split into its `--- <name>` sections.
fn sections(stdout: &str) -> Vec<(&str, Vec<&str>)> {
    let mut out: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in stdout.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix("--- ") {
            out.push((name.trim(), Vec::new()));
        } else if let Some((_, lines)) = out.last_mut() {
            if !line.is_empty() {
                lines.push(line);
            }
        }
    }
    out
}

pub(crate) fn parse_facts(os: HostOs, probe_stdout: &str) -> HostFacts {
    let mut facts = HostFacts {
        os,
        uid: None,
        user: None,
        linger: None,
        filevault: None,
        autologin: None,
        sleeps: None,
        windows_admin: false,
    };
    for (name, lines) in sections(probe_stdout) {
        let first = lines.first().copied();
        match name {
            "uid" => facts.uid = first.and_then(|l| l.parse().ok()),
            "user" => facts.user = first.map(str::to_string),
            "linger" => {
                facts.linger = lines.iter().find_map(|l| match *l {
                    "Linger=yes" => Some(true),
                    "Linger=no" => Some(false),
                    _ => None,
                })
            }
            "filevault" => {
                facts.filevault = lines.iter().find_map(|l| match *l {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                })
            }
            "autologin" => {
                facts.autologin = Some(lines.iter().any(|l| !l.contains("does not exist")))
            }
            "pmset" => {
                facts.sleeps = lines.iter().find_map(|l| {
                    let minutes = l.strip_prefix("sleep ")?.split_whitespace().next()?;
                    minutes.parse::<u32>().ok().map(|m| m > 0)
                })
            }
            "groups" => facts.windows_admin = lines.iter().any(|l| l.contains("S-1-5-32-544")),
            "autologon" => {
                facts.autologin = Some(
                    lines
                        .iter()
                        .any(|l| l.contains("REG_SZ") && l.ends_with('1')),
                )
            }
            "standby" => {
                facts.sleeps = lines.iter().find_map(|l| {
                    let hex = l.strip_prefix("Current AC Power Setting Index: 0x")?;
                    u32::from_str_radix(hex.trim(), 16).ok().map(|v| v != 0)
                })
            }
            _ => {}
        }
    }
    facts
}

/// What `ralphy daemon describe` said about Ralphy on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RalphyOnHost {
    Missing,
    Old,
    Described(DaemonDescription),
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
                _ => RalphyOnHost::Old,
            },
        );
    }
    if err.contains("unrecognized subcommand") || err.contains("unexpected argument") {
        return Ok(RalphyOnHost::Old);
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
            _ => false,
        }
    }
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
    use CheckStatus::{Copy, Fix, Pass, Pending, Warn};
    let os = facts.os;
    let mut checks = Vec::new();
    let described = match ralphy {
        RalphyOnHost::Described(d) => {
            checks.push(HostCheck::new(
                CheckId::Ralphy,
                Pass,
                format!(
                    "Ralphy is installed and speaks peer protocol {}",
                    d.protocol_version
                ),
            ));
            Some(d)
        }
        RalphyOnHost::Missing => {
            checks.push(HostCheck::new(
                CheckId::Ralphy,
                Copy(format!(
                    "download Ralphy for {} from https://github.com/paulocorcino/ralphy/releases, then run ./ralphy install",
                    os.label()
                )),
                "Ralphy is not installed on the host",
            ));
            None
        }
        RalphyOnHost::Old => {
            checks.push(HostCheck::new(
                CheckId::Ralphy,
                Copy("ralphy update".to_string()),
                "the Ralphy on the host is too old to connect to this one",
            ));
            None
        }
    };
    let pending = || (Pending, "waits for Ralphy on the host".to_string());

    let (status, text) = match described {
        None => pending(),
        Some(d) => name_check(d, fleet_names, destination, wanted_name),
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

    if os == HostOs::Linux {
        let (status, text) = match facts.linger {
            Some(true) => (
                Pass,
                "lingering is on: the daemon runs with no one signed in",
            ),
            Some(false) => (
                Fix(HostOp::EnableLinger),
                "lingering is off: the daemon stops when you sign out. Ralphy turns it on",
            ),
            None => (
                Warn,
                "could not read lingering: the daemon may stop when you sign out",
            ),
        };
        checks.push(HostCheck::new(CheckId::Linger, status, text));
    }

    if os != HostOs::Linux {
        let after_sign_in =
            "after a restart, the daemon starts only when someone signs in on the host";
        let (status, text) = match (os, facts.filevault, facts.autologin) {
            (HostOs::MacOs, Some(true), _) => (
                Warn,
                format!("{after_sign_in}. FileVault is on, and it blocks automatic sign-in"),
            ),
            (_, _, Some(true)) => (
                Pass,
                "the host signs in by itself after a restart".to_string(),
            ),
            (HostOs::Windows, _, _) => (
                Warn,
                format!("{after_sign_in}. To sign in by itself, use Autologon from Sysinternals"),
            ),
            _ => (
                Warn,
                format!("{after_sign_in}. Turn on automatic sign-in to change this"),
            ),
        };
        checks.push(HostCheck::new(CheckId::SignIn, status, text));

        let (status, text) = match facts.sleeps {
            Some(false) => (Pass, "the host does not sleep"),
            Some(true) => (
                Copy(
                    if os == HostOs::MacOs {
                        "sudo pmset -a sleep 0"
                    } else {
                        "powercfg /change standby-timeout-ac 0"
                    }
                    .to_string(),
                ),
                "the host sleeps when idle, and a sleeping host does not answer",
            ),
            None => (Warn, "could not read the sleep setting"),
        };
        checks.push(HostCheck::new(CheckId::Sleep, status, text));
    }

    let (status, text) = match described {
        None => pending(),
        Some(d) if d.require_token => (Pass, "the daemon asks for its access token".to_string()),
        Some(_) => (
            Fix(HostOp::RequireTokenOn),
            "the daemon does not ask for its access token on this computer's tunnel: Ralphy turns this on".to_string(),
        ),
    };
    checks.push(HostCheck::new(CheckId::RequireToken, status, text));

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
    checks.push(HostCheck::new(CheckId::User, status, text));
    checks
}

fn name_check(
    d: &DaemonDescription,
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
            None => (
                CheckStatus::Copy(format!("ralphy host add {destination} --name <name>")),
                "the daemon on the host has no name".to_string(),
            ),
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

#[cfg(test)]
pub(crate) mod tests;
