//! `ralphy host aliases`: the hosts named in the operator's SSH config, each
//! resolved by `ssh -G`, so the workbench can offer them. `Include` lines are
//! not followed; the operator can still type any alias.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::json;

/// The concrete host names of `Host` lines, in order and without duplicates.
/// A pattern (`*`, `?`) or a negation (`!`) names no one host, and `Match`
/// blocks have no name at all.
pub(crate) fn parse_config_hosts(text: &str) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let (keyword, rest) = match line.split_once(|c: char| c == '=' || c.is_whitespace()) {
            Some((k, rest)) => (k, rest),
            None => continue,
        };
        if !keyword.eq_ignore_ascii_case("host") {
            continue;
        }
        let rest = rest.trim_start_matches(|c: char| c == '=' || c.is_whitespace());
        for word in rest.split_whitespace() {
            if word.starts_with('#') {
                break;
            }
            if word.contains(['*', '?', '!']) || hosts.iter().any(|h| h == word) {
                continue;
            }
            hosts.push(word.to_string());
        }
    }
    hosts
}

/// What `ssh -G` says it would use for a destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub hostname: String,
    pub user: String,
    pub port: u16,
    pub known_hosts: Vec<String>,
    /// A `ProxyJump` or `ProxyCommand` stands between this computer and the
    /// host, so a direct connection cannot read its key.
    pub proxied: bool,
}

/// `path` with a leading `~` replaced by `home`.
pub(crate) fn expand_home(path: &str, home: Option<&Path>) -> String {
    match (path.strip_prefix('~'), home) {
        (Some(rest), Some(home)) => {
            let rest = rest.trim_start_matches(['/', '\\']);
            home.join(rest).display().to_string()
        }
        _ => path.to_string(),
    }
}

pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Split the `userknownhostsfile` value of `ssh -G`. It prints the paths
/// unquoted, so a space inside a path looks like a separator (measured with
/// OpenSSH_for_Windows 9.5p2: `C:/Temp/sp ace/kh C:\Users\me/.ssh/known_hosts`).
/// A new path starts with `/`, `~`, `\`, `%` or a drive letter; any other word
/// continues the path before it.
pub(crate) fn split_known_hosts(value: &str) -> Vec<String> {
    let starts_path = |w: &str| {
        let b = w.as_bytes();
        w.starts_with(['/', '~', '\\', '%'])
            || (b.len() >= 3
                && b[0].is_ascii_alphabetic()
                && b[1] == b':'
                && (b[2] == b'/' || b[2] == b'\\'))
    };
    let mut paths: Vec<String> = Vec::new();
    for word in value.split_whitespace() {
        match paths.last_mut() {
            Some(last) if !starts_path(word) => {
                last.push(' ');
                last.push_str(word);
            }
            _ => paths.push(word.to_string()),
        }
    }
    paths
}

/// Read `ssh -G` output: one lowercase keyword and its value per line.
pub(crate) fn parse_ssh_g(text: &str, home: Option<&Path>) -> Result<Resolved> {
    let mut hostname = None;
    let mut user = None;
    let mut port = None;
    let mut known_hosts = Vec::new();
    let mut proxied = false;
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once(' ') else {
            continue;
        };
        let value = value.trim();
        match key.to_ascii_lowercase().as_str() {
            "hostname" => hostname = Some(value.to_string()),
            "user" => user = Some(value.to_string()),
            "port" => port = value.parse::<u16>().ok(),
            "userknownhostsfile" => known_hosts.extend(
                split_known_hosts(value)
                    .iter()
                    .map(|p| expand_home(p, home)),
            ),
            "proxyjump" | "proxycommand" => proxied |= value != "none",
            _ => {}
        }
    }
    let (Some(hostname), Some(port)) = (hostname, port) else {
        bail!("`ssh -G` printed no hostname or port");
    };
    Ok(Resolved {
        hostname,
        user: user.unwrap_or_default(),
        port,
        known_hosts,
        proxied,
    })
}

/// Ask `ssh` how it would reach `dest`. Nothing connects.
pub(crate) fn resolve(ssh: &Path, dest: &str) -> Result<Resolved> {
    let mut cmd = Command::new(ssh);
    cmd.args(["-G", "--", dest]);
    ralphy_proc_util::no_window(&mut cmd);
    let out = cmd
        .output()
        .with_context(|| format!("starting {}", ssh.display()))?;
    if !out.status.success() {
        bail!(
            "`ssh -G {dest}` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    parse_ssh_g(&String::from_utf8_lossy(&out.stdout), home_dir().as_deref())
}

/// The JSON array `ralphy host aliases` prints. An alias `ssh -G` cannot
/// resolve is left out.
pub(crate) fn list(ssh: &Path) -> Result<serde_json::Value> {
    let Some(home) = home_dir() else {
        return Ok(json!([]));
    };
    let config = home.join(".ssh").join("config");
    let text = match std::fs::read_to_string(&config) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!([])),
        Err(e) => return Err(e).with_context(|| format!("reading {}", config.display())),
    };
    Ok(rows(&text, |alias| resolve(ssh, alias)))
}

/// The `host.aliases` reply: one row per `Host` alias of `text` that
/// `resolve` can read; an alias it cannot read is skipped.
fn rows(text: &str, resolve: impl Fn(&str) -> Result<Resolved>) -> serde_json::Value {
    let aliases: Vec<serde_json::Value> = parse_config_hosts(text)
        .into_iter()
        .filter_map(|alias| match resolve(&alias) {
            Ok(r) => Some(
                json!({"alias": alias, "hostname": r.hostname, "user": r.user, "port": r.port}),
            ),
            // Stderr, not stdout: stdout is the JSON the workbench reads.
            Err(e) => {
                eprintln!("skipped the SSH config host {alias}: {e:#}");
                None
            }
        })
        .collect();
    serde_json::Value::Array(aliases)
}

#[cfg(test)]
mod tests;
