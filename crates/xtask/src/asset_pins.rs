//! `asset-pins` — an inventory of every assertion the Rust tests make about the
//! workbench's served assets.
//!
//! ## Why this is a command and not a document
//!
//! `crates/ralphy-daemon/src/lib.rs` holds a large, undocumented contract:
//! `include_str!` a `.js`/`.css`/`.html` file and assert a substring of it.
//! Nobody decided to build that contract — it accreted, one pin per issue, and
//! it is now the only CI-visible gate over most of the workbench. Any plan to
//! split those assets has to say what happens to each pin, and a plan cannot
//! say that against a number somebody counted by hand in a review that is
//! already stale (110 when #367 was written, more every week).
//!
//! So the inventory is executable. It is written in Rust rather than as a shell
//! pipeline for one reason that matters here: CI runs Windows and Linux, and a
//! `grep -P` counting script does not (`grep -P` is not available on this
//! project's own Windows shell).
//!
//! ## What it does NOT do
//!
//! It does not judge. It classifies by SHAPE, because shape is what decides a
//! pin's fate under a file split — an identifier pin survives a move untouched,
//! a scoped slice `.expect()`s and panics, a script-order pin is meaningless
//! once the tags change. Which pins are worth keeping is ADR-0057's call and a
//! human's; this tells you what is there.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The six shapes an asset assertion takes in this tree.
///
/// Ordered by what a split does to them, which is the only ordering that helps:
/// the ones at the top survive a move, the ones at the bottom break.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Shape {
    /// `src.contains("function foo(")` — an identifier that exists somewhere in
    /// one file. Survives a split only if it lands in the file the test names.
    Identifier,
    /// `!src.contains(…)` — a NEGATIVE. The one shape that gets STRONGER when
    /// restated over the whole tree instead of one named file, because then a
    /// split cannot move the offending text out from under it.
    Negative,
    /// `find(…)`/`split_once(…)` followed by `.expect()` — a slice of the file
    /// scoped to a region. Under a split this does not fail, it PANICS, and the
    /// message is about a missing delimiter rather than about the invariant.
    ScopedSlice,
    /// `matches(…).count()` / uniqueness. A whole-file count silently degrades
    /// to a per-fragment count the moment the file is one of several.
    Count,
    /// `split_whitespace()` / comment-stripping before the compare — a pin that
    /// already survives reformatting. The shape to imitate.
    Normalized,
    /// Two `find()`s on `index.html` compared with `<` — asserts one script tag
    /// precedes another. Made redundant by the tree/tag cross-check gate.
    ScriptOrder,
}

impl Shape {
    fn label(self) -> &'static str {
        match self {
            Shape::Identifier => "A identifier",
            Shape::Negative => "F negative",
            Shape::ScopedSlice => "B scoped-slice",
            Shape::Count => "D count",
            Shape::Normalized => "C normalized",
            Shape::ScriptOrder => "E script-order",
        }
    }

    fn under_a_split(self) -> &'static str {
        match self {
            Shape::Identifier => "survives if the text lands in the named file",
            Shape::Negative => "restate over the tree — gets stronger",
            Shape::ScopedSlice => "PANICS on a missing delimiter, not a failure",
            Shape::Count => "whole-file count degrades to per-fragment",
            Shape::Normalized => "survives reformatting; the shape to imitate",
            Shape::ScriptOrder => "redundant — the tag cross-check covers it",
        }
    }
}

/// One assertion, located.
struct Pin {
    file: String,
    line: usize,
    shape: Shape,
    asset: Option<String>,
    text: String,
}

pub fn asset_pins_cmd(args: &[String]) -> Result<()> {
    let mut root: Option<PathBuf> = None;
    let mut verbose = false;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--root" => {
                root = Some(PathBuf::from(it.next().context("--root needs a path")?));
            }
            "--verbose" => verbose = true,
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }
    let root = root.unwrap_or_else(repo_root);

    let mut pins = Vec::new();
    for rel in [
        "crates/ralphy-daemon/src/lib.rs",
        "crates/ralphy-daemon/src/dispatch.rs",
        "crates/ralphy-daemon/src/spend.rs",
        "crates/ralphy-daemon/src/usage.rs",
    ] {
        let path = root.join(rel);
        if !path.exists() {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        collect(rel, &text, &mut pins);
    }

    report(&pins, verbose);
    Ok(())
}

