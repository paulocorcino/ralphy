//! Capture the git-published version at build time so the workbench's About
//! panel reports the same string that was tagged/released (e.g. `v0.1.0-rc2`)
//! rather than the Cargo manifest version, which can lag the tag. Mirrors
//! `ralphy-cli`'s build script; falls back to the Cargo version when git is
//! unavailable (e.g. a source tarball with no `.git`).

use std::process::Command;

fn main() {
    // Re-run when HEAD or the tag set moves so the embedded version stays current.
    // HEAD alone is not enough: a commit moves `refs/heads/<branch>`, not the
    // HEAD file, so the embedded version stayed at the previous commit until a
    // checkout. `packed-refs` is where fetched tags land, and `index` moves when
    // work is staged — which is what `--dirty` reports. (An edit that is never
    // staged can still leave a stale `-dirty`; nothing cheap observes that.)
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
    println!("cargo:rerun-if-changed=../../.git/packed-refs");
    println!("cargo:rerun-if-changed=../../.git/refs");
    // lib.rs embeds assets/ui via include_dir!, which Cargo does not track on
    // its own: without this line an edit to a UI asset leaves the binary
    // serving the stale embedded copy after a "successful" rebuild.
    println!("cargo:rerun-if-changed=assets/ui");

    let version = git_describe().unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rustc-env=RALPHY_VERSION={version}");
}

/// `git describe --tags --match 'v*' --always --dirty`: the nearest **version**
/// tag (plus commits-ahead and short SHA when HEAD isn't exactly on a tag), or
/// the short SHA alone if no tag is reachable. `--match` keeps the run's own
/// `ralphy/pre-run-*` tags out of the answer: without it the nearest tag by
/// commit distance can be one of those, and the binary then reports a version
/// that is not one (ADR-0056 §5). Returns `None` when git isn't present or the
/// command fails.
fn git_describe() -> Option<String> {
    let out = Command::new("git")
        .args(["describe", "--tags", "--match", "v*", "--always", "--dirty"])
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
