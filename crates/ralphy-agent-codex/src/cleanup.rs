//! Cleanup whose failure changes nothing for the run.

use std::path::Path;

/// Removes a plan or output file that an earlier run may have left.
///
/// The file may not exist, which is the normal case, so the result is dropped.
pub(crate) fn remove_stale_file(path: &Path) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the file may not exist, and the next write replaces one that stays"
    )]
    let _ = std::fs::remove_file(path);
}