/// Walk one source file, classifying each assertion that bears on an asset.
///
/// The asset a pin is about is tracked by scanning forward from the last
/// `include_str!("../assets/ui/…")` in the same test — which is how these tests
/// are written without exception, and it is why the attribution is reliable
/// rather than heuristic.
fn collect(file: &str, text: &str, out: &mut Vec<Pin>) {
    let lines: Vec<&str> = text.lines().collect();
    let mut asset: Option<String> = None;
    let mut idx = 0;

    while idx < lines.len() {
        let trimmed = lines[idx].trim();

        // A new test resets the attribution: a pin never refers to an asset
        // named by a PREVIOUS test.
        if trimmed.starts_with("fn ") || trimmed.starts_with("async fn ") {
            asset = None;
        }
        // Two ways a test names the asset it is about: it embeds it, or it asks
        // the router for it. Both are attribution.
        for (marker, offset) in [
            (
                "include_str!(\"../assets/ui/",
                "include_str!(\"../assets/ui/".len(),
            ),
            ("get_local(\"/", "get_local(\"/".len()),
        ] {
            if let Some(at) = lines[idx].find(marker) {
                let rest = &lines[idx][at + offset..];
                if let Some(end) = rest.find('"') {
                    let named = &rest[..end];
                    // `get_local` also fetches ROUTES (`/api/session`), which are
                    // not assets and carry no pins. An extension is what tells
                    // the two apart.
                    if named.contains('.') {
                        asset = Some(named.to_string());
                    }
                }
            }
        }
        // A comment describing a pin is not a pin.
        if trimmed.starts_with("//") {
            idx += 1;
            continue;
        }

        // The dominant idiom in these tests is a LIST of pinned strings driven
        // through one assertion:
        //
        //     for pin in ["function foo(", "id=\"bar\""] {
        //         assert!(js.contains(pin), "…");
        //     }
        //
        // Counting that as one assertion would undercount the contract by an
        // order of magnitude — it is N independent claims about the asset's
        // text, and a split breaks them one at a time. So the array's elements
        // are the pins, and the loop body decides their shape.
        if trimmed.starts_with("for ") && trimmed.contains(" in [") {
            let (block, next) = balanced(&lines, idx, '[', ']');
            let (body, after) = balanced(&lines, next.saturating_sub(1), '{', '}');
            // The loop body carries the assertion, but the NORMALIZATION that
            // decides whether the pin survives a reformat is usually applied to
            // the haystack before the loop starts. Look back for it.
            let window = lines[idx.saturating_sub(12)..idx].join(" ");
            let shape = if window.contains("split_whitespace()")
                || window.contains("strip_css_comments(")
            {
                Shape::Normalized
            } else {
                classify(&body).unwrap_or(Shape::Identifier)
            };
            for literal in string_literals(&block) {
                out.push(Pin {
                    file: file.to_string(),
                    line: idx + 1,
                    shape,
                    asset: asset.clone(),
                    text: literal,
                });
            }
            idx = after.max(idx + 1);
            continue;
        }

        // A `const FOO: &[&str] = [...]` table of pins, same reasoning.
        if trimmed.starts_with("const ") && trimmed.contains("&[&str]") {
            let (block, after) = balanced(&lines, idx, '[', ']');
            for literal in string_literals(&block) {
                out.push(Pin {
                    file: file.to_string(),
                    line: idx + 1,
                    shape: Shape::Identifier,
                    asset: asset.clone(),
                    text: literal,
                });
            }
            idx = after.max(idx + 1);
            continue;
        }

        if trimmed.contains("assert!(") || trimmed.contains("assert_eq!(") {
            let (stmt, after) = balanced(&lines, idx, '(', ')');
            if let Some(shape) = classify(&stmt) {
                out.push(Pin {
                    file: file.to_string(),
                    line: idx + 1,
                    shape,
                    asset: asset.clone(),
                    text: squeeze(&stmt),
                });
            }
            idx = after.max(idx + 1);
            continue;
        }

        // A scoped slice is a `let` that carves a region out before asserting on
        // it. It is a pin in its own right — it is the one that panics.
        if trimmed.contains(".expect(")
            && (trimmed.contains(".find(") || trimmed.contains("split_once("))
        {
            out.push(Pin {
                file: file.to_string(),
                line: idx + 1,
                shape: Shape::ScopedSlice,
                asset: asset.clone(),
                text: squeeze(trimmed),
            });
        }
        idx += 1;
    }
}

