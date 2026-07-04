//! Embedded-asset materialization: extract an [`include_dir::Dir`] tree onto
//! disk, clearing any prior copy first.

use std::fs;
use std::path::Path;

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
        fs::remove_dir_all(dest_dir).context("clearing the stale materialized asset directory")?;
    }
    fs::rename(&staging, dest_dir)
        .context("swapping the materialized asset directory into place")?;
    if let Some(dir) = gitignore_dir {
        fs::write(dir.join(".gitignore"), "*\n").context("writing .gitignore")?;
    }
    Ok(())
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
}
