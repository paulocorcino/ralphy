//! `oversized` holds the size of every source file (ADR-0022 §5-§9) and the
//! width of its lines. Three modes:
//! - no argument lists the files over [`LIMIT`] lines of code, and the Rust
//!   files with a line over [`WIDTH`] characters;
//! - `--base <rev>` fails when such a count is larger than at `<rev>`;
//! - `--strict` counts every line, lists each file over [`LIMIT`] and each
//!   Rust or TypeScript line over [`WIDTH`] with its line number, and exits 0.
//!
//! A comment line starts with `//`, or is inside a `/* … */` block; in HTML,
//! text inside `<!-- … -->` is a comment.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::asset_pins::repo_root;
use crate::capabilities::{git, git_show};

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
}

/// Lines of code a source file may have, in every language: the split
/// threshold of ADR-0022 §5, which its 2026-10-30 amendment applies to
/// TypeScript and HTML too.
const LIMIT: usize = 500;

/// Characters a line of Rust may have: the default `max_width` of rustfmt,
/// so the code `cargo fmt` wraps already fits, and only a string literal or
/// a comment, which stable rustfmt does not wrap, goes over (ADR-0022 §8).
/// Characters, not bytes: an accent counts once.
const WIDTH: usize = 100;

/// How a file is counted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Count {
    /// Code lines, and Rust width only.
    Code,
    /// Every line, and Rust and TypeScript width.
    Every,
}

/// What the gate counts in a file. Once a count is over its limit, it may
/// not grow against the base branch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Rule {
    /// Lines of code, over [`LIMIT`].
    Size,
    /// Lines of Rust wider than [`WIDTH`], over none.
    Width,
}

impl Rule {
    const ALL: [Rule; 2] = [Rule::Size, Rule::Width];

    /// The count of `text`, or `None` when the rule does not measure `lang`.
    fn count(self, lang: Lang, text: &str, count: Count) -> Option<usize> {
        match (self, count) {
            (Rule::Size, Count::Code) => Some(lines_of_code(lang, text)),
            (Rule::Size, Count::Every) => Some(text.lines().count()),
            (Rule::Width, _) => measures_width(lang, count).then(|| wide_lines(text)),
        }
    }

    fn limit(self) -> usize {
        match self {
            Rule::Size => LIMIT,
            Rule::Width => 0,
        }
    }

    fn label(self) -> String {
        match self {
            Rule::Size => format!("lines of code, over the limit of {LIMIT}"),
            Rule::Width => format!("lines wider than {WIDTH} characters"),
        }
    }

    /// What a change does when the count of a file grew.
    fn fix(self) -> &'static str {
        match self {
            Rule::Size => {
                "split the file along a responsibility it already has, in a commit that \
                 changes no behavior"
            }
            Rule::Width => {
                "wrap the comment, continue the string on the next line with `\\` or \
                 `concat!`, or move the test data to `tests/fixtures/`"
            }
        }
    }
}

/// A count over its limit, with the path of its file from the repo root
/// with `/` separators.
#[derive(Debug)]
struct Oversized {
    rule: Rule,
    path: String,
    lines: usize,
}

pub fn oversized_cmd(args: &[String]) -> Result<()> {
    let root = repo_root();
    match args {
        [] => {
            for file in &measure(&root)? {
                println!("{}	{}	{}", file.path, file.lines, file.rule.label());
            }
            Ok(())
        }
        [flag] if flag == "--strict" => {
            for row in &strict_report(&root)? {
                println!("{}", row.render());
            }
            Ok(())
        }
        [flag, rev] if flag == "--base" => check_growth(&root, rev),
        _ => bail!("usage: oversized [--base <rev> | --strict]"),
    }
}

/// Fails when a count over its limit is larger at `base` than it is now.
fn check_growth(root: &Path, base: &str) -> Result<()> {
    // A wrong rev would read every file as new there.
    let commit = format!("{base}^{{commit}}");
    git(root, &["rev-parse", "--verify", "--quiet", &commit])
        .with_context(|| format!("{base} is not a commit"))?;
    let mut errors = Vec::new();
    for file in &measure(root)? {
        errors.extend(growth_since(file, &git_show(root, base, &file.path)?));
    }
    if errors.is_empty() {
        return Ok(());
    }
    for error in &errors {
        eprintln!("{error}");
    }
    bail!(
        "{} count(s) over a limit grew against {base}; each line above says what to do",
        errors.len()
    )
}

/// A row of `oversized --strict`.
#[derive(Debug, PartialEq, Eq)]
enum StrictRow {
    /// A file over [`LIMIT`] lines, every line counted.
    Size { path: String, lines: usize },
    /// A line wider than [`WIDTH`] characters.
    Width {
        path: String,
        line: usize,
        width: usize,
    },
}

impl StrictRow {
    fn path(&self) -> &str {
        match self {
            StrictRow::Size { path, .. } | StrictRow::Width { path, .. } => path,
        }
    }

    /// The line a row is about; a file row is about none.
    fn line(&self) -> usize {
        match self {
            StrictRow::Size { .. } => 0,
            StrictRow::Width { line, .. } => *line,
        }
    }

    fn render(&self) -> String {
        match self {
            StrictRow::Size { path, lines } => {
                format!("{path}	{lines}	lines, over the limit of {LIMIT} (every line counted)")
            }
            StrictRow::Width { path, line, width } => {
                format!("{path}:{line}	{width}	characters, over the width of {WIDTH}")
            }
        }
    }
}

