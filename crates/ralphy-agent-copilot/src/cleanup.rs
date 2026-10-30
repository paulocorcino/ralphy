//! Test-only cleanup helpers of this crate.

use std::path::Path;

/// Removes a scratch directory of a test.
///
/// The directory may not exist yet, and a cleanup that fails leaves only a
/// temporary directory behind, so the result is dropped.
pub(crate) fn remove_scratch(path: &Path) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the directory may not exist, and a failed cleanup must not fail the test"
    )]
    let _ = std::fs::remove_dir_all(path);
}
