//! Reading a file's content at a git revision — the "original" side of a diff.
//! The working-tree side is the daemon's own file reader; this module answers
//! the same question against a committed tree, with the SAME text/binary and
//! size rules so the two halves of one diff agree on what "text" is.

use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::git::raw;

/// Which revision a blob is read at. A closed enum: this slice diffs against
/// HEAD only, so widening it later widens the wire contract deliberately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revision {
    Head,
}

impl Revision {
    /// The revision as git spells it in a `<rev>:<path>` spec.
    pub fn spec(self) -> &'static str {
        match self {
            Revision::Head => "HEAD",
        }
    }
}

/// What a [`read`] found at the revision. A refusal is an OUTCOME, not an
/// `Err`: the daemon relays "absent" and "binary" as ordinary answers, and an
/// `Err` there would surface as a Query error and lose the vocabulary. `Err` is
/// reserved for a git or IO failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blob {
    Text(String),
    /// The path does not exist at that revision — a newly added file, not an error.
    Absent,
    Binary,
    TooLarge,
}

/// Hard cap on one blob, matching the working-tree reader's own cap so an
/// uncapped HEAD side cannot cross the wire for a diff whose other half refused.
pub const MAX_BLOB_BYTES: u64 = 2 * 1024 * 1024;

/// Window scanned for the text/binary heuristic, mirroring the working-tree reader.
const SNIFF_BYTES: usize = 8 * 1024;

/// Read `path` (repo-relative, forward slashes) as text at `rev`.
///
/// `repo` must be the git toplevel. Absence is keyed on git's exit code 1 from
/// `rev-parse --verify --quiet`, not on empty output: any other failing exit
/// (e.g. 128 for a path outside the repository) stays an `Err` so a real git
/// failure never masquerades as an added file.
pub fn read(repo: &Path, rev: Revision, path: &str) -> Result<Blob> {
    let spec = format!("{}:{}", rev.spec(), path);
    let ctx = || format!("reading {path} at {}", rev.spec());

    let out = raw(repo, &["rev-parse", "--verify", "--quiet", &spec]).with_context(ctx)?;
    match out.status.code() {
        Some(0) => {}
        Some(1) => return Ok(Blob::Absent),
        _ => bail!(
            "`git rev-parse {}` failed: {}",
            spec,
            String::from_utf8_lossy(&out.stderr).trim()
        ),
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();

    let out = raw(repo, &["cat-file", "-s", &sha]).with_context(ctx)?;
    if !out.status.success() {
        bail!(
            "`git cat-file -s {}` failed: {}",
            sha,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let size: u64 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .with_context(|| format!("unparseable object size for {sha}"))?;
    if size > MAX_BLOB_BYTES {
        return Ok(Blob::TooLarge);
    }

    let out = raw(repo, &["cat-file", "blob", &sha]).with_context(ctx)?;
    if !out.status.success() {
        bail!(
            "`git cat-file blob {}` failed: {}",
            sha,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let bytes = out.stdout;

    // NUL in the first window is the cheap binary tell; UTF-8 validity is decided
    // over the WHOLE file so a multibyte char straddling the window boundary is
    // not a false positive. Same rule as the working-tree reader, deliberately.
    if bytes[..bytes.len().min(SNIFF_BYTES)].contains(&0) {
        return Ok(Blob::Binary);
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Blob::Text(text)),
        Err(_) => Ok(Blob::Binary),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::git;
    use std::path::PathBuf;

    fn init_repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ralphy-blob-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]).unwrap();
        git(&dir, &["config", "user.email", "t@example.com"]).unwrap();
        git(&dir, &["config", "user.name", "Test"]).unwrap();
        dir
    }

    fn commit_all(dir: &Path) {
        git(dir, &["add", "."]).unwrap();
        git(dir, &["commit", "-q", "-m", "init"]).unwrap();
    }

    #[test]
    fn head_blob_reads_the_committed_bytes() {
        let dir = init_repo("present");
        // The trailing newline is the assertion: a reader routed through the
        // trimming `git()` would drop it.
        std::fs::write(dir.join("README.md"), "hello\nworld\n").unwrap();
        commit_all(&dir);

        assert_eq!(
            read(&dir, Revision::Head, "README.md").unwrap(),
            Blob::Text("hello\nworld\n".to_string())
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn absent_at_the_revision_is_not_an_error() {
        let dir = init_repo("absent");
        std::fs::write(dir.join("README.md"), "hello\n").unwrap();
        commit_all(&dir);
        std::fs::write(dir.join("fresh.txt"), "brand new line\n").unwrap();

        let got = read(&dir, Revision::Head, "fresh.txt");
        assert!(got.is_ok(), "absence must be an Ok outcome: {got:?}");
        assert_eq!(got.unwrap(), Blob::Absent);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn binary_at_the_revision_is_refused() {
        let dir = init_repo("binary");
        std::fs::write(dir.join("logo.png"), [0x89, b'P', 0x00, 0x01]).unwrap();
        commit_all(&dir);

        assert_eq!(
            read(&dir, Revision::Head, "logo.png").unwrap(),
            Blob::Binary
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_over_cap_blob_is_refused() {
        let dir = init_repo("too-large");
        std::fs::write(
            dir.join("big.txt"),
            vec![b'a'; (MAX_BLOB_BYTES + 1) as usize],
        )
        .unwrap();
        commit_all(&dir);

        assert_eq!(
            read(&dir, Revision::Head, "big.txt").unwrap(),
            Blob::TooLarge
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
