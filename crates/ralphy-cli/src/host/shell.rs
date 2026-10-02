//! The remote shell adapter: one pure function renders each host operation as
//! the command line `ssh` sends for the host's OS. Linux and macOS run a login
//! shell, because SSH's non-login `PATH` on macOS lacks `/usr/local/bin`
//! (ADR-0067 "Spike results"); Windows runs a plain `cmd.exe` line. Every
//! `ralphy` command prefers the binary that `ralphy host install` writes, then
//! the one on `PATH` (ADR-0067 amendment D2).

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
    Describe {
        with_token: bool,
    },
    SetName {
        name: String,
        avatar: usize,
    },
    InstallAutostart,
    EnableLinger,
    RequireTokenOn,
    Restart,
    RotateToken,
    ReadKeys {
        admin: bool,
    },
    WriteKeys {
        admin: bool,
    },
    ClearKeys {
        admin: bool,
    },
    /// Add the key line on standard input to the keys file, unless a line
    /// already holds `body`, the key's base64 field.
    AppendKey {
        admin: bool,
        body: String,
    },
    /// Write standard input to the installed binary's `.part` file and print
    /// its SHA-256.
    WriteBinary,
    /// Put the `.part` file in place, the previous binary renamed to `.old`.
    CommitBinary,
    /// A command of the installed binary itself, never the one on `PATH`.
    Installed(InstalledOp),
}

/// What `ralphy host install` asks the binary it has just written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstalledOp {
    Describe,
    InstallAutostart,
    Restart,
}

impl InstalledOp {
    fn args(self) -> &'static str {
        match self {
            InstalledOp::Describe => "daemon describe",
            InstalledOp::InstallAutostart => "daemon install",
            InstalledOp::Restart => "daemon restart",
        }
    }
}

const LINUX_PROBE: &str = r#"echo "--- host"; uname -n; echo "--- uid"; id -u; echo "--- user"; id -un; echo "--- arch"; uname -m; echo "--- linger"; loginctl show-user "$(id -un)" --property=Linger"#;

const MACOS_PROBE: &str = r#"echo "--- host"; uname -n; echo "--- uid"; id -u; echo "--- user"; id -un; echo "--- arch"; uname -m; echo "--- filevault"; fdesetup isactive; echo "--- autologin"; defaults read /Library/Preferences/com.apple.loginwindow autoLoginUser; echo "--- pmset"; pmset -g"#;

const WINDOWS_PROBE: &str = r#"echo --- host & hostname & echo --- user & echo %USERNAME% & echo --- groups & whoami /groups & echo --- arch & echo %PROCESSOR_ARCHITECTURE% & echo --- autologon & reg query "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon" /v AutoAdminLogon & echo --- standby & powercfg /q SCHEME_CURRENT SUB_SLEEP STANDBYIDLE"#;

/// Where `ralphy host install` puts the binary, in each shell's own words. It
/// is under the home folder, so writing it needs no administrator.
const UNIX_INSTALLED: &str = r#""$HOME/.ralphy/bin/ralphy""#;
const WINDOWS_INSTALLED: &str = r#""%USERPROFILE%\.ralphy\bin\ralphy.exe""#;

/// The folder of the installed binary, as the operator types it.
pub(crate) fn installed_folder(os: HostOs) -> &'static str {
    match os {
        HostOs::Windows => r"%USERPROFILE%\.ralphy\bin",
        _ => "~/.ralphy/bin",
    }
}

/// `ralphy <args>` for the host: the installed binary when it exists, else the
/// one on `PATH`. `args` is fixed text or words already quoted for that shell.
fn ralphy(os: HostOs, args: &str) -> String {
    match os {
        HostOs::Windows => format!(
            "if exist {WINDOWS_INSTALLED} ({WINDOWS_INSTALLED} {args}) else (ralphy {args})"
        ),
        _ => format!(
            "if [ -x {UNIX_INSTALLED} ]; then {UNIX_INSTALLED} {args}; else ralphy {args}; fi"
        ),
    }
}

// The copy reads standard input to its end, so the file holds exactly the bytes
// sent; `findstr` would treat them as text lines. The script has no `"` or `%`,
// which cmd.exe cannot pass inside the quoted argument.
const WINDOWS_WRITE_BINARY: &str = "powershell -NoProfile -NonInteractive -Command \"$d = Join-Path $env:USERPROFILE '.ralphy\\bin'; New-Item -ItemType Directory -Force -Path $d | Out-Null; $p = Join-Path $d 'ralphy.exe.part'; $f = [IO.File]::Create($p); [Console]::OpenStandardInput().CopyTo($f); $f.Close(); (Get-FileHash -Algorithm SHA256 -LiteralPath $p).Hash\"";

const UNIX_WRITE_BINARY: &str = r#"mkdir -p "$HOME/.ralphy/bin" && cat > "$HOME/.ralphy/bin/ralphy.part" && { sha256sum "$HOME/.ralphy/bin/ralphy.part" 2>/dev/null || shasum -a 256 "$HOME/.ralphy/bin/ralphy.part"; }"#;

// A running binary cannot be overwritten on Windows, but it can be renamed.
const WINDOWS_COMMIT_BINARY: &str = r#"cd /d "%USERPROFILE%\.ralphy\bin" && (if exist ralphy.exe.old del /f /q ralphy.exe.old) && (if exist ralphy.exe move /y ralphy.exe ralphy.exe.old) && move /y ralphy.exe.part ralphy.exe"#;

