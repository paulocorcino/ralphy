//! Cleanup whose failure changes nothing for the run.

use std::path::Path;

/// Removes a file an earlier run may have left (a flag or a plan).
///
/// The file may not exist, which is the normal case, so the result is dropped.
pub(crate) fn remove_stale_file(path: &Path) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the file may not exist, and the next write replaces one that stays"
    )]
    let _ = std::fs::remove_file(path);
}

/// Removes a scratch directory of a test.
///
/// The directory may not exist yet, and a cleanup that fails leaves only a
/// temporary directory behind, so the result is dropped.
#[cfg(test)]
pub(crate) fn remove_scratch(path: &Path) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the directory may not exist, and a failed cleanup must not fail the test"
    )]
    let _ = std::fs::remove_dir_all(path);
}
