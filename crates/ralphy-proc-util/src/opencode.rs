//! Where `OpenCode` is installed. It lives here because the daemon imports no
//! adapter crate; the roster and the console find the binary through this module.

use std::ffi::OsString;
use std::path::PathBuf;

/// Locate `opencode` against the real environment; `None` when it is not installed.
pub fn locate_opencode() -> Option<PathBuf> {
    locate_opencode_with(
        std::env::var_os("PATH"),
        std::env::var_os("PATHEXT"),
        crate::home_dir(),
    )
}

/// [`locate_opencode`] over its inputs: `PATH` and the usual user folders first,
/// then the installer folder `<home>/.opencode/bin`.
pub fn locate_opencode_with(
    path_var: Option<OsString>,
    pathext: Option<OsString>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(found) = crate::locate_program_with("opencode", path_var, pathext, home.clone()) {
        return Some(found);
    }
    let mut cand = home?.join(".opencode").join("bin").join("opencode");
    if cfg!(windows) {
        cand.set_extension("exe");
    }
    crate::is_executable_file(&cand).then_some(cand)
}

#[cfg(test)]
mod tests;
