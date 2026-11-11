//! Test-only cleanup helpers of this crate.

use std::path::Path;

/// Removes a scratch directory of a test.
///
/// The directory may not exist yet, and a cleanup that fails leaves only a
/// temporary directory behind, so the result is dropped.
pub(crate) fn remove_scratch(path: &Path) {
    if let Err(e) = std::fs::remove_dir_all(path) {
        tracing::debug!("could not remove scratch dir {}: {e}", path.display());
    }
}

/// Removes a scratch file of a test, for the same reasons as [`remove_scratch`].
pub(crate) fn remove_scratch_file(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        tracing::debug!("could not remove scratch file {}: {e}", path.display());
    }
}
