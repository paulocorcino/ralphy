//! `oversized` holds the size of every source file (ADR-0022 §7). A Rust
//! file may have [`Lang::Rust`]'s limit of lines of code, and a TypeScript
//! file [`Lang::TypeScript`]'s. With no argument it lists the files over
//! their limit. With `--base <rev>` it fails when a file over its limit is
//! larger than at `<rev>`: an old large file may shrink but never grow, and a
//! new file, or a file that grows past the limit, is split. The pull-request
//! job runs it against the base branch, so no list of large files is kept by
//! hand.
//!
//! Comment and blank lines do not count, so a well-commented file is not
//! penalised. A line counts as a comment when it starts with `//` after its
//! indentation, or when it starts with `/*` or is inside the block comment
//! such a line opens. Code after a `*/` on the same line is not counted.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::asset_pins::repo_root;
use crate::capabilities::{git, git_show};

/// The two languages of the repo's source files. A file of any other
/// extension is not measured.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lang {
    Rust,
    TypeScript,
}

impl Lang {
    fn of(name: &str) -> Option<Lang> {
        if name.ends_with(".rs") {
            Some(Lang::Rust)
        } else if name.ends_with(".ts") {
            Some(Lang::TypeScript)
        } else {
            None
        }
    }

    /// Lines of code a file may have. Rust: the split threshold of ADR-0022
    /// §5. TypeScript: the module size of ADR-0073 (its 2026-10-11
    /// amendment), so an agent can read a workbench module whole.
    fn limit(self) -> usize {
        match self {
            Lang::Rust => 500,
            Lang::TypeScript => 700,
        }
    }
}

/// A file over its limit, by its path from the repo root with `/`
/// separators.
#[derive(Debug)]
struct Oversized {
    path: String,
    lines: usize,
    limit: usize,
}

pub fn oversized_cmd(args: &[String]) -> Result<()> {
    let base = match args {
        [] => None,
        [flag, rev] if flag == "--base" => Some(rev.as_str()),
        _ => bail!("usage: oversized [--base <rev>]"),
    };
    let root = repo_root();
    let over = measure(&root)?;
    let Some(base) = base else {
        for file in &over {
            println!("{}\t{}\t{}", file.path, file.lines, file.limit);
        }
        return Ok(());
    };
    // A wrong rev would read every file as new there.
    let commit = format!("{base}^{{commit}}");
    git(&root, &["rev-parse", "--verify", "--quiet", &commit])
        .with_context(|| format!("{base} is not a commit"))?;
    let mut errors = Vec::new();
    for file in &over {
        let before = code_lines(&git_show(&root, base, &file.path)?);
        errors.extend(growth_error(file, before));
    }
    if errors.is_empty() {
        return Ok(());
    }
    for error in &errors {
        eprintln!("{error}");
    }
    bail!(
        "{} file(s) over the size limit grew against {base}; split each one along a \
         responsibility it already has, in a commit that changes no behavior",
        errors.len()
    )
}

/// Every source file under `crates/` that is over its limit, sorted by path.
fn measure(root: &Path) -> Result<Vec<Oversized>> {
    let mut files = Vec::new();
    collect_sources(&root.join("crates"), &mut files)?;
    let mut over = Vec::new();
    for (path, lang) in files {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let lines = code_lines(&text);
        if lines > lang.limit() {
            over.push(Oversized {
                path: rel_path(root, &path)?,
                lines,
                limit: lang.limit(),
            });
        }
    }
    over.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(over)
}

/// The error for a file over its limit that has more lines of code than the
/// `before` it had at the base. A file the base does not have, under its
/// path, has 0 lines there.
fn growth_error(file: &Oversized, before: usize) -> Option<String> {
    (file.lines > before).then(|| {
        format!(
            "{}: {before} -> {} lines of code, over the limit of {}",
            file.path, file.lines, file.limit
        )
    })
}

/// Lines of code above the first inline `#[cfg(test)] mod … {`, comment and
/// blank lines left out. A TypeScript file has no such module, so all of it
/// counts.
fn code_lines(text: &str) -> usize {
    let mut lines = text.lines().peekable();
    let mut count = 0;
    let mut in_block = false;
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if in_block {
            in_block = !trimmed.contains("*/");
            continue;
        }
        if trimmed == "#[cfg(test)]" && lines.peek().is_some_and(|next| opens_inline_mod(next)) {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("/*") {
            in_block = !rest.contains("*/");
            continue;
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

/// Every `.rs` and `.ts` file under `dir`, test code included. Not read:
/// build output, installed packages, vendored code and hidden directories.
fn collect_sources(dir: &Path, out: &mut Vec<(PathBuf, Lang)>) -> Result<()> {
    let entries = std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        let path = entry
            .with_context(|| format!("reading an entry of {}", dir.display()))?
            .path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            let skipped =
                matches!(name, "target" | "node_modules" | "vendor") || name.starts_with('.');
            if !skipped {
                collect_sources(&path, out)?;
            }
        } else if let Some(lang) = Lang::of(name) {
            out.push((path, lang));
        }
    }
    Ok(())
}

/// `path` relative to `root`, with `/` separators on every platform, so it
/// names the same file in `git show <rev>:<path>`.
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

    #[test]
    fn a_block_comment_does_not_count() {
        let src = "/**\n\
                    * A doc block.\n\
                    * let x = 1;\n\
                    */\n\
                   const a = 1;\n\
                   /* one line */\n\
                   let b = *p;\n";
        assert_eq!(code_lines(src), 2, "`const a` and `let b` only");
    }

    #[test]
    fn each_language_has_its_limit() {
        assert_eq!(Lang::of("lib.rs").map(Lang::limit), Some(500));
        assert_eq!(Lang::of("wb-desk.ts").map(Lang::limit), Some(700));
        assert_eq!(Lang::of("globals.d.ts").map(Lang::limit), Some(700));
        assert_eq!(Lang::of("app.js"), None);
    }

    #[test]
    fn a_file_over_its_limit_may_shrink_but_not_grow() {
        let file = Oversized {
            path: "crates/a/src/big.rs".to_string(),
            lines: 900,
            limit: 500,
        };
        assert_eq!(growth_error(&file, 900), None, "the same size");
        assert_eq!(growth_error(&file, 950), None, "it shrank");
        let grown = growth_error(&file, 899).expect("it grew by one line");
        assert!(
            grown.starts_with("crates/a/src/big.rs: 899 -> 900"),
            "{grown}"
        );
        assert!(
            growth_error(&file, 0).is_some(),
            "a file the base does not have"
        );
    }

    #[test]
    fn tests_are_read_and_vendored_code_is_not() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let root = dir.path();
        for rel in [
            "a/src/lib.rs",
            "a/tests/flow.rs",
            "ui/app.ts",
            "ui/vendor/alpine.esm.d.ts",
            "ui/node_modules/x/index.ts",
            "a/target/debug/build.rs",
            "ui/app.js",
        ] {
            let path = root.join(rel);
            let parent = path.parent().expect("a relative path with a parent");
            std::fs::create_dir_all(parent).expect("create the parent");
            std::fs::write(&path, "").expect("write the file");
        }
        let mut found = Vec::new();
        collect_sources(root, &mut found).expect("collect");
        let mut names: Vec<String> = found
            .iter()
            .map(|(path, _)| rel_path(root, path).expect("under the root"))
            .collect();
        names.sort();
        assert_eq!(names, ["a/src/lib.rs", "a/tests/flow.rs", "ui/app.ts"]);
    }
}
