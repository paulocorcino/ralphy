//! Capture the git-published version at build time so `ralphy --version` reports
//! the same string that was tagged/released (e.g. `v0.1.0-rc2`) rather than the
//! Cargo manifest version, which can lag the tag. Falls back to the Cargo version
//! when git is unavailable (e.g. a source tarball with no `.git`).

use std::process::Command;

fn main() {
    // Re-run when HEAD or the tag set moves so the embedded version stays current.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/tags");

    let version = git_describe().unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rustc-env=RALPHY_VERSION={version}");

    embed_windows_icon();
}

/// Embed the application icon (`assets/icons/ralphy.ico`) into `ralphy.exe` so
/// the file shows the Ralphy loop-R mark in Explorer, the taskbar, and
/// shortcuts. No-op off Windows; the `winresource` build-dependency is
/// Windows-only, so nothing is compiled or linked elsewhere.
fn embed_windows_icon() {
    #[cfg(windows)]
    {
        // Path is relative to this crate's manifest dir (crates/ralphy-cli).
        const ICON: &str = "../../assets/icons/ralphy.ico";
        println!("cargo:rerun-if-changed={ICON}");

        let mut res = winresource::WindowsResource::new();
        res.set_icon(ICON);
        if let Err(err) = res.compile() {
            // Don't fail the build if a resource compiler is unavailable; the
            // binary stays fully functional, just without the custom icon.
            println!("cargo:warning=failed to embed Windows icon: {err}");
        }
    }
}

/// `git describe --tags --always --dirty`: the nearest tag (plus commits-ahead and
/// short SHA when HEAD isn't exactly on a tag), or the short SHA alone if no tag
/// is reachable. Returns `None` when git isn't present or the command fails.
fn git_describe() -> Option<String> {
    let out = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}
