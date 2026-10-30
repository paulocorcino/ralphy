//! Capture the git-published version at build time so the workbench's About
//! panel reports the same string that was tagged/released (e.g. `v0.1.0-rc2`)
//! rather than the Cargo manifest version, which can lag the tag. The rule is
//! shared with `ralphy-cli` in `ralphy-cli/build/git_version.rs`.
//!
//! It also writes the served workbench tree (`build/ui.rs`).

#[path = "../ralphy-cli/build/git_version.rs"]
mod git_version;
#[path = "build/parts.rs"]
mod parts;
#[path = "build/ui.rs"]
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    // lib.rs embeds the tree `ui::build` writes, which Cargo does not track on
    // its own: without this line an edit to a UI asset leaves the binary
    // serving the stale embedded copy after a "successful" rebuild.
    println!("cargo:rerun-if-changed=assets/ui");
    let out =
        PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR for a build script"))
            .join("ui");
    if let Err(e) = ui::build(&PathBuf::from("assets/ui"), &out) {
        eprintln!("error: build the workbench tree: {e:#}");
        return ExitCode::FAILURE;
    }

    let version = git_version::embedded_version();
    println!("cargo:rustc-env=RALPHY_VERSION={version}");
    ExitCode::SUCCESS
}
