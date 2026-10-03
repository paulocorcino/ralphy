//! `oversized` lists the files over the file-split threshold (ADR-0022): more
//! than [`SPLIT_AT`] lines of code above the file's inline test module. Comment
//! and blank lines do not count, so a well-commented file is not penalised. The
//! report never fails: the threshold is a recommendation, and the pull-request
//! job turns each listed file the PR touches into a warning.
//!
//! A line counts as a comment when it starts with `//` after its indentation.
//! A `/* … */` block is counted as code.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::asset_pins::repo_root;

const SPLIT_AT: usize = 500;

pub fn oversized_cmd(args: &[String]) -> Result<()> {
    if !args.is_empty() {
        anyhow::bail!("usage: oversized");
    }
    let root = repo_root();
    let mut files = Vec::new();
    collect_rs(&root.join("crates"), &mut files)?;
    let mut over = Vec::new();
    for path in &files {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let lines = code_lines(&text);
        if lines > SPLIT_AT {
            over.push((rel_path(&root, path)?, lines));
        }
    }
    over.sort();
    for (path, lines) in over {
        println!("{path}\t{lines}");
    }
    Ok(())
}

/// Lines of code above the first inline `#[cfg(test)] mod … {`, comment and
/// blank lines left out.
fn code_lines(text: &str) -> usize {
    let mut lines = text.lines().peekable();
    let mut count = 0;
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed == "#[cfg(test)]" && lines.peek().is_some_and(|next| opens_inline_mod(next)) {
            break;
        }
        if !trimmed.is_empty() && !trimmed.starts_with("//") {
            count += 1;
        }
    }
    count
}

/// `mod x {` or `pub(crate) mod x {`. A `mod x;` declaration is not the cut:
/// the code after it is still production code.
fn opens_inline_mod(line: &str) -> bool {
    let head = line.trim();
    let head = head.strip_prefix("pub(crate) ").unwrap_or(head);
    head.starts_with("mod ") && head.ends_with('{')
}

/// Production `.rs` files: no `tests/` dir, no `tests.rs`, no test child.
fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        let path = entry
            .with_context(|| format!("reading an entry of {}", dir.display()))?
            .path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            let skipped =
                matches!(name, "target" | "node_modules" | "tests") || name.starts_with('.');
            if !skipped {
                collect_rs(&path, out)?;
            }
        } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_test_child.rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// `path` relative to `root`, with `/` separators on every platform, so the
/// pull-request job can match it against `git diff --name-only`.
fn rel_path(root: &Path, path: &Path) -> Result<String> {
    let rel = path
        .strip_prefix(root)
        .with_context(|| format!("{} is not under {}", path.display(), root.display()))?;
    Ok(rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_blanks_and_the_test_module_do_not_count() {
        let src = "//! crate doc\n\
                   /// item doc\n\
                   fn a() {\n\
                   \n\
                       // why\n\
                       let x = 1; // trailing comment, still code\n\
                   }\n\
                   #[cfg(test)]\n\
                   mod tests;\n\
                   fn b() {}\n\
                   #[cfg(test)]\n\
                   pub(crate) mod more {\n\
                   fn t() {}\n\
                   }\n";
        assert_eq!(
            code_lines(src),
            6,
            "fn a, let, close, the `mod tests;` attribute and line, fn b"
        );
    }
}
