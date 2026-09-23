//! The inline-test budget (ADR-0022 §6): an inline `#[cfg(test)] mod … { … }`
//! longer than [`BUDGET`] lines moves to a sibling file (`foo/tests.rs`, or
//! `src/tests.rs` for a crate root), whatever the file's production size.
//!
//! The file-split threshold counts production lines only, so a file whose bulk
//! is its test module never trips it; this is the gate that does.

use std::path::{Path, PathBuf};

const BUDGET: usize = 500;

#[test]
fn no_inline_test_module_outgrows_the_budget() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    collect_rs(&crates, &mut files);
    assert!(
        files.len() > 100,
        "expected the workspace's sources under {}, found {} .rs files",
        crates.display(),
        files.len()
    );

    let mut over = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        for (line, len) in inline_test_modules(&text) {
            if len > BUDGET {
                over.push(format!("{}:{line} ({len} lines)", path.display()));
            }
        }
    }
    assert!(
        over.is_empty(),
        "inline #[cfg(test)] modules over {BUDGET} lines — move each to a sibling \
         `tests.rs` (ADR-0022 §6):\n{}",
        over.join("\n")
    );
}

#[test]
fn a_module_is_measured_from_its_attribute_to_its_closing_brace() {
    let src = "fn a() {}\n\n#[cfg(test)]\nmod tests {\n    fn b() {\n    }\n}\n";
    assert_eq!(inline_test_modules(src), vec![(3, 5)]);
    // An out-of-line module has no body to measure.
    assert!(inline_test_modules("#[cfg(test)]\nmod tests;\n").is_empty());
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("reading an entry of {}: {e}", dir.display()))
            .path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            if !matches!(name, "target" | "node_modules") && !name.starts_with('.') {
                collect_rs(&path, out);
            }
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

/// `(1-based line of the #[cfg(test)], length in lines)` for each inline test
/// module. rustfmt puts a module's closing brace alone at the module's own
/// indentation, which is what makes the brace match a line match.
fn inline_test_modules(text: &str) -> Vec<(usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    for (i, pair) in lines.windows(2).enumerate() {
        if pair[0].trim() != "#[cfg(test)]" {
            continue;
        }
        let head = pair[1].trim_start();
        let indent = &pair[1][..pair[1].len() - head.len()];
        let head = head.strip_prefix("pub(crate) ").unwrap_or(head);
        if !(head.starts_with("mod ") && head.trim_end().ends_with('{')) {
            continue;
        }
        let close = format!("{indent}}}");
        if let Some(end) = lines[i + 2..].iter().position(|l| *l == close) {
            found.push((i + 1, end + 3));
        }
    }
    found
}
