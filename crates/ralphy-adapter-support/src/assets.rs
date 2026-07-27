//! Embedded-asset materialization: extract an [`include_dir::Dir`] tree onto
//! disk, clearing any prior copy first.

use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};

/// Materialize `asset` into `dest_dir`, clearing any prior copy first, and
/// optionally write a `*` `.gitignore` at `gitignore_dir/.gitignore`.
///
/// The clear-before-extract pattern guarantees a removed file in the embedded
/// tree never lingers between runs. `gitignore_dir` is `None` for adapters that
/// own no `.gitignore` concern (Claude's plugin dir is already inside `.ralphy`
/// which carries its own ignore rules); it is `Some(dir)` for adapters that
/// materialize into a directory the executor might otherwise commit
/// (Codex → `.agents`, OpenCode → `.ralphy`).
pub fn materialize_assets(
    asset: &include_dir::Dir,
    dest_dir: &Path,
    gitignore_dir: Option<&Path>,
) -> Result<()> {
    // Extract into a sibling staging dir first, then swap it over `dest_dir`. A
    // failed extract (disk full, permission) leaves the previous good copy
    // untouched instead of a half-populated tree — the slow, failure-prone step
    // happens off to the side, and only the fast remove+rename touches `dest_dir`.
    let staging = dest_dir.with_file_name(format!(
        "{}.tmp-{}",
        dest_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("asset"),
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&staging); // clear any leftover from a crashed run
    fs::create_dir_all(&staging).context("creating the asset staging directory")?;
    if let Err(e) = asset.extract(&staging) {
        let _ = fs::remove_dir_all(&staging);
        return Err(e).context("extracting the embedded asset tree");
    }
    if dest_dir.exists() {
        retry_transient_fs(|| fs::remove_dir_all(dest_dir))
            .context("clearing the stale materialized asset directory")?;
    }
    retry_transient_fs(|| fs::rename(&staging, dest_dir))
        .context("swapping the materialized asset directory into place")?;
    if let Some(dir) = gitignore_dir {
        fs::write(dir.join(".gitignore"), "*\n").context("writing .gitignore")?;
    }
    Ok(())
}

/// Retry `op` while it fails with a transient Windows sharing error.
///
/// On Windows, deleting a directory another process still holds a handle to
/// (a just-exited agent child, an antivirus scan, a file watcher) does not
/// fail: the directory goes delete-pending and its *name* is freed only when
/// the last handle closes. `remove_dir_all` then reports success while the
/// follow-up `rename` onto that name fails with `ERROR_ACCESS_DENIED`; a
/// scanner holding a freshly extracted file open surfaces as
/// `ERROR_SHARING_VIOLATION` the same way. Both clear as soon as the handle
/// closes, so a short backoff loop (~1.5s total) absorbs the race. On
/// non-Windows targets no error is treated as transient and `op` runs once.
fn retry_transient_fs<T>(mut op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    const MAX_ATTEMPTS: u32 = 10;
    let mut delay = Duration::from_millis(4);
    let mut attempt = 1;
    loop {
        match op() {
            Ok(v) => return Ok(v),
            Err(e) if attempt < MAX_ATTEMPTS && is_transient_lock(&e) => {
                std::thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_millis(500));
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// `ERROR_ACCESS_DENIED` (5) and `ERROR_SHARING_VIOLATION` (32) — the two
/// shapes a still-open handle takes on Windows (see [`retry_transient_fs`]).
#[cfg(windows)]
fn is_transient_lock(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(5) | Some(32))
}

#[cfg(not(windows))]
fn is_transient_lock(_e: &io::Error) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use include_dir::include_dir;

    static FIXTURE: include_dir::Dir<'_> =
        include_dir!("$CARGO_MANIFEST_DIR/tests/fixtures/sample");

    #[test]
    fn materialize_assets_clears_extracts_and_writes_gitignore() {
        let tmp = std::env::temp_dir().join(format!("ralphy-mat-assets-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);

        // Destination with a pre-existing stale file.
        let dest = tmp.join("dest");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("stale.txt"), b"stale").unwrap();

        // Separate dir for the .gitignore.
        let gitignore_dir = tmp.join("gi");
        fs::create_dir_all(&gitignore_dir).unwrap();

        materialize_assets(&FIXTURE, &dest, Some(&gitignore_dir)).expect("materialize");

        // Stale file was cleared.
        assert!(
            !dest.join("stale.txt").exists(),
            "stale file must be removed before extraction"
        );
        // Top-level file extracted.
        assert!(
            dest.join("hello.txt").is_file(),
            "hello.txt must be extracted"
        );
        // Nested file extracted.
        assert!(
            dest.join("sub/nested.txt").is_file(),
            "sub/nested.txt must be extracted"
        );
        // .gitignore written at the requested location.
        let gi_path = gitignore_dir.join(".gitignore");
        assert!(gi_path.is_file(), ".gitignore must be written");
        let gi_contents = fs::read_to_string(&gi_path).unwrap();
        assert!(
            gi_contents.contains('*'),
            ".gitignore must contain '*': {gi_contents:?}"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn retry_transient_fs_does_not_retry_a_real_error() {
        let mut calls = 0;
        let err = retry_transient_fs(|| -> io::Result<()> {
            calls += 1;
            Err(io::Error::new(io::ErrorKind::NotFound, "missing"))
        })
        .expect_err("a non-transient error must surface");
        assert_eq!(calls, 1, "a non-transient error must not be retried");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[cfg(windows)]
    #[test]
    fn retry_transient_fs_outlasts_a_delete_pending_window() {
        let mut calls = 0;
        retry_transient_fs(|| {
            calls += 1;
            if calls < 3 {
                // ERROR_ACCESS_DENIED, as raised while the old dir's name is
                // still delete-pending.
                Err(io::Error::from_raw_os_error(5))
            } else {
                Ok(())
            }
        })
        .expect("must succeed once the transient error clears");
        assert_eq!(calls, 3);
    }
}
