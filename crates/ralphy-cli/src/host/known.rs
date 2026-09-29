//! `ralphy host key` and `ralphy host trust`: show a host's key before the
//! first sign-in, and add exactly the key the operator saw to their
//! `known_hosts`. `ssh-keygen -F` does the lookup, so hashed entries work.

use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use data_encoding::{BASE64, BASE64_NOPAD};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::aliases::{resolve, Resolved};

const TIMEOUT: Duration = Duration::from_secs(10);

/// The `SHA256:` fingerprint of a key blob, as `ssh-keygen -l` prints it.
pub(crate) fn fingerprint(blob_b64: &str) -> Result<String> {
    let blob = BASE64
        .decode(blob_b64.as_bytes())
        .context("a host key that is not base64")?;
    Ok(format!(
        "SHA256:{}",
        BASE64_NOPAD.encode(&Sha256::digest(&blob))
    ))
}

/// One key line of `ssh-keyscan` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScanLine {
    pub host_field: String,
    pub kind: String,
    pub blob: String,
    pub raw: String,
}

pub(crate) fn parse_scan(text: &str) -> Vec<ScanLine> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut fields = l.split_whitespace();
            Some(ScanLine {
                host_field: fields.next()?.to_string(),
                kind: fields.next()?.to_string(),
                blob: fields.next()?.to_string(),
                raw: l.to_string(),
            })
        })
        .collect()
}

/// The scan lines whose key has fingerprint `fp`. None is an error, so a key
/// that changed after the operator saw it is never written.
pub(crate) fn lines_matching(scan: &[ScanLine], fp: &str, dest: &str) -> Result<Vec<String>> {
    let matching: Vec<String> = scan
        .iter()
        .filter(|l| fingerprint(&l.blob).is_ok_and(|f| f == fp))
        .map(|l| l.raw.clone())
        .collect();
    if matching.is_empty() {
        bail!("the host key of {dest} is not the one you were shown: nothing was written");
    }
    Ok(matching)
}

/// The name `known_hosts` stores for a host: bare on port 22, else
/// `[host]:port`.
pub(crate) fn known_name(hostname: &str, port: u16) -> String {
    if port == 22 {
        hostname.to_string()
    } else {
        format!("[{hostname}]:{port}")
    }
}

/// Why `hostname:port` does not accept a TCP connection, or `None` when it
/// does.
fn unreachable_reason(hostname: &str, port: u16) -> Option<String> {
    let addrs = match (hostname, port).to_socket_addrs() {
        Ok(addrs) => addrs.collect::<Vec<_>>(),
        Err(e) => return Some(format!("could not resolve {hostname}: {e}")),
    };
    let mut last = format!("{hostname} has no address");
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, TIMEOUT) {
            Ok(_) => return None,
            Err(e) => last = format!("could not connect to {addr}: {e}"),
        }
    }
    Some(last)
}

fn run(program: &Path, args: &[&str]) -> Result<std::process::Output> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    ralphy_proc_util::no_window(&mut cmd);
    cmd.output()
        .with_context(|| format!("starting {}", program.display()))
}

fn is_known(keygen: &Path, r: &Resolved) -> Result<bool> {
    let name = known_name(&r.hostname, r.port);
    for file in r.known_hosts.iter().filter(|f| Path::new(f).is_file()) {
        if run(keygen, &["-F", &name, "-f", file])?.status.success() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The host key `ssh` negotiates with `dest`, as `known_hosts` lines. `ssh`
/// records it into a throwaway file and then offers no sign-in method, so
/// nothing signs in. `ssh-keyscan` is not used: the one in Windows
/// (OpenSSH_for_Windows 9.5p2) reads no key from an OpenSSH 10 server
/// (`choose_kex: unsupported KEX method sntrup761x25519-sha512@openssh.com`).
fn scan(ssh: &Path, dest: &str) -> Result<Vec<ScanLine>> {
    let dir = std::env::temp_dir().join(format!("ralphy-hostkey-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let file = dir.join("known_hosts");
    let user_file = format!("UserKnownHostsFile={}", file.display());
    let global_file = format!("GlobalKnownHostsFile={}", dir.join("none").display());
    let out = run(
        ssh,
        &[
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            &user_file,
            "-o",
            &global_file,
            "-o",
            "HashKnownHosts=no",
            "-o",
            "PreferredAuthentications=none",
            "-o",
            "ConnectTimeout=10",
            "--",
            dest,
            "exit",
        ],
    );
    let text = std::fs::read_to_string(&file);
    let cleaned = std::fs::remove_dir_all(&dir);
    out?;
    cleaned.with_context(|| format!("deleting {}", dir.display()))?;
    match text {
        Ok(text) => Ok(parse_scan(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", file.display())),
    }
}

/// `ralphy host key`: the state of `dest`'s host key, as one JSON object.
pub(crate) fn key(ssh: &Path, keygen: &Path, dest: &str) -> Result<Value> {
    let r = resolve(ssh, dest)?;
    if r.proxied {
        return Ok(json!({"state": "proxied"}));
    }
    if let Some(reason) = unreachable_reason(&r.hostname, r.port) {
        return Ok(json!({"state": "unreachable", "reason": reason}));
    }
    if is_known(keygen, &r)? {
        return Ok(json!({"state": "known"}));
    }
    let lines = scan(ssh, dest)?;
    if lines.is_empty() {
        let reason = format!("{}:{} showed no SSH host key", r.hostname, r.port);
        return Ok(json!({"state": "unreachable", "reason": reason}));
    }
    let keys = lines
        .iter()
        .map(|l| Ok(json!({"type": l.kind, "fingerprint": fingerprint(&l.blob)?})))
        .collect::<Result<Vec<Value>>>()?;
    Ok(json!({"state": "unknown", "host": r.hostname, "port": r.port, "keys": keys}))
}

/// `ralphy host trust`: read `dest`'s key again and append to the first
/// `known_hosts` file only the keys whose fingerprint is `fp`.
pub(crate) fn trust(ssh: &Path, dest: &str, fp: &str) -> Result<()> {
    let r = resolve(ssh, dest)?;
    if r.proxied {
        bail!("{dest} is reached through a proxy: run `ssh {dest}` once to trust its key");
    }
    let lines = lines_matching(&scan(ssh, dest)?, fp, dest)?;
    let file = r
        .known_hosts
        .first()
        .with_context(|| format!("`ssh -G {dest}` names no known_hosts file"))?;
    append(Path::new(file), &lines)
}

fn append(file: &Path, lines: &[String]) -> Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let current = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", file.display())),
    };
    let mut text = String::new();
    if !current.is_empty() && !current.ends_with(b"\n") {
        text.push('\n');
    }
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .with_context(|| format!("opening {}", file.display()))?;
    f.write_all(text.as_bytes())
        .with_context(|| format!("writing {}", file.display()))
}

#[cfg(test)]
mod tests;
