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
#[path = "build/ui_gzip.rs"]
mod ui_gzip;
#[path = "build/ui_import.rs"]
mod ui_import;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use std::process::ExitCode;

/// The wording catalogs, one `<domain>.json` per text domain (ADR-0078 D9).
const WORDING: &str = "../../assets/wording";

/// Copy each catalog to the served tree, where a module imports it as
/// `./wording/<domain>.json`.
fn copy_wording(src: &Path, out: &Path) -> Result<()> {
    fs::create_dir_all(out).with_context(|| format!("create {}", out.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("read {}", src.display()))? {
        let path = entry
            .with_context(|| format!("read an entry of {}", src.display()))?
            .path();
        if path.extension().is_some_and(|e| e == "json") {
            let name = path.file_name().context("a catalog has a file name")?;
            fs::copy(&path, out.join(name)).with_context(|| format!("copy {}", path.display()))?;
        }
    }
    Ok(())
}

#[expect(
    clippy::print_stderr,
    reason = "a build script reports its failure to Cargo on stderr"
)]
fn main() -> ExitCode {
    // lib.rs embeds the tree `ui::build` writes, which Cargo does not track on
    // its own: without this line an edit to a UI asset leaves the binary
    // serving the stale embedded copy after a "successful" rebuild.
    println!("cargo:rerun-if-changed=assets/ui");
    println!("cargo:rerun-if-changed={WORDING}");
    let out_dir =
        PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR for a build script"));
    let out = out_dir.join("ui");
    if let Err(e) = ui::build(&PathBuf::from("assets/ui"), &out) {
        eprintln!("error: build the workbench tree: {e:#}");
        return ExitCode::FAILURE;
    }
    if let Err(e) = copy_wording(Path::new(WORDING), &out.join("wording")) {
        eprintln!("error: copy the wording catalogs: {e:#}");
        return ExitCode::FAILURE;
    }
    if let Err(e) = ui_gzip::store(&out, &out_dir.join("ui-gz")) {
        eprintln!("error: store the vendored files gzipped: {e:#}");
        return ExitCode::FAILURE;
    }

    let version = git_version::embedded_version();
    println!("cargo:rustc-env=RALPHY_VERSION={version}");
    ExitCode::SUCCESS
}
