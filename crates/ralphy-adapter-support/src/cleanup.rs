//! Cleanup whose failure changes nothing for the run.

use std::path::Path;

/// Removes a file an earlier run may have left (a flag, a plan or an output file).
///
/// The file may not exist, which is the normal case; any other failure is logged,
/// because the next write replaces a file that stays.
pub fn remove_stale_file(path: &Path) {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            tracing::warn!("could not remove stale file {}: {e}", path.display());
        }
        _ => {}
    }
}
