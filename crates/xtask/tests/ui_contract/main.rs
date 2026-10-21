//! The contract between the browser and the daemon (ADR-0070 Compliance).
//!
//! - Shared replies: every message type the UI reads, and how many of them
//!   have no reply file under `crates/ralphy-daemon/ui-tests/fixtures/`.
//!   Each ok reply file carries its JSON in the field the daemon uses.
//! - Error literals: every string the UI compares with a reply's `reason`,
//!   `message` or `state` is a literal in the daemon's or the CLI's Rust code.
//! - Mirrored limits: each value the UI repeats equals its Rust constant.
//!
//! Each check reads both sides as text, like `ratchets/`: xtask may not
//! depend on the daemon.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use regex::Regex;

mod error_literals;
mod mirrored_limits;
mod reply_fields;
mod shared_replies;

const FIXTURES: &str = "crates/ralphy-daemon/ui-tests/fixtures";

/// The fixture file names, without `.json`.
fn fixture_names(root: &Path) -> BTreeSet<String> {
    let dir = root.join(FIXTURES);
    std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|e| panic!("reading an entry of {}: {e}", dir.display()))
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter_map(|name| name.strip_suffix(".json").map(str::to_string))
        .collect()
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// The part of a file above its first inline test module, the same cut the
/// file-split rule uses (ADR-0022).
fn production(text: &str) -> &str {
    let mut offset = 0;
    let mut lines = text.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        if line.trim() == "#[cfg(test)]" && lines.peek().is_some_and(|next| opens_inline_mod(next))
        {
            return &text[..offset];
        }
        offset += line.len();
    }
    text
}

/// `mod x {` or `pub(crate) mod x {`. A `mod x;` declaration is not a cut.
fn opens_inline_mod(line: &str) -> bool {
    let head = line.trim();
    let head = head.strip_prefix("pub(crate) ").unwrap_or(head);
    head.starts_with("mod ") && head.ends_with('{')
}

/// Production `.rs` files: no `tests/` dir, no `tests.rs`, no test child.
fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("reading an entry of {}: {e}", dir.display()))
            .path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            if name != "tests" {
                collect_rs(&path, out);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_test_child.rs") {
            out.push(path);
        }
    }
}
