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

    // Emit the comma-joined list of engineering-skill subdirectory names so the
    // binary can tell the dev which skills will be installed (informational; the
    // downloaded set comes from the pinned tag, which may differ across versions).
    println!("cargo:rerun-if-changed=../../assets/agents_template/skills");
    let skills_csv = skills_csv("../../assets/agents_template/skills");
    println!("cargo:rustc-env=RALPHY_SKILLS={skills_csv}");
}

/// Read immediate subdirectory names from `skills_path`, sort them, and join with
/// commas. Returns an empty string on any read failure so the build still succeeds.
fn skills_csv(skills_path: &str) -> String {
    let Ok(rd) = std::fs::read_dir(skills_path) else {
        return String::new();
    };
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names.join(",")
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
