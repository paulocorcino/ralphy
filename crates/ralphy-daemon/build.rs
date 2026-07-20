//! Capture the git-published version at build time so the workbench's About
//! panel reports the same string that was tagged/released (e.g. `v0.1.0-rc2`)
//! rather than the Cargo manifest version, which can lag the tag. Mirrors
//! `ralphy-cli`'s build script; falls back to the Cargo version when git is
//! unavailable (e.g. a source tarball with no `.git`).

use std::process::Command;

fn main() {
    // Re-run when HEAD or the tag set moves so the embedded version stays current.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/tags");

    let version = git_describe().unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rustc-env=RALPHY_VERSION={version}");
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
