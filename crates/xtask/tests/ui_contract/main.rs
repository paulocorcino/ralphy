//! The contract between the browser and the daemon (ADR-0070 Compliance).
//!
//! - Shared replies: every message type the UI reads, and how many of them
//!   have no reply file under `crates/ralphy-daemon/ui-tests/fixtures/`.
//!   Each ok reply file carries its JSON in the field the daemon uses.
//! - Error literals: every string the UI compares with a reply's `reason`,
//!   `message` or `state` is a literal in the daemon's or the CLI's Rust code.
//! - Mirrored limits: each value the UI repeats equals its Rust constant.
//! - Cited routes: every `/api/…` or `/ws/…` path the repo cites is served.
//!
//! Each check reads both sides as text, like `ratchets/`: xtask may not
//! depend on the daemon.
#![expect(
    clippy::tests_outside_test_module,
    reason = "an integration test crate is the test module"
)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use regex::Regex;

#[path = "../common/production_cut.rs"]
mod production_cut;
use production_cut::production;

mod cited_routes;
mod error_literals;
mod mirrored_limits;
mod reply_fields;
mod shared_replies;

const FIXTURES: &str = "crates/ralphy-daemon/ui-tests/fixtures";

/// The fixture file names, without `.json`.
fn fixture_names(root: &Path) -> BTreeSet<String> {
    let dir = root.join(FIXTURES);
    std::fs::read_dir(&dir)
        .map_err(|e| format!("reading {}: {e}", dir.display()))
        .expect("this test needs the call to succeed")
        .map(|entry| {
            entry
                .map_err(|e| format!("reading an entry of {}: {e}", dir.display()))
                .expect("this test needs the call to succeed")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter_map(|name| name.strip_suffix(".json").map(str::to_owned))
        .collect()
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .map_err(|e| format!("reading {}: {e}", path.display()))
        .expect("this test needs the call to succeed")
}

/// Production `.rs` files: no `tests/` dir, no `tests.rs`, no test child.
fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("reading {}: {e}", dir.display()))
        .expect("this test needs the call to succeed");
    for entry in entries {
        let path = entry
            .map_err(|e| format!("reading an entry of {}: {e}", dir.display()))
            .expect("this test needs the call to succeed")
            .path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            if name != "tests" {
                collect_rs(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs")
            && name != "tests.rs"
            && !name.ends_with("_test_child.rs")
        {
            out.push(path);
        }
    }
}
