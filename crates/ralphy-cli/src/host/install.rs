//! `ralphy host install`: send this computer's version of Ralphy to a host over
//! the SSH session, into the folder the other host flows look in first
//! (ADR-0067 amendment D1–D8). Running the command is the operator's
//! permission; `ralphy host add` never installs.

use std::io::Write;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use ralphy_release::{Build, Release};
use sha2::{Digest, Sha256};

use super::checks::{
    classify_describe, manual_install, parse_facts, CheckId, CheckStatus, HostCheck, HostFacts,
    RalphyOnHost,
};
use super::pair::{connect, Local, Session};
use super::report::Report;
use super::shell::{installed_folder, HostOp, HostOs, InstalledOp};
use super::ssh::HostShell;
use crate::update::apply::{
    binary_for_target, download, parse_checksum, target_for, unpack_binary, verify,
};

/// Where the binary comes from (D1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Source {
    /// The host has this computer's target: its own executable is sent.
    ThisComputer,
    /// The release archive of this computer's tag, for the host's target.
    Release(String),
}

impl Source {
    /// The `source` of an `install` progress event.
    pub(crate) fn key(&self) -> &'static str {
        match self {
            Source::ThisComputer => "this-computer",
            Source::Release(_) => "release",
        }
    }

    pub(crate) fn describe(&self) -> String {
        match self {
            Source::ThisComputer => "this computer".to_string(),
            Source::Release(tag) => format!("the {tag} release"),
        }
    }
}

/// What `ralphy host install` would send to a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Offer {
    pub version: String,
    pub target: &'static str,
    pub source: Source,
    pub folder: &'static str,
}

/// The release target of a host, from what its probe printed (D6). A target
/// with no release archive is refused, Windows on ARM64 included: it runs x64
/// binaries through emulation, but nobody has measured Ralphy's consoles there.
pub(crate) fn target_of(os: HostOs, arch: Option<&str>) -> Result<&'static str, String> {
    let printed = arch.unwrap_or("").trim();
    // `uname -m` says `arm64` on macOS and `aarch64` on Linux; Windows says
    // `AMD64` or `ARM64`.
    let arch = match printed.to_ascii_lowercase().as_str() {
        "x86_64" | "amd64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        _ => "",
    };
    let os_name = match os {
        HostOs::Linux => "linux",
        HostOs::MacOs => "macos",
        HostOs::Windows => "windows",
    };
    target_for(os_name, arch).ok_or_else(|| {
        if printed.is_empty() {
            "the host did not say what processor it has".to_string()
        } else {
            format!(
                "there is no Ralphy build for {} on {printed}, so build it on the host with cargo",
                os.label()
            )
        }
    })
}

/// What `ralphy host install` sends to a host with `facts`, or why it cannot.
/// A development build has no release archive, so it can send only its own
/// executable, to a host with its own target.
pub(crate) fn offer(
    facts: &HostFacts,
    build: &Build,
    local_target: Option<&str>,
) -> Result<Offer, String> {
    let target = target_of(facts.os, facts.arch.as_deref())?;
    let (version, source) = if local_target == Some(target) {
        (build.raw.clone(), Source::ThisComputer)
    } else {
        match (&build.tag, build.ahead) {
            (Some(tag), false) => (tag.clone(), Source::Release(tag.clone())),
            _ => {
                return Err(format!(
                    "this computer runs a development build of Ralphy, which has no release for {target}, \
                     so install Ralphy on the host by hand"
                ))
            }
        }
    };
    Ok(Offer {
        version,
        target,
        source,
        folder: installed_folder(facts.os),
    })
}

/// The install offer for a host whose Ralphy must be installed. When it cannot
/// be installed from here, the Ralphy check says why and falls back to the
/// manual install.
pub(crate) fn offer_for(
    checks: &mut [HostCheck],
    facts: &HostFacts,
    ralphy: &RalphyOnHost,
    local: &Local<'_>,
) -> Option<Offer> {
    if matches!(ralphy, RalphyOnHost::Described(_)) || ralphy.is_newer() {
        return None;
    }
    match offer(facts, &local.build, local.target) {
        Ok(offer) => Some(offer),
        Err(why) => {
            if let Some(c) = checks.iter_mut().find(|c| c.id == CheckId::Ralphy) {
                c.status = CheckStatus::Copy(manual_install(facts.os));
                c.text = format!("{}: {why}", c.text);
            }
            None
        }
    }
}

/// The bytes of the binary a host receives.
pub(crate) trait Fetch {
    fn binary(&mut self, source: &Source, target: &str) -> Result<Vec<u8>>;
}

/// The production [`Fetch`]: this computer's own executable, or a release
/// archive kept in the store.
pub(crate) struct Releases<'a> {
    pub store: &'a Path,
}

impl Fetch for Releases<'_> {
    fn binary(&mut self, source: &Source, target: &str) -> Result<Vec<u8>> {
        match source {
            Source::ThisComputer => {
                let exe = std::env::current_exe().context("locating this ralphy binary")?;
                std::fs::read(&exe).with_context(|| format!("reading {}", exe.display()))
            }
            Source::Release(tag) => release_binary(
                self.store,
                tag,
                target,
                find_release,
                |url: &str, size: Option<u64>| download(url, size),
            ),
        }
    }
}

/// The release named `tag`: from the cached list when it is there, else asked
/// for by name.
fn find_release(tag: &str) -> Result<Release> {
    if let Some(cache) = ralphy_release::cache_file() {
        if let Some(found) = ralphy_release::fetch::load(&cache)
            .into_iter()
            .find(|r| r.tag_name == tag)
        {
            return Ok(found);
        }
    }
    ralphy_release::fetch_release_by_tag(ralphy_release::RELEASE_BY_TAG_URL, tag)
        .map_err(|e| anyhow!("reading the {tag} release: {e}"))
}

