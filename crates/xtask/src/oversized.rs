//! `oversized` holds the size of every source file (ADR-0022 §9) and the
//! width of its lines: a file has at most [`LIMIT`] lines, every line
//! counted, and a line of Rust or TypeScript at most [`WIDTH`] characters.
//! No argument lists each file and line over a limit; `--check` fails when
//! there is one. There is no exception for an old file.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::asset_pins::repo_root;

/// The languages of the repo's source files. A file of any other extension
/// is not measured.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lang {
    Rust,
    TypeScript,
    Html,
}

impl Lang {
    fn of(name: &str) -> Option<Lang> {
        if name.ends_with(".rs") {
            Some(Lang::Rust)
        } else if name.ends_with(".ts") {
            Some(Lang::TypeScript)
        } else if name.ends_with(".html") {
            Some(Lang::Html)
        } else {
            None
        }
    }

    /// Whether the width rule reads files of this language.
    fn has_width(self) -> bool {
        match self {
            Lang::Rust | Lang::TypeScript => true,
            Lang::Html => false,
        }
    }
}

/// Lines a source file may have in every language, code, comments, blank
/// lines and tests counted (ADR-0022 §9).
const LIMIT: usize = 500;

/// Characters a line of Rust or TypeScript may have: the default `max_width`
/// of rustfmt, so the code `cargo fmt` wraps already fits, and only a string
/// literal or a comment goes over (ADR-0022 §9). Characters, not bytes: an
/// accent counts once.
const WIDTH: usize = 100;

const USAGE: &str = "usage: oversized [--check]";

/// A file or a line over a limit, with the path of its file from the repo
/// root with `/` separators.
#[derive(Debug, PartialEq, Eq)]
enum Row {
    /// A file over [`LIMIT`] lines.
    Size { path: String, lines: usize },
    /// A line wider than [`WIDTH`] characters.
    Width {
        path: String,
        line: usize,
        width: usize,
    },
}

impl Row {
    fn path(&self) -> &str {
        match self {
            Row::Size { path, .. } | Row::Width { path, .. } => path,
        }
    }

    /// The line a row is about; a file row is about none.
    fn line(&self) -> usize {
        match self {
            Row::Size { .. } => 0,
            Row::Width { line, .. } => *line,
        }
    }

    fn render(&self) -> String {
        match self {
            Row::Size { path, lines } => {
                format!("{path}	{lines}	lines, over the limit of {LIMIT} (every line counted)")
            }
            Row::Width { path, line, width } => {
                format!("{path}:{line}	{width}	characters, over the width of {WIDTH}")
            }
        }
    }

    /// What a change does to fit the rule of the row.
    fn fix(&self) -> &'static str {
        match self {
            Row::Size { .. } => {
                "split the file along a responsibility it already has, in a commit that \
                 changes no behavior"
            }
            Row::Width { .. } => {
                "wrap the comment, continue the string on the next line with `\\` or \
                 `concat!` (in TypeScript, join `\"…\" +` pieces), or move the test data \
                 to `tests/fixtures/`"
            }
        }
    }
}

pub(crate) fn oversized_cmd(args: &[String]) -> Result<()> {
    match args {
        [] => {
            for row in &report(&repo_root())? {
                println!("{}", row.render());
            }
            Ok(())
        }
        [flag] if flag == "--check" => check(&repo_root()),
        _ => bail!(USAGE),
    }
}

/// Fails when a file or a line under `crates/` of `root` is over a limit;
/// the error lists each one with what to do.
fn check(root: &Path) -> Result<()> {
    let rows = report(root)?;
    if rows.is_empty() {
        return Ok(());
    }
    let list: Vec<String> = rows
        .iter()
        .map(|row| format!("{}; {}", row.render(), row.fix()))
        .collect();
    bail!(
        "{} file(s) or line(s) over a limit:\n{}",
        rows.len(),
        list.join("\n")
    )
}

/// Every file over [`LIMIT`] lines and every line over [`WIDTH`] characters
/// under `crates/`, sorted by path, then by line.
fn report(root: &Path) -> Result<Vec<Row>> {
    let mut files = Vec::new();
    collect_sources(&root.join("crates"), &mut files)?;
    let mut rows = Vec::new();
    for (path, lang) in files {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        rows.extend(rows_of(&rel_path(root, &path)?, lang, &text));
    }
    rows.sort_by(|a, b| (a.path(), a.line()).cmp(&(b.path(), b.line())));
    Ok(rows)
}

/// The rows of one file.
fn rows_of(path: &str, lang: Lang, text: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    let lines = text.lines().count();
    if lines > LIMIT {
        rows.push(Row::Size {
            path: path.to_owned(),
            lines,
        });
    }
    if lang.has_width() {
        for (line, width) in wide_line_rows(text) {
            rows.push(Row::Width {
                path: path.to_owned(),
                line,
                width,
            });
        }
    }
    rows
}

/// The line number (from 1) and the width of each line of `text` wider than
/// [`WIDTH`] characters.
fn wide_line_rows(text: &str) -> Vec<(usize, usize)> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let width = line.chars().count();
            (width > WIDTH).then_some((index + 1, width))
        })
        .collect()
}

/// Every `.rs`, `.ts` and `.html` file under `dir`, test code included. Not read:
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

/// `path` relative to `root`, with `/` separators on every platform.
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
mod tests;
