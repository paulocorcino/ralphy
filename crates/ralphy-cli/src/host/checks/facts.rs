//! The facts the probe printed, read into [`HostFacts`].

use super::super::shell::HostOs;

/// What the probe found on the host. `None` means the probe printed nothing
/// readable for that fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostFacts {
    pub os: HostOs,
    pub uid: Option<u32>,
    pub user: Option<String>,
    /// What `uname -n` or `hostname` printed.
    pub host: Option<String>,
    /// What `uname -m` or `PROCESSOR_ARCHITECTURE` printed.
    pub arch: Option<String>,
    pub linger: Option<bool>,
    pub filevault: Option<bool>,
    pub autologin: Option<bool>,
    pub sleeps: Option<bool>,
    pub windows_admin: bool,
    /// `git --version` answered.
    pub git: Option<bool>,
    /// `gh --version` answered.
    pub gh: Option<bool>,
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
        host: None,
        arch: None,
        linger: None,
        filevault: None,
        autologin: None,
        sleeps: None,
        windows_admin: false,
        git: None,
        gh: None,
    };
    for (name, lines) in sections(probe_stdout) {
        read_section(&mut facts, name, &lines);
    }
    facts
}

/// Read one `--- <name>` section of the probe into `facts`. An unknown
/// section is skipped.
fn read_section(facts: &mut HostFacts, name: &str, lines: &[&str]) {
    let first = lines.first().copied();
    match name {
        "uid" => facts.uid = first.and_then(|l| l.parse().ok()),
        "user" => facts.user = first.map(str::to_string),
        "host" => facts.host = first.map(str::to_string),
        "arch" => facts.arch = first.map(str::to_string),
        "linger" => facts.linger = linger_flag(lines),
        "filevault" => facts.filevault = filevault_flag(lines),
        "autologin" => facts.autologin = Some(lines.iter().any(|l| !l.contains("does not exist"))),
        "pmset" => facts.sleeps = pmset_sleeps(lines),
        // A section with no version line: the command is not installed.
        "git" => facts.git = Some(lines.iter().any(|l| l.starts_with("git version"))),
        "gh" => facts.gh = Some(lines.iter().any(|l| l.starts_with("gh version"))),
        "groups" => facts.windows_admin = lines.iter().any(|l| l.contains("S-1-5-32-544")),
        "autologon" => {
            facts.autologin = Some(
                lines
                    .iter()
                    .any(|l| l.contains("REG_SZ") && l.ends_with('1')),
            );
        }
        "standby" => facts.sleeps = standby_sleeps(lines),
        _ => {}
    }
}

/// The `Linger=` line of `loginctl show-user`.
fn linger_flag(lines: &[&str]) -> Option<bool> {
    lines.iter().find_map(|l| match *l {
        "Linger=yes" => Some(true),
        "Linger=no" => Some(false),
        _ => None,
    })
}

/// The `true`/`false` line of `fdesetup isactive`.
fn filevault_flag(lines: &[&str]) -> Option<bool> {
    lines.iter().find_map(|l| match *l {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    })
}

/// Whether the macOS `pmset -g` `sleep` minutes are above 0.
fn pmset_sleeps(lines: &[&str]) -> Option<bool> {
    lines.iter().find_map(|l| {
        let minutes = l.strip_prefix("sleep ")?.split_whitespace().next()?;
        minutes.parse::<u32>().ok().map(|m| m > 0)
    })
}

/// Whether the Windows AC standby timeout of `powercfg /q` is not 0.
fn standby_sleeps(lines: &[&str]) -> Option<bool> {
    lines.iter().find_map(|l| {
        let hex = l.strip_prefix("Current AC Power Setting Index: 0x")?;
        u32::from_str_radix(hex.trim(), 16).ok().map(|v| v != 0)
    })
}
