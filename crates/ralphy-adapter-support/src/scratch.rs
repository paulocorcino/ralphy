//! Cleanup of the scratch files and directories the unit tests make.

use std::path::Path;

/// Remove a scratch directory a test made. It may be gone already, and a
/// leftover directory in the temp dir fails nothing.
pub(crate) fn remove_scratch_dir(dir: &Path) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "cleanup of a test directory that may be gone, and no test reads it after"
    )]
    let _ = std::fs::remove_dir_all(dir);
}

/// Remove a scratch file a test made, for the same reason as [`remove_scratch_dir`].
pub(crate) fn remove_scratch_file(file: &Path) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "cleanup of a test file that may be gone, and no test reads it after"
    )]
    let _ = std::fs::remove_file(file);
}
