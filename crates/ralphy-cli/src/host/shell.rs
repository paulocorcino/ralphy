//! The remote shell adapter: one pure function renders each host operation as
//! the command line `ssh` sends for the host's OS. Linux and macOS run a login
//! shell, because SSH's non-login `PATH` on macOS lacks `/usr/local/bin`
//! (ADR-0067 "Spike results"); Windows runs a plain `cmd.exe` line.

use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostOs {
    Linux,
    MacOs,
    Windows,
}

impl HostOs {
    pub(crate) fn label(self) -> &'static str {
        match self {
            HostOs::Linux => "Linux",
            HostOs::MacOs => "macOS",
            HostOs::Windows => "Windows",
        }
    }
}

/// One thing Ralphy asks a host to do or to tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostOp {
    Uname,
    WindowsVer,
    Probe,
    Describe { with_token: bool },
    SetName { name: String, avatar: usize },
    InstallAutostart,
    EnableLinger,
    RequireTokenOn,
    Restart,
    RotateToken,
    ReadKeys { admin: bool },
    WriteKeys { admin: bool },
}

const LINUX_PROBE: &str = r#"echo "--- uid"; id -u; echo "--- user"; id -un; echo "--- linger"; loginctl show-user "$(id -un)" --property=Linger"#;

const MACOS_PROBE: &str = r#"echo "--- uid"; id -u; echo "--- filevault"; fdesetup isactive; echo "--- autologin"; defaults read /Library/Preferences/com.apple.loginwindow autoLoginUser; echo "--- pmset"; pmset -g"#;

const WINDOWS_PROBE: &str = r#"echo --- groups & whoami /groups & echo --- autologon & reg query "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon" /v AutoAdminLogon & echo --- standby & powercfg /q SCHEME_CURRENT SUB_SLEEP STANDBYIDLE"#;

/// One word for a POSIX shell: bare when every character is safe, else single
/// quotes with each `'` written as `'\''`.
pub(crate) fn quote_posix(word: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "_./:=@%+,-".contains(c);
    if !word.is_empty() && word.chars().all(safe) {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// One word for `cmd.exe`. cmd has no reliable escape for `"`, `%`, `^` or a
/// line break, so a word that holds one is refused; every word Ralphy sends is
/// a validated name or a fixed path.
pub(crate) fn quote_cmd(word: &str) -> Result<String> {
    if let Some(c) = word.chars().find(|c| "\"%^\r\n".contains(*c)) {
        bail!("cannot pass {word:?} to cmd.exe: it contains {c:?}");
    }
    if word.is_empty() || word.chars().any(|c| c == ' ' || "&|<>(),;=".contains(c)) {
        return Ok(format!("\"{word}\""));
    }
    Ok(word.to_string())
}

/// The authorized-keys file, relative to the SSH start folder (the home
/// folder). OpenSSH on Windows reads an administrator's keys from one shared
/// file instead.
pub(crate) fn keys_path(os: HostOs, admin: bool) -> &'static str {
    match (os, admin) {
        (HostOs::Windows, true) => r"C:\ProgramData\ssh\administrators_authorized_keys",
        (HostOs::Windows, false) => r".ssh\authorized_keys",
        _ => ".ssh/authorized_keys",
    }
}

/// The command line for `op` on a host running `os`. `Uname` and `WindowsVer`
/// detect the OS, so they need none.
pub(crate) fn render(os: Option<HostOs>, op: &HostOp) -> Result<String> {
    match op {
        HostOp::Uname => return Ok("uname -s".to_string()),
        HostOp::WindowsVer => return Ok("cmd /c ver".to_string()),
        _ => {}
    }
    let Some(os) = os else {
        bail!("the host's operating system is not known yet");
    };
    let windows = os == HostOs::Windows;
    let script = match op {
        HostOp::Uname | HostOp::WindowsVer => unreachable!("answered above"),
        HostOp::Probe => match os {
            HostOs::Linux => LINUX_PROBE.to_string(),
            HostOs::MacOs => MACOS_PROBE.to_string(),
            HostOs::Windows => WINDOWS_PROBE.to_string(),
        },
        HostOp::Describe { with_token } => {
            let mut s = "ralphy daemon describe".to_string();
            if *with_token {
                s.push_str(" --with-token");
            }
            s
        }
        HostOp::SetName { name, avatar } => {
            let name = if windows {
                quote_cmd(name)?
            } else {
                quote_posix(name)
            };
            format!("ralphy daemon setup --name {name} --avatar {avatar}")
        }
        HostOp::InstallAutostart => "ralphy daemon install".to_string(),
        HostOp::EnableLinger => {
            if os != HostOs::Linux {
                bail!("lingering exists only on Linux");
            }
            "loginctl enable-linger".to_string()
        }
        HostOp::RequireTokenOn => "ralphy daemon require-token on".to_string(),
        HostOp::Restart => "ralphy daemon restart".to_string(),
        HostOp::RotateToken => "ralphy daemon rotate-token".to_string(),
        HostOp::ReadKeys { admin } => {
            let path = keys_path(os, *admin);
            if windows {
                format!("type {}", quote_cmd(path)?)
            } else {
                format!("cat {}", quote_posix(path))
            }
        }
        HostOp::WriteKeys { admin } => {
            let path = keys_path(os, *admin);
            if windows {
                format!("findstr \"^\" > {}", quote_cmd(path)?)
            } else {
                format!("cat > {}", quote_posix(path))
            }
        }
    };
    Ok(match os {
        HostOs::Linux => format!("sh -lc {}", quote_posix(&script)),
        HostOs::MacOs => format!("zsh -lc {}", quote_posix(&script)),
        HostOs::Windows => script,
    })
}

#[cfg(test)]
mod tests;
