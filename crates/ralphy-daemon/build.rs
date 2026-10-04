//! Capture the git-published version at build time so the workbench's About
//! panel reports the same string that was tagged/released (e.g. `v0.1.0-rc2`)
//! rather than the Cargo manifest version, which can lag the tag. The rule is
//! shared with `ralphy-cli` in `ralphy-cli/build/git_version.rs`.

#[path = "../ralphy-cli/build/git_version.rs"]
mod git_version;

fn main() {
    // lib.rs embeds assets/ui via include_dir!, which Cargo does not track on
    // its own: without this line an edit to a UI asset leaves the binary
    // serving the stale embedded copy after a "successful" rebuild.
    println!("cargo:rerun-if-changed=assets/ui");

    let version = git_version::embedded_version();
    println!("cargo:rustc-env=RALPHY_VERSION={version}");
}
