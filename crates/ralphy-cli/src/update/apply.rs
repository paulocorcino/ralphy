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
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("windows-x64"),
        ("linux", "x86_64") => Some("linux-x64"),
        ("macos", "x86_64") => Some("macos-x64"),
        ("macos", "aarch64") => Some("macos-arm64"),
        _ => None,
    }
}

/// The slack allowed over an asset's published size before a download is
/// refused. A few hundred kilobytes covers a re-packed archive; it does not
/// cover a body that intends to fill memory.
const SIZE_SLACK: u64 = 512 * 1024;
/// The cap for a body with no published size — the `.sha256` files, which are
/// one short line.
const UNSIZED_CAP: u64 = 64 * 1024;

/// GET `url` into memory, following redirects — a release asset URL answers 302
/// to the object store, so refusing redirects (as the poll does) would fail
/// every download.
///
/// `expected_size` is the size the release published for this asset; the read is
/// capped just above it, so a redirect target that streams without end cannot
/// fill memory before the checksum ever gets a chance to refuse it.
pub(crate) fn download(url: &str, expected_size: Option<u64>) -> Result<Vec<u8>> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build();
    let resp = agent
        .get(url)
        .set("User-Agent", "ralphy")
        .call()
        .with_context(|| format!("downloading {url}"))?;

    let cap = match expected_size {
        Some(size) => size.saturating_add(SIZE_SLACK),
        None => UNSIZED_CAP,
    };
    let mut bytes = Vec::new();
    resp.into_reader()
        .take(cap.saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading the body of {url}"))?;
    if bytes.len() as u64 > cap {
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

/// Unpack `bytes` into `dir` and return the path of the `ralphy` binary inside.
/// The published archives carry one top-level directory, but nothing here
/// depends on that: the binary is found by name at any depth.
pub(crate) fn unpack(bytes: &[u8], name: &str, dir: &Path) -> Result<PathBuf> {
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

    find_binary(dir)?.ok_or_else(|| {
        anyhow!(
            "{name} carries no {} — refusing to replace anything",
            binary_name()
        )
    })
}

fn find_binary(dir: &Path) -> Result<Option<PathBuf>> {
    let wanted = binary_name();
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
mod tests {
    use super::*;

    /// Serve `response` once on a loopback port. The release crate has a richer
    /// harness; this one exists because the cap is a property of `download`,
    /// which lives here.
    fn serve_once(response: Vec<u8>) -> (u16, std::thread::JoinHandle<()>) {
        use std::io::Write;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut sink = [0u8; 1024];
                let _ = std::io::Read::read(&mut stream, &mut sink);
                let _ = stream.write_all(&response);
                let _ = stream.flush();
            }
        });
        std::thread::sleep(std::time::Duration::from_millis(20));
        (port, handle)
    }

    fn http_response(status: u16, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status} OK
Content-Length: {}
Connection: close

{body}",
            body.len()
        )
        .into_bytes()
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ralphy-apply-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[test]
    fn the_host_maps_to_a_published_target() {
        // Whatever runs the suite must be a platform the project publishes for;
        // if that stops being true, this is the test that says so.
        assert!(
            host_target().is_some(),
            "no published archive for {}/{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
    }

    #[test]
    fn a_checksum_file_is_the_hash_before_the_two_spaces() {
        let hash = "a".repeat(64);
        assert_eq!(
            parse_checksum(&format!("{hash}  ralphy-v1-linux-x64.tar.gz\n")).expect("parse"),
            hash
        );
        assert_eq!(
            parse_checksum(&format!("{}  file\n", hash.to_uppercase())).expect("parse"),
            hash,
            "case must not decide a comparison"
        );
    }

    #[test]
    fn something_that_is_not_a_digest_is_refused() {
        assert!(parse_checksum("").is_err());
        assert!(parse_checksum("not-a-hash  file\n").is_err());
        assert!(parse_checksum(&format!("{}  file\n", "a".repeat(63))).is_err());
    }

    #[test]
    fn a_flipped_byte_is_refused() {
        let bytes = b"the release archive";
        let good = format!("{:x}", Sha256::digest(bytes));
        verify(bytes, &good).expect("the archive we hashed must verify");

        let mut tampered = bytes.to_vec();
        tampered[0] ^= 0x01;
        let err = verify(&tampered, &good).expect_err("one flipped byte must refuse the update");
        assert!(err.to_string().contains("checksum mismatch"), "{err}");
    }

    #[test]
    fn an_archive_with_no_binary_is_refused_before_anything_is_touched() {
        let dir = scratch("empty-archive");
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            zip.start_file::<_, ()>(
                "ralphy-v1-x/README.md",
                zip::write::SimpleFileOptions::default(),
            )
            .expect("entry");
            std::io::Write::write_all(&mut zip, b"docs only").expect("write");
            zip.finish().expect("finish");
        }
        let err = unpack(&buf, "ralphy-v1-x.zip", &dir.join("out"))
            .expect_err("an archive with no binary must refuse");
        assert!(err.to_string().contains("carries no"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_zip_yields_the_binary_at_any_depth() {
        let dir = scratch("zip");
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            zip.start_file::<_, ()>(
                format!("ralphy-v1-x/{}", binary_name()),
                zip::write::SimpleFileOptions::default(),
            )
            .expect("entry");
            std::io::Write::write_all(&mut zip, b"new binary").expect("write");
            zip.finish().expect("finish");
        }
        let found = unpack(&buf, "ralphy-v1-x.zip", &dir.join("out")).expect("unpack");
        assert_eq!(std::fs::read(&found).expect("read"), b"new binary");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tar_gz_yields_the_binary_too() {
        // The Linux and macOS format. It was reachable only through the
        // `#[ignore]`d live test, so the primary non-Windows update path had no
        // gate at all: a broken gzip or tar branch shipped green.
        let dir = scratch("targz");
        let mut gz = Vec::new();
        {
            let encoder = flate2::write::GzEncoder::new(&mut gz, flate2::Compression::fast());
            let mut builder = tar::Builder::new(encoder);
            let payload = b"new binary";
            let mut header = tar::Header::new_gnu();
            header.set_size(payload.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    format!("ralphy-v1-x/{}", binary_name()),
                    &payload[..],
                )
                .expect("entry");
            builder.into_inner().expect("tar").finish().expect("gzip");
        }

        let found = unpack(&gz, "ralphy-v1-x.tar.gz", &dir.join("out")).expect("unpack");
        assert_eq!(std::fs::read(&found).expect("read"), b"new binary");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_body_larger_than_it_published_is_refused() {
        // The download is capped just above the size the release published, so a
        // redirect target that streams without end cannot fill memory before the
        // checksum ever gets to refuse it.
        // A body with no published size is capped at UNSIZED_CAP — the shape a
        // `.sha256` takes, where anything past one short line is a lie.
        let body = "x".repeat(200_000);
        let (port, handle) = serve_once(http_response(200, &body));
        let url = format!("http://127.0.0.1:{port}/");
        let err = download(&url, None).expect_err("more than the cap must be refused");
        assert!(err.to_string().contains("more than the"), "{err}");
        drop(handle);

        // And a body inside the published size plus its slack is taken.
        let small = "y".repeat(32);
        let (port, handle) = serve_once(http_response(200, &small));
        let url = format!("http://127.0.0.1:{port}/");
        assert_eq!(
            download(&url, Some(32))
                .expect("a body of its published size is fine")
                .len(),
            32
        );
        drop(handle);
    }

    #[test]
    fn an_unknown_archive_format_is_refused() {
        let dir = scratch("format");
        assert!(unpack(b"whatever", "ralphy-v1-x.7z", &dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The one test that touches the network, and the only way to prove the
    /// download → checksum → unpack path against what is actually published.
    /// Ignored by default so CI and an offline machine never reach for it; run
    /// it deliberately:
    ///
    /// ```text
    /// cargo nextest run -p ralphy-cli -E 'test(takes_a_published_release)' --run-ignored all
    /// ```
    ///
    /// It replaces nothing: the binary is unpacked into a temp dir and dropped.
    #[test]
    #[ignore = "reaches github.com; run deliberately"]
    fn it_takes_a_published_release_and_verifies_it() {
        use ralphy_release::fetch::{self, RefreshOpts};

        let target = host_target().expect("a published host");
        let cache = std::env::temp_dir().join(format!("ralphy-live-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&cache);
        let mut opts = RefreshOpts::new(&cache);
        opts.force = true;
        opts.offline = false;
        fetch::refresh_if_stale(&opts);

        let releases = fetch::load(&cache);
        assert!(!releases.is_empty(), "the project publishes releases");
        let (release, archive, checksum) = releases
            .iter()
            .find_map(|r| r.archive_for(target).map(|(a, c)| (r, a, c)))
            .expect("some published release carries an archive for this host");

        let bytes = download(&archive.browser_download_url, Some(archive.size))
            .expect("download the archive");
        let sums = download(&checksum.browser_download_url, None).expect("download the checksum");
        let expected =
            parse_checksum(&String::from_utf8_lossy(&sums)).expect("the published checksum parses");
        verify(&bytes, &expected).expect("the published archive matches its published checksum");

        let dir = scratch("live");
        let staged = unpack(&bytes, &archive.name, &dir).expect("unpack");
        assert!(
            std::fs::metadata(&staged).expect("staged binary").len() > 1_000_000,
            "a real ralphy binary, not a stub"
        );
        println!(
            "verified {} ({}, {} bytes)",
            release.tag_name,
            archive.name,
            bytes.len()
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&cache);
    }
}