/// Every file over [`LIMIT`] lines and every line over [`WIDTH`] characters
/// under `crates/`, sorted by path, then by line.
fn strict_report(root: &Path) -> Result<Vec<StrictRow>> {
    let mut files = Vec::new();
    collect_sources(&root.join("crates"), &mut files)?;
    let mut rows = Vec::new();
    for (path, lang) in files {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        rows.extend(strict_rows_of(rel_path(root, &path)?, lang, &text));
    }
    rows.sort_by(|a, b| (a.path(), a.line()).cmp(&(b.path(), b.line())));
    Ok(rows)
}

/// The rows of one file.
fn strict_rows_of(path: String, lang: Lang, text: &str) -> Vec<StrictRow> {
    let mut rows = Vec::new();
    let lines = Rule::Size.count(lang, text, Count::Every).unwrap_or(0);
    if lines > LIMIT {
        rows.push(StrictRow::Size {
            path: path.clone(),
            lines,
        });
    }
    if measures_width(lang, Count::Every) {
        for (line, width) in wide_line_rows(text) {
            rows.push(StrictRow::Width {
                path: path.clone(),
                line,
                width,
            });
        }
    }
    rows
}

/// Every count over its limit in a source file under `crates/`, sorted by
/// path.
fn measure(root: &Path) -> Result<Vec<Oversized>> {
    let mut files = Vec::new();
    collect_sources(&root.join("crates"), &mut files)?;
    let mut over = Vec::new();
    for (path, lang) in files {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        for (rule, lines) in counts_over(lang, &text) {
            over.push(Oversized {
                rule,
                path: rel_path(root, &path)?,
                lines,
            });
        }
    }
    // A stable sort: the counts of one file keep the order of `Rule::ALL`.
    over.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(over)
}

/// Each rule whose count of `text` is over its limit, with the count.
fn counts_over(lang: Lang, text: &str) -> Vec<(Rule, usize)> {
    Rule::ALL
        .into_iter()
        .filter_map(|rule| Some((rule, rule.count(lang, text, Count::Code)?)))
        .filter(|(rule, lines)| *lines > rule.limit())
        .collect()
}

/// The error for a count over its limit that is larger than the `before` it
/// had at the base. A file the base does not have, under its path, counts 0
/// there.
fn growth_error(file: &Oversized, before: usize) -> Option<String> {
    (file.lines > before).then(|| {
        format!(
            "{}: {before} -> {} {}; {}",
            file.path,
            file.lines,
            file.rule.label(),
            file.rule.fix()
        )
    })
}

/// The error for `file` when its count grew against `base_text`, the text it
/// had at the base. Both sides are counted the way the file's own language
/// counts.
fn growth_since(file: &Oversized, base_text: &str) -> Option<String> {
    let lang = Lang::of(&file.path)?;
    growth_error(file, file.rule.count(lang, base_text, Count::Code)?)
}

/// Whether the width rule reads files of `lang` under `count`.
fn measures_width(lang: Lang, count: Count) -> bool {
    match count {
        Count::Code => lang == Lang::Rust,
        Count::Every => matches!(lang, Lang::Rust | Lang::TypeScript),
    }
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

/// Lines of `text` wider than [`WIDTH`] characters.
fn wide_lines(text: &str) -> usize {
    wide_line_rows(text).len()
}

/// The lines of code of `text`, counted by the rules of `lang`.
fn lines_of_code(lang: Lang, text: &str) -> usize {
    match lang {
        Lang::Rust | Lang::TypeScript => code_lines(text),
        Lang::Html => html_code_lines(text),
    }
}

/// Lines of HTML that have something outside `<!-- … -->`. A comment may
/// span lines.
fn html_code_lines(text: &str) -> usize {
    let mut in_comment = false;
    text.lines()
        .filter(|line| html_line_has_code(line, &mut in_comment))
        .count()
}

/// Whether `line` has text outside a comment. `in_comment` says whether the
/// line starts inside a comment, and is left saying whether it ends inside
/// one.
fn html_line_has_code(line: &str, in_comment: &mut bool) -> bool {
    let mut rest = line;
    let mut code = false;
    while let Some((text, tail)) = html_next_piece(rest, *in_comment) {
        code |= has_text(text);
        *in_comment = !*in_comment;
        rest = tail;
    }
    code || (!*in_comment && has_text(rest))
}

/// The text of `rest` up to the next comment boundary, and what follows the
/// boundary. Outside a comment the boundary is `<!--`, inside one it is
/// `-->`; the text of a piece inside a comment is not code, so the caller
/// reads it only when it is outside one.
fn html_next_piece(rest: &str, in_comment: bool) -> Option<(&str, &str)> {
    let (text, tail) = rest.split_once(if in_comment { "-->" } else { "<!--" })?;
    Some((if in_comment { "" } else { text }, tail))
}

fn has_text(text: &str) -> bool {
    !text.trim().is_empty()
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
        if is_code(trimmed) {
            count += 1;
        }
    }
    count
}

/// A trimmed line outside a block comment holds code when it is not blank
/// and not a line comment.
fn is_code(trimmed: &str) -> bool {
    !trimmed.is_empty() && !trimmed.starts_with("//")
}

/// `mod x {` or `pub(crate) mod x {`. A `mod x;` declaration is not the cut:
/// the code after it is still production code.
fn opens_inline_mod(line: &str) -> bool {
    let head = line.trim();
    let head = head.strip_prefix("pub(crate) ").unwrap_or(head);
    head.starts_with("mod ") && head.ends_with('{')
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
mod tests;