/// Accumulate from `start` until the delimiter opened on that line is balanced.
/// Returns the joined text and the index one past the closing line.
fn balanced(lines: &[&str], start: usize, open: char, close: char) -> (String, usize) {
    let mut depth = 0i32;
    let mut seen = false;
    let mut buf = String::new();
    for (offset, line) in lines[start..].iter().enumerate() {
        buf.push_str(line);
        buf.push(' ');
        let mut in_str = false;
        let mut escaped = false;
        for ch in line.chars() {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' if in_str => escaped = true,
                '"' => in_str = !in_str,
                c if c == open && !in_str => {
                    depth += 1;
                    seen = true;
                }
                c if c == close && !in_str => depth -= 1,
                _ => {}
            }
        }
        if seen && depth <= 0 {
            return (buf, start + offset + 1);
        }
        // A pin table can be long, but not unbounded: bail rather than swallow
        // the rest of the file on an unbalanced parse.
        if offset > 200 {
            break;
        }
    }
    (buf, start + 1)
}

/// Every double-quoted literal in a fragment, unescaped enough to read.
fn string_literals(fragment: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = fragment.chars().peekable();
    while let Some(ch) = chars.next() {
        // `r#"…"#` and `"…"` both start their payload at a quote.
        if ch != '"' {
            continue;
        }
        let mut lit = String::new();
        let mut escaped = false;
        for c in chars.by_ref() {
            if escaped {
                lit.push(c);
                escaped = false;
                continue;
            }
            match c {
                '\\' => escaped = true,
                '"' => break,
                _ => lit.push(c),
            }
        }
        // Assertion MESSAGES are not pins, and they sit in the same arrays and
        // blocks. Prose is what distinguishes them: a pin is a fragment of
        // source text — `function projectBadge(`, `max-height: 30vh`,
        // `data-act="stage"` — and even the longest runs three or four words. A
        // failure message is a sentence. Five words is the line, and it is a
        // heuristic: the count this tool reports is an estimate of the contract
        // by shape, not an audit of individual strings.
        let words = lit.split_whitespace().count();
        if !lit.is_empty() && lit.len() < 120 && words < 5 {
            out.push(lit);
        }
    }
    out
}

fn squeeze(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(96)
        .collect()
}

/// Shape from a whole statement.
///
/// Order matters: the tests below are not mutually exclusive and the FIRST
/// match wins, so the more specific shapes are asked about first. A normalized
/// compare that also happens to count is normalized — the normalization is what
/// decides whether it survives a reformat, which is the question being asked.
fn classify(stmt: &str) -> Option<Shape> {
    if stmt.contains("split_whitespace()") || stmt.contains("strip_css_comments(") {
        return Some(Shape::Normalized);
    }
    if stmt.contains(".matches(") && stmt.contains(".count()") {
        return Some(Shape::Count);
    }
    if stmt.contains(".expect(") && (stmt.contains(".find(") || stmt.contains("split_once(")) {
        return Some(Shape::ScopedSlice);
    }
    // A script-order pin compares two byte offsets into the same document.
    if (stmt.contains("_tag <") || stmt.contains("_at <")) && stmt.contains("assert!(") {
        return Some(Shape::ScriptOrder);
    }
    if !stmt.contains(".contains(") {
        return None;
    }
    if !stmt.contains("assert!(") && !stmt.contains("assert_eq!(") {
        return None;
    }
    // The NEGATIVE form: the receiver is negated. Written as a scan for `!`
    // immediately before an identifier that leads into `.contains(`, because
    // `assert!(` itself contains a `!` and a naive search finds that first.
    let negated = stmt
        .match_indices(".contains(")
        .any(|(at, _)| receiver_is_negated(&stmt[..at]));
    Some(if negated {
        Shape::Negative
    } else {
        Shape::Identifier
    })
}

/// Walk back over the receiver expression to see whether it is negated.
///
/// `!lc.contains(…)` and `!html.contains(…)` are negatives; `assert!(x.contains(…))`
/// is not, even though the statement holds a `!`.
fn receiver_is_negated(before: &str) -> bool {
    let mut chars = before.chars().rev().peekable();
    let mut saw_receiver = false;
    // The receiver can be a chain — `body.to_lowercase()`, `src[0..10]` — so the
    // walk back has to step OVER a balanced group rather than stopping at its
    // opening bracket. Anything inside `(…)` or `[…]` belongs to the receiver.
    let mut depth = 0i32;
    while let Some(&c) = chars.peek() {
        match c {
            ')' | ']' => {
                depth += 1;
                saw_receiver = true;
            }
            '(' | '[' if depth > 0 => depth -= 1,
            _ if depth > 0 => {}
            c if c.is_alphanumeric() || c == '_' || c == '.' => saw_receiver = true,
            _ => break,
        }
        chars.next();
    }
    saw_receiver && chars.peek() == Some(&'!')
}

