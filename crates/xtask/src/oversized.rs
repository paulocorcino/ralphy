//! `oversized` holds the size of every source file (ADR-0022 §7) and the
//! width of its lines (§8). A Rust, TypeScript or HTML file may have
//! [`LIMIT`] lines of code, and a line of Rust [`WIDTH`] characters. With no
//! argument it lists the files over a limit. With `--base <rev>` it fails
//! when a count over its limit is larger than at `<rev>`: an old large file
//! may shrink but never grow, and a new file, or a file that grows past the
//! limit, is split; an old wide line may stay, and a new one is wrapped. The
//! pull-request job runs it against the base branch, so no list of large
//! files is kept by hand.
//!
//! Comment and blank lines do not count, so a well-commented file is not
//! penalised. A line counts as a comment when it starts with `//` after its
//! indentation, or when it starts with `/*` or is inside the block comment
//! such a line opens. Code after a `*/` on the same line is not counted. In an
//! HTML file, text inside `<!-- … -->` is a comment, and a line with code
//! before or after a comment is code.

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
    fn count(self, lang: Lang, text: &str) -> Option<usize> {
        match self {
            Rule::Size => Some(lines_of_code(lang, text)),
            Rule::Width => (lang == Lang::Rust).then(|| wide_lines(text)),
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
    let base = match args {
        [] => None,
        [flag, rev] if flag == "--base" => Some(rev.as_str()),
        _ => bail!("usage: oversized [--base <rev>]"),
    };
    let root = repo_root();
    let over = measure(&root)?;
    let Some(base) = base else {
        for file in &over {
            println!("{}\t{}\t{}", file.path, file.lines, file.rule.label());
        }
        return Ok(());
    };
    // A wrong rev would read every file as new there.
    let commit = format!("{base}^{{commit}}");
    git(&root, &["rev-parse", "--verify", "--quiet", &commit])
        .with_context(|| format!("{base} is not a commit"))?;
    let mut errors = Vec::new();
    for file in &over {
        errors.extend(growth_since(file, &git_show(&root, base, &file.path)?));
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
        .filter_map(|rule| Some((rule, rule.count(lang, text)?)))
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
    growth_error(file, file.rule.count(lang, base_text)?)
}

/// Lines of `text` wider than [`WIDTH`] characters.
fn wide_lines(text: &str) -> usize {
    text.lines()
        .filter(|line| line.chars().count() > WIDTH)
        .count()
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
    fn every_language_has_a_limit_of_500_code_lines() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let root = dir.path();
        for (rel, lines, line) in [
            ("crates/a/src/big.rs", 501, "let a = 1;\n"),
            ("crates/a/src/fits.rs", 500, "let a = 1;\n"),
            ("crates/ui/big.ts", 501, "const a = 1;\n"),
            ("crates/ui/fits.ts", 500, "const a = 1;\n"),
            ("crates/ui/globals.d.ts", 501, "declare const a: 1;\n"),
            ("crates/ui/big.part.html", 501, "<p>x</p>\n"),
            ("crates/ui/fits.html", 500, "<p>x</p>\n"),
            ("crates/ui/app.js", 501, "const a = 1;\n"),
        ] {
            let path = root.join(rel);
            let parent = path.parent().expect("a relative path with a parent");
            std::fs::create_dir_all(parent).expect("create the parent");
            std::fs::write(&path, line.repeat(lines)).expect("write the file");
        }
        let over: Vec<String> = measure(root)
            .expect("measure")
            .into_iter()
            .map(|file| file.path)
            .collect();
        assert_eq!(
            over,
            [
                "crates/a/src/big.rs",
                "crates/ui/big.part.html",
                "crates/ui/big.ts",
                "crates/ui/globals.d.ts",
            ]
        );
    }

    #[test]
    fn an_html_comment_is_not_code() {
        let src = "<div>
                   <!-- a comment
                   that spans
                   <p>not code</p>
                   -->
                   <!-- one line -->
                   
                   <b>x</b> <!-- after code -->
                   </div>
";
        assert_eq!(
            lines_of_code(Lang::Html, src),
            3,
            "`<div>`, the line with code before a comment, `</div>`"
        );
        let after = "<!-- opens
closes --> <i>y</i>
";
        assert_eq!(lines_of_code(Lang::Html, after), 1, "code after `-->`");
    }

    #[test]
    fn a_page_that_grows_past_its_limit_fails() {
        let page = |code: usize| {
            let mut text = String::from(
                "<!-- a long
comment
of three lines -->
",
            );
            text.push_str(
                &"<p>x</p>
"
                .repeat(code),
            );
            text
        };
        let file = Oversized {
            rule: Rule::Size,
            path: "crates/a/ui/index.html".to_string(),
            lines: lines_of_code(Lang::Html, &page(501)),
        };
        assert_eq!(file.lines, 501);
        let grown = growth_since(&file, &page(500)).expect("it grew by one code line");
        assert!(
            grown.starts_with("crates/a/ui/index.html: 500 -> 501"),
            "{grown}"
        );
        assert_eq!(growth_since(&file, &page(501)), None, "the same size");
    }

    #[test]
    fn a_file_over_its_limit_may_shrink_but_not_grow() {
        let file = Oversized {
            rule: Rule::Size,
            path: "crates/a/src/big.rs".to_string(),
            lines: 900,
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
    fn a_line_of_rust_may_have_100_characters() {
        let text = format!(
            "{}\n{}\n{}\n",
            "a".repeat(100),
            "b".repeat(101),
            "é".repeat(100)
        );
        assert_eq!(
            Rule::Width.count(Lang::Rust, &text),
            Some(1),
            "101 characters; 100 accented letters are 200 bytes but 100 characters"
        );
        assert_eq!(
            Rule::Width.count(Lang::TypeScript, &text),
            None,
            "only Rust is measured"
        );
        assert_eq!(Rule::Width.count(Lang::Html, &text), None);
    }

    #[test]
    fn a_rust_file_with_a_wide_line_is_listed() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let root = dir.path();
        let src = root.join("crates/a/src");
        std::fs::create_dir_all(&src).expect("create the dir");
        let wide = format!("let a = \"{}\";\n", "x".repeat(100));
        std::fs::write(src.join("wide.rs"), &wide).expect("write wide.rs");
        std::fs::write(src.join("narrow.rs"), "let a = 1;\n").expect("write narrow.rs");
        let over: Vec<(Rule, String, usize)> = measure(root)
            .expect("measure")
            .into_iter()
            .map(|file| (file.rule, file.path, file.lines))
            .collect();
        assert_eq!(over, [(Rule::Width, "crates/a/src/wide.rs".to_string(), 1)]);
    }

    #[test]
    fn a_new_wide_line_fails_and_an_old_one_may_stay() {
        let line = format!("    // {}\n", "x".repeat(100));
        let file = Oversized {
            rule: Rule::Width,
            path: "crates/a/src/lib.rs".to_string(),
            lines: 2,
        };
        assert_eq!(
            growth_since(&file, &line.repeat(2)),
            None,
            "the same two lines"
        );
        let grown = growth_since(&file, &line).expect("one wide line more");
        assert!(
            grown.starts_with("crates/a/src/lib.rs: 1 -> 2 lines wider than 100 characters"),
            "{grown}"
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
