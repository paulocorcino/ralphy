//! Cleanup of the scratch directories the unit tests make.

use std::path::Path;

/// Removes a scratch directory of a test.
///
/// The directory may not exist yet, and a cleanup that fails leaves only a
/// temporary directory behind, so the failure is only logged.
pub(crate) fn remove_scratch(path: &Path) {
    if let Err(e) = std::fs::remove_dir_all(path) {
        tracing::debug!("could not remove scratch dir {}: {e}", path.display());
    }
}
