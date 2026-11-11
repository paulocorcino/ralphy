//! Taking a release: download, verify, unpack, replace (ADR-0056 §8).
//!
//! Every step here refuses rather than guesses. The checksum the release
//! workflow already publishes is what makes replacing a binary defensible, so a
//! mismatch is fatal and never a warning; an archive that does not carry a
//! `ralphy` binary is refused before anything on disk is touched; and the
//! replacement is rename-then-put, because a running image on Windows cannot be
//! deleted but can be renamed.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};

use crate::install::binary_name;

/// Generous: this is a multi-megabyte download the operator asked for and is
/// watching, not a background poll.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// A release archive is a few megabytes; ten minutes is a slow-link allowance,
/// not a budget anything is expected to use.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

/// The published target name for this host, matching the release workflow's
/// matrix. `None` on a host the project does not publish for, which is a refusal
/// with a name rather than a wrong download.
pub(crate) fn host_target() -> Option<&'static str> {
    target_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// The published target for an OS and an architecture, spelled as
/// `std::env::consts` spells them (`linux`, `macos`, `windows`; `x86_64`,
/// `aarch64`). `None` when the release workflow builds nothing for them.
pub(crate) fn target_for(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("windows", "x86_64") => Some("windows-x64"),
        ("linux", "x86_64") => Some("linux-x64"),
        ("linux", "aarch64") => Some("linux-arm64"),
        ("macos", "x86_64") => Some("macos-x64"),
        ("macos", "aarch64") => Some("macos-arm64"),
        _ => None,
    }
}

/// The name of the binary inside a target's archive.
pub(crate) fn binary_for_target(target: &str) -> &'static str {
    if target.starts_with("windows-") {
        "ralphy.exe"
    } else {
        "ralphy"
    }
}

/// The slack allowed over an asset's published size before a download is
/// refused. A few hundred kilobytes covers a re-packed archive; it does not
/// cover a body that intends to fill memory.
const SIZE_SLACK: u64 = 512 * 1024;
/// The cap for a body with no published size — the `.sha256` files, which are
/// one short line.
const UNSIZED_CAP: u64 = 64 * 1024;

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(READ_TIMEOUT))
        // No body timeout: ureq's covers the whole body, and a slow link must
        // get the full REQUEST_TIMEOUT for a multi-megabyte archive.
        .timeout_global(Some(REQUEST_TIMEOUT))
        // ureq 3 reads HTTPS_PROXY and friends by default; Ralphy never has
        // (#443).
        .proxy(None)
        .build()
        .into()
}

/// GET `url` into memory, following redirects — a release asset URL answers 302
/// to the object store, so refusing redirects would fail every download.
///
/// `expected_size` is the size the release published for this asset; the read is
/// capped just above it, so a redirect target that streams without end cannot
/// fill memory before the checksum ever gets a chance to refuse it.
pub(crate) fn download(url: &str, expected_size: Option<u64>) -> Result<Vec<u8>> {
    let agent = agent();
    let resp = agent
        .get(url)
        .header("User-Agent", "ralphy")
        .call()
        .with_context(|| format!("downloading {url}"))?;

    let cap = match expected_size {
        Some(size) => size.saturating_add(SIZE_SLACK),
        None => UNSIZED_CAP,
    };
    let mut bytes = Vec::new();
    // `into_reader` has no limit of its own; the cap is this `take`.
    resp.into_body()
        .into_reader()
        .take(cap.saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading the body of {url}"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > cap {
        bail!("{url} returned more than the {cap} bytes it published; refusing it");
    }
    Ok(bytes)
}

/// The hash out of a `sha256sum`/`shasum -a 256` file: `<64 hex>  <filename>`,
/// two spaces. Lowercased so a comparison is case-insensitive by construction.
pub(crate) fn parse_checksum(text: &str) -> Result<String> {
    let first = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .context("the checksum file is empty")?;
    let hash = first
        .split_whitespace()
        .next()
        .context("the checksum file has no hash")?;
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("`{hash}` is not a sha256 digest");
    }
    Ok(hash.to_ascii_lowercase())
}

/// Refuse `bytes` unless they hash to `expected`. This is the whole warrant for
/// replacing a binary, so it is an error and never a warning.
pub(crate) fn verify(bytes: &[u8], expected: &str) -> Result<()> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual != expected {
        bail!("checksum mismatch: expected {expected}, got {actual}");
    }
    Ok(())
}

/// Unpack `bytes` into `dir` and return the path of this computer's `ralphy`
/// binary inside.
pub(crate) fn unpack(bytes: &[u8], name: &str, dir: &Path) -> Result<PathBuf> {
    unpack_binary(bytes, name, dir, binary_name())
}

/// Unpack `bytes` into `dir` and return the path of the binary named `binary`
/// inside, which is another computer's name when the archive is for another
/// target. The published archives carry one top-level directory, but nothing
/// here depends on that: the binary is found by name at any depth.
pub(crate) fn unpack_binary(bytes: &[u8], name: &str, dir: &Path, binary: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    if name.ends_with(".zip") {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .with_context(|| format!("opening {name}"))?;
        archive
            .extract(dir)
            .with_context(|| format!("extracting {name}"))?;
    } else if name.ends_with(".tar.gz") {
        let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
        tar::Archive::new(decoder)
            .unpack(dir)
            .with_context(|| format!("extracting {name}"))?;
    } else {
        bail!("unknown archive format: {name}");
    }

    find_binary(dir, binary)?
        .ok_or_else(|| anyhow!("{name} carries no {binary} — refusing to replace anything"))
}

fn find_binary(dir: &Path, wanted: &str) -> Result<Option<PathBuf>> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = std::fs::read_dir(&current)
            .with_context(|| format!("reading {}", current.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n == wanted) {
                return Ok(Some(path));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
