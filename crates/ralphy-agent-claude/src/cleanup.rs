//! Cleanup whose failure changes nothing for the run.

use std::path::Path;

/// Removes a file an earlier run may have left (a flag or a plan).
///
/// The file may not exist, which is the normal case; any other failure is logged,
/// because the next write replaces a file that stays.
pub(crate) fn remove_stale_file(path: &Path) {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            tracing::warn!("could not remove stale file {}: {e}", path.display());
        }
        _ => {}
    }
}

/// Removes a scratch directory of a test.
///
/// The directory may not exist yet, and a cleanup that fails leaves only a
/// temporary directory behind, so the failure is only logged.
#[cfg(test)]
pub(crate) fn remove_scratch(path: &Path) {
    if let Err(e) = std::fs::remove_dir_all(path) {
        tracing::debug!("could not remove scratch dir {}: {e}", path.display());
    }
}