const UNIX_COMMIT_BINARY: &str = r#"cd "$HOME/.ralphy/bin" && { if [ -e ralphy ]; then mv -f ralphy ralphy.old; fi; } && chmod +x ralphy.part && mv -f ralphy.part ralphy"#;

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

// The line comes on standard input; the body is checked as base64 before it
// is written into the script. The shared administrators' file is ignored by
// sshd unless only Administrators and SYSTEM can write it, so its ACL is set
// every time.
const WINDOWS_APPEND_KEY: &str = "powershell -NoProfile -NonInteractive -Command \"$ErrorActionPreference = 'Stop'; $p = PATH; New-Item -ItemType Directory -Force -Path (Split-Path $p) | Out-Null; $l = [Console]::In.ReadToEnd().Trim(); $t = ''; if (Test-Path -LiteralPath $p) { $t = [IO.File]::ReadAllText($p) }; if (-not $t.Contains('BODY')) { if ($t.Length -gt 0 -and -not $t.EndsWith([string][char]10)) { $t += [string][char]13 + [char]10 }; [IO.File]::WriteAllText($p, $t + $l + [char]13 + [char]10) }ACL\"";

const WINDOWS_ADMIN_ACL: &str =
    "; icacls $p /inheritance:r /grant '*S-1-5-32-544:F' /grant '*S-1-5-18:F' | Out-Null";

// `tail -c 1` is empty after command substitution when the file ends with a
// line break, so one is added only when it is missing.
const UNIX_APPEND_KEY: &str = r#"umask 077 && mkdir -p .ssh && chmod 700 .ssh && touch .ssh/authorized_keys && chmod 600 .ssh/authorized_keys && if grep -qF BODY .ssh/authorized_keys; then cat > /dev/null; else { if [ -s .ssh/authorized_keys ] && [ -n "$(tail -c 1 .ssh/authorized_keys)" ]; then echo; fi; cat; } >> .ssh/authorized_keys; fi"#;

/// Whether `body` is a key's base64 field, so it can go into a script as it is.
fn is_key_body(body: &str) -> bool {
    !body.is_empty()
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c))
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
            let args = if *with_token {
                "daemon describe --with-token"
            } else {
                "daemon describe"
            };
            ralphy(os, args)
        }
        HostOp::SetName { name, avatar } => {
            let name = if windows {
                quote_cmd(name)?
            } else {
                quote_posix(name)
            };
            ralphy(os, &format!("daemon setup --name {name} --avatar {avatar}"))
        }
        HostOp::InstallAutostart => ralphy(os, "daemon install"),
        HostOp::EnableLinger => {
            if os != HostOs::Linux {
                bail!("lingering exists only on Linux");
            }
            "loginctl enable-linger".to_string()
        }
        HostOp::RequireTokenOn => ralphy(os, "daemon require-token on"),
        HostOp::Restart => ralphy(os, "daemon restart"),
        HostOp::RotateToken => ralphy(os, "daemon rotate-token"),
        HostOp::ReadKeys { admin } => {
            let path = keys_path(os, *admin);
            if windows {
                let path = quote_cmd(path)?;
                format!("if exist {path} type {path}")
            } else {
                let path = quote_posix(path);
                format!("if [ -e {path} ]; then cat {path}; fi")
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
        // `findstr` exits 1 on empty input, so an empty file is written apart.
        HostOp::ClearKeys { admin } => {
            let path = keys_path(os, *admin);
            if windows {
                format!("type nul > {}", quote_cmd(path)?)
            } else {
                format!(": > {}", quote_posix(path))
            }
        }
        HostOp::AppendKey { admin, body } => {
            if !is_key_body(body) {
                bail!("{body:?} is not the base64 field of a key");
            }
            if windows {
                let (path, acl) = if *admin {
                    (format!("'{}'", keys_path(os, true)), WINDOWS_ADMIN_ACL)
                } else {
                    (
                        r"Join-Path $env:USERPROFILE '.ssh\authorized_keys'".to_string(),
                        "",
                    )
                };
                WINDOWS_APPEND_KEY
                    .replace("PATH", &format!("({path})"))
                    .replace("BODY", body)
                    .replace("ACL", acl)
            } else {
                UNIX_APPEND_KEY.replace("BODY", body)
            }
        }
        HostOp::WriteBinary if windows => WINDOWS_WRITE_BINARY.to_string(),
        HostOp::WriteBinary => UNIX_WRITE_BINARY.to_string(),
        HostOp::CommitBinary if windows => WINDOWS_COMMIT_BINARY.to_string(),
        HostOp::CommitBinary => UNIX_COMMIT_BINARY.to_string(),
        HostOp::Installed(op) if windows => format!("{WINDOWS_INSTALLED} {}", op.args()),
        HostOp::Installed(op) => format!("{UNIX_INSTALLED} {}", op.args()),
    };
    // The keys file and the binary need no PATH, and a login shell may print a
    // banner that would end up inside the keys file or before the hash.
    let keys = matches!(
        op,
        HostOp::ReadKeys { .. }
            | HostOp::WriteKeys { .. }
            | HostOp::ClearKeys { .. }
            | HostOp::AppendKey { .. }
            | HostOp::WriteBinary
            | HostOp::CommitBinary
    );
    Ok(match os {
        HostOs::Linux | HostOs::MacOs if keys => format!("sh -c {}", quote_posix(&script)),
        HostOs::Linux => format!("sh -lc {}", quote_posix(&script)),
        HostOs::MacOs => format!("zsh -lc {}", quote_posix(&script)),
        HostOs::Windows => script,
    })
}

#[cfg(test)]
mod tests;
