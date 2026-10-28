//! The path of the running `ralphy` binary, in a form every program can run.
//!
//! On Windows `std::fs::canonicalize` returns the extended-length form
//! (`\\?\C:\…`). `CreateProcess` accepts it, but a path written into a command
//! line another program parses does not survive: Git Bash, which Claude Code
//! runs its hooks with, reads `"\\?\C:\…"` as `\?\C:\…` and finds no file.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// The running binary, resolved to the real file (not a link to it), in the
/// plain form [`plain_path`] returns.
pub fn running_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("locating the running ralphy binary")?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    Ok(plain_path(exe))
}

/// `path` without the `\\?\` prefix when it is a verbatim drive path the plain
/// form names the same way: `\\?\C:\x` becomes `C:\x`. Any other path comes
/// back unchanged — a verbatim UNC path, a path off Windows, and a path of
/// `MAX_PATH` (260) characters or more, which only the verbatim form can open.
/// For a path `canonicalize` returned: that form has no `.`, `..` or `/` to
/// normalize.
pub fn plain_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        const MAX_PATH: usize = 260;
        let verbatim_disk = matches!(
            path.components().next(),
            Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::VerbatimDisk(_))
        );
        if let Some(plain) = path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
            if verbatim_disk && plain.len() < MAX_PATH {
                return PathBuf::from(plain);
            }
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn a_verbatim_drive_path_loses_its_prefix() {
        assert_eq!(
            plain_path(PathBuf::from(r"\\?\C:\Users\me\.cargo\bin\ralphy.exe")),
            PathBuf::from(r"C:\Users\me\.cargo\bin\ralphy.exe")
        );
    }

    #[cfg(windows)]
    #[test]
    fn other_windows_paths_stay_as_they_are() {
        let long = format!(r"\\?\C:\{}\ralphy.exe", "a".repeat(260));
        for path in [
            r"C:\plain\ralphy.exe",
            r"\\?\UNC\server\share\ralphy.exe",
            long.as_str(),
        ] {
            assert_eq!(plain_path(PathBuf::from(path)), PathBuf::from(path));
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn a_unix_path_stays_as_it_is() {
        // On Unix `\\?\` is part of a file name, not a prefix.
        let path = PathBuf::from(r"\\?\C:\ralphy");
        assert_eq!(plain_path(path.clone()), path);
    }

    #[test]
    fn the_running_exe_is_absolute_and_plain() {
        let exe = running_exe().unwrap();
        assert!(exe.is_absolute(), "{}", exe.display());
        assert!(
            !exe.to_string_lossy().starts_with(r"\\?\"),
            "{}",
            exe.display()
        );
    }
}