/// The binary for `target` out of the `tag` release (D7). The archive is kept
/// in the store by name, which holds the tag and the target, and its hash is
/// checked against the release's `.sha256` each time it is used.
pub(crate) fn release_binary(
    store: &Path,
    tag: &str,
    target: &str,
    find: impl FnOnce(&str) -> Result<Release>,
    fetch: impl Fn(&str, Option<u64>) -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    let release = find(tag)?;
    let (archive, checksum) = release
        .archive_for(target)
        .with_context(|| format!("the {tag} release has no archive for {target}"))?;
    let sums = fetch(&checksum.browser_download_url, None)?;
    let expected = parse_checksum(&String::from_utf8_lossy(&sums))?;

    let cache = store.join("host-install").join(&archive.name);
    let cached = match std::fs::read(&cache) {
        Ok(bytes) => verify(&bytes, &expected).is_ok().then_some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("reading {}", cache.display())),
    };
    let bytes = match cached {
        Some(bytes) => bytes,
        None => {
            let bytes = fetch(&archive.browser_download_url, Some(archive.size))?;
            verify(&bytes, &expected)?;
            if let Some(dir) = cache.parent() {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("creating {}", dir.display()))?;
            }
            std::fs::write(&cache, &bytes)
                .with_context(|| format!("writing {}", cache.display()))?;
            bytes
        }
    };

    let staging = std::env::temp_dir().join(format!("ralphy-host-install-{}", std::process::id()));
    let binary = unpack_binary(&bytes, &archive.name, &staging, binary_for_target(target))
        .and_then(|path| {
            std::fs::read(&path).with_context(|| format!("reading {}", path.display()))
        });
    if let Err(e) = std::fs::remove_dir_all(&staging) {
        tracing::debug!(error = %e, path = %staging.display(), "removing the unpacked archive");
    }
    binary
}

/// The hash the host printed: the first word of the last line, from
/// `sha256sum`, `shasum -a 256` or `Get-FileHash`.
fn printed_hash(stdout: &str) -> Option<String> {
    let line = stdout.lines().rev().find(|l| !l.trim().is_empty())?;
    let hash = line.split_whitespace().next()?;
    (hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| hash.to_ascii_lowercase())
}

/// `ralphy host install`: send this computer's version to `dest`, then move a
/// running daemon to it (D8). Nothing on the host changes before the copy's
/// hash matches, and an older version is never installed.
pub(crate) fn install(
    shell: &mut impl HostShell,
    local: &Local<'_>,
    dest: &str,
    key_file: Option<&Path>,
    keygen: impl FnOnce(&Path) -> Result<()>,
    fetch: &mut impl Fetch,
    out: &mut Report<impl Write>,
) -> Result<()> {
    let (identity, os) = connect(shell, local.store, dest, key_file, keygen)?;
    out.connected(os)?;
    let mut s = Session {
        shell,
        dest,
        identity,
        os,
    };
    let probe = s.run(&HostOp::Probe, b"")?;
    let facts = parse_facts(os, &probe.stdout);
    let ralphy = classify_describe(&s.run(&HostOp::Describe { with_token: false }, b"")?)?;
    if matches!(ralphy, RalphyOnHost::Described(_)) {
        out.note(&format!(
            "Ralphy on {dest} can already connect to this computer. Nothing was installed."
        ))?;
        return Ok(());
    }
    if ralphy.is_newer() {
        bail!("the Ralphy on {dest} is newer than this one, so it was not replaced: update Ralphy on this computer");
    }
    let offer = offer(&facts, &local.build, local.target).map_err(|why| anyhow!(why))?;

    let bytes = fetch.binary(&offer.source, offer.target)?;
    let expected = format!("{:x}", Sha256::digest(&bytes));
    out.note(&format!(
        "Sending Ralphy {} for {} from {} to {dest} ({:.1} MB).",
        offer.version,
        offer.target,
        offer.source.describe(),
        bytes.len() as f64 / 1_048_576.0
    ))?;
    let written = s.run_ok(&HostOp::WriteBinary, &bytes)?;
    match printed_hash(&written.stdout) {
        Some(hash) if hash == expected => {}
        Some(hash) => bail!(
            "the copy on {dest} is not the binary that was sent (expected {expected}, the host computed {hash}), so nothing was replaced"
        ),
        None => bail!(
            "{dest} did not print the hash of the copy, so nothing was replaced: {}",
            written.stdout.trim()
        ),
    }
    s.run_ok(&HostOp::CommitBinary, b"")?;
    out.note(&format!("Installed Ralphy in {} on {dest}.", offer.folder))?;

    let now = s.run_ok(&HostOp::Installed(InstalledOp::Describe), b"")?;
    let RalphyOnHost::Described(d) = classify_describe(&now)? else {
        bail!("the Ralphy installed on {dest} does not answer as this computer's version");
    };
    if d.autostart {
        s.run_ok(&HostOp::Installed(InstalledOp::InstallAutostart), b"")?;
        out.note(&format!(
            "The daemon on {dest} now starts with the installed Ralphy."
        ))?;
    }
    if d.running {
        s.run_ok(&HostOp::Installed(InstalledOp::Restart), b"")?;
        out.note(&format!(
            "Restarted the daemon on {dest} with the installed Ralphy."
        ))?;
    }
    if matches!(ralphy, RalphyOnHost::Old { .. }) {
        out.note(&format!(
            "`ralphy` on the PATH of {dest} is still the old version. To use the new one by hand, put {} first in PATH.",
            offer.folder
        ))?;
    } else {
        out.note(&format!(
            "To use Ralphy by hand on {dest}, add {} to PATH.",
            offer.folder
        ))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