fn report(pins: &[Pin], verbose: bool) {
    let mut by_shape: BTreeMap<Shape, Vec<&Pin>> = BTreeMap::new();
    for pin in pins {
        by_shape.entry(pin.shape).or_default().push(pin);
    }

    // Two numbers, because they answer different questions. A CLAIM is one
    // pinned fragment of asset text: that is the size of the contract, and it is
    // what a split has to account for one by one. An ASSERTION is a statement in
    // the source: that is what reds, and what a reviewer reads. The dominant
    // idiom here drives many claims through one assertion, so the two differ by
    // roughly a factor of four and neither one alone is the honest figure.
    let sites: std::collections::BTreeSet<(&str, usize)> =
        pins.iter().map(|p| (p.file.as_str(), p.line)).collect();
    println!(
        "asset pins — {} claims over the served UI, made by {} assertions\n",
        pins.len(),
        sites.len()
    );
    println!("shape             count  what a file split does to it");
    println!("{}", "-".repeat(78).as_str());
    for (shape, group) in &by_shape {
        println!(
            "{:<16} {:>6}  {}",
            shape.label(),
            group.len(),
            shape.under_a_split()
        );
    }

    println!("\nby asset:");
    let mut by_asset: BTreeMap<&str, usize> = BTreeMap::new();
    for pin in pins {
        *by_asset
            .entry(pin.asset.as_deref().unwrap_or("(unattributed)"))
            .or_default() += 1;
    }
    for (asset, n) in &by_asset {
        println!("  {n:>4}  {asset}");
    }

    if verbose {
        println!("\nevery pin:");
        for (shape, group) in &by_shape {
            println!("\n  {} ({}):", shape.label(), group.len());
            for pin in group {
                println!("    {}:{}  {}", pin.file, pin.line, pin.text);
            }
        }
    }
}

fn repo_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `crates/xtask`; the root is two up. Not `git
    // rev-parse`: this must run in a source tarball with no `.git`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The classifier's one genuinely subtle judgement: `assert!(` contains a
    /// `!`, so "is this a negative pin?" cannot be answered by searching for one.
    #[test]
    fn a_negated_receiver_is_not_confused_with_the_assert_macro() {
        assert_eq!(
            classify(r#"assert!(js.contains("function foo("), "msg");"#),
            Some(Shape::Identifier)
        );
        assert_eq!(
            classify(r#"assert!(!lc.contains("mock"), "msg");"#),
            Some(Shape::Negative)
        );
        // The receiver can be an expression, not just a name.
        assert_eq!(
            classify(r#"assert!(!src[0..10].contains("x"));"#),
            Some(Shape::Negative)
        );
        assert_eq!(
            classify(r#"assert!(!body.to_lowercase().contains("x"));"#),
            Some(Shape::Negative)
        );
    }

    #[test]
    fn the_more_specific_shapes_win_over_a_bare_contains() {
        assert_eq!(
            classify(r#"assert_eq!(html.matches("<script").count(), 3);"#),
            Some(Shape::Count)
        );
        assert_eq!(
            classify(r#"assert!(css.split_whitespace().eq(other.contains("x")));"#),
            Some(Shape::Normalized)
        );
        // Not an assertion about asset text at all.
        assert_eq!(classify("let x = 1;"), None);
        assert_eq!(classify(r#"let s = html.contains("x");"#), None);
    }

    /// A pin is a fragment of source text; a failure message is a sentence.
    #[test]
    fn assertion_prose_is_not_counted_as_a_pin() {
        let block =
            r#"["function foo(", "max-height: 30vh", "index.html must link both favicon forms"]"#;
        assert_eq!(
            string_literals(block),
            vec!["function foo(".to_string(), "max-height: 30vh".to_string()]
        );
    }

    #[test]
    fn a_pin_table_is_read_to_its_closing_bracket() {
        let lines = [
            r#"        for pin in ["#,
            r#"            "a(","#,
            r#"            "b(","#,
            r#"        ] {"#,
            r#"            assert!(js.contains(pin), "must keep it");"#,
            r#"        }"#,
        ];
        let mut out = Vec::new();
        collect("x.rs", &lines.join("\n"), &mut out);
        assert_eq!(out.len(), 2, "both pins in the table are claims");
        assert!(out.iter().all(|p| p.shape == Shape::Identifier));
    }
}
