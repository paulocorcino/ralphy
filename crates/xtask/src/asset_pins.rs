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

/// Which asset a statement is about: the receiver it reads, if that receiver was
/// bound to one, else the test's last-named asset.
fn attribute(
    stmt: &str,
    bindings: &BTreeMap<String, String>,
    fallback: &Option<String>,
) -> Option<String> {
    for (var, named) in bindings {
        // `css.contains(` / `!css.contains(` / `css_rule_body(&css,` — the
        // receiver appears immediately before a `.` or as a borrowed argument.
        if stmt.contains(&format!("{var}.")) || stmt.contains(&format!("&{var},")) {
            return Some(named.clone());
        }
    }
    fallback.clone()
}

/// Is this literal a bare asset filename rather than a fragment of asset text?
fn is_asset_name(lit: &str) -> bool {
    [".html", ".js", ".css"]
        .iter()
        .any(|ext| lit.ends_with(ext))
        && !lit.contains(['(', '{', '<', '=', ';', ' '])
}

/// Does this statement read one of the normalized variables?
fn reads_normalized(stmt: &str, normalized: &std::collections::BTreeSet<String>) -> bool {
    normalized
        .iter()
        .any(|var| stmt.contains(&format!("{var}.")))
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
    let mut bindings: BTreeMap<String, String> = BTreeMap::new();
    // Variables bound from a normalizing expression. A pin read through one of
    // these survives a reformat, which is what shape C means — and the
    // normalization is virtually always a `let` one or more statements above the
    // loop that uses it, so the shape cannot be read off the loop body alone.
    let mut normalized: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut idx = 0;

    while idx < lines.len() {
        let trimmed = lines[idx].trim();

        // A new test resets the attribution: a pin never refers to an asset
        // named by a PREVIOUS test.
        if trimmed.starts_with("fn ") || trimmed.starts_with("async fn ") {
            asset = None;
            bindings.clear();
            normalized.clear();
        }
        // Three ways a test names the asset it is about: it embeds it, it asks
        // the router for it, or it assembles the stylesheet.
        //
        // `bindings` maps the LOCAL VARIABLE to the asset, because a single test
        // routinely reads several. Booking every pin to the last file seen was
        // wrong for 39 of this repo's tests — all sixteen pins of
        // `the_release_badge_and_panel_are_pinned_in_the_served_assets` went to
        // `wb-release.js`, which was the last of the four it reads.
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
                        if let Some(var) = binds(trimmed) {
                            bindings.insert(var, named.to_string());
                        }
                    }
                }
            }
        }
        if trimmed.contains("split_whitespace()") || trimmed.contains("strip_css_comments(") {
            if let Some(var) = binds(trimmed) {
                normalized.insert(var);
            }
        }
        // The stylesheet is twelve partials assembled by a helper, so it is
        // named by a CALL rather than by a path. Without this the tool that was
        // built to measure this branch's own split could not see the asset it
        // split: `styles.css` went from 129 claims to zero.
        if lines[idx].contains("served_css()") {
            asset = Some("styles/*.css".to_string());
            if let Some(var) = binds(trimmed) {
                bindings.insert(var, "styles/*.css".to_string());
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
            // Shape comes from the loop's OWN body and nothing else. An earlier
            // version looked back twelve lines for a `split_whitespace()` to
            // catch normalization applied to the haystack before the loop — and
            // it caught unrelated statements instead, booking 16 of the 19
            // "normalized" claims wrong. A window that reaches outside the
            // construct it is classifying cannot be made reliable; a narrower
            // wrong answer beats a wider one.
            let shape = if reads_normalized(&body, &normalized) {
                Shape::Normalized
            } else {
                classify(&body).unwrap_or(Shape::Identifier)
            };
            let about = attribute(&body, &bindings, &asset);
            // A table whose elements PAIR a document with its name — `for (doc,
            // name) in [(include_str!(…), "index.html"), …]` — carries labels,
            // not pins. Counting them booked four filenames as claims about
            // asset text.
            // A TUPLE loop variable — `for (doc, name) in [(shell, "index.html"),
            // …]` — means the table pairs a value with a label, and the bare
            // filenames in it are labels rather than pins.
            let labels = trimmed.starts_with("for (")
                || block.contains("include_str!")
                || block.contains("served_css()");
            // Where the pins are depends on which kind of table this is. An
            // ordinary `for pin in ["a(", "b("]` carries them in the TABLE. A
            // label table carries documents in the table and the claim in the
            // BODY — `doc.contains("function foo(")` — so reading the block
            // there would count filenames and lose the actual pin.
            let source = if labels { &body } else { &block };
            for literal in string_literals(source) {
                if is_asset_name(&literal) {
                    continue;
                }
                out.push(Pin {
                    file: file.to_string(),
                    line: idx + 1,
                    shape,
                    asset: about.clone(),
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
            if let Some(shape) = classify(&stmt).map(|shape| {
                if reads_normalized(&stmt, &normalized) {
                    Shape::Normalized
                } else {
                    shape
                }
            }) {
                out.push(Pin {
                    file: file.to_string(),
                    line: idx + 1,
                    shape,
                    asset: attribute(&stmt, &bindings, &asset),
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

/// The local variable a `let` binds, if the line is a binding.
///
/// `let css = served_css();` -> `css`. Used to book a pin to the receiver named
/// in its own statement rather than to whichever asset the test read last.
fn binds(line: &str) -> Option<String> {
    let rest = line.strip_prefix("let ")?;
    let rest = rest.strip_prefix("mut ").unwrap_or(rest);
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
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
        // A pin table can be long, but not unbounded. On an unbalanced parse
        // this returns NOTHING rather than the 200 lines it accumulated: handing
        // that buffer on would count every literal in 200 lines of unrelated
        // code as a claim, so the bail would inflate the number it exists to
        // protect. Undercounting on a parse we do not understand is the honest
        // direction.
        if offset > 200 {
            return (String::new(), start + 1);
        }
    }
    (buf, start + 1)
}

/// Every string literal in a fragment, in source order.
///
/// Rust has two spellings and they need different scanners. `"…"` ends at the
/// first unescaped quote; `r#"…"#` ends at the matching `"#` and treats an
/// embedded `"` as content. Scanning a raw string as a plain one cuts it at its
/// first inner quote — `r#"class="fence-item""#` was recorded as the pin
/// `class=` — and 69 raw-string elements sit in this file's pin tables, so the
/// text this tool prints was wrong for every one of them.
fn string_literals(fragment: &str) -> Vec<String> {
    let bytes: Vec<char> = fragment.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // A raw string opens with `r`, any number of `#`, then a quote, and
        // closes with a quote followed by the SAME number of `#`.
        if bytes[i] == 'r' {
            let mut hashes = 0;
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] == '#' {
                hashes += 1;
                j += 1;
            }
            if j < bytes.len() && bytes[j] == '"' {
                let close: String = std::iter::once('"')
                    .chain(std::iter::repeat_n('#', hashes))
                    .collect();
                let rest: String = bytes[j + 1..].iter().collect();
                if let Some(at) = rest.find(&close) {
                    keep(&mut out, rest[..at].to_string());
                    i = j + 1 + at + close.len();
                    continue;
                }
            }
        }
        if bytes[i] != '"' {
            i += 1;
            continue;
        }
        let mut lit = String::new();
        let mut escaped = false;
        i += 1;
        while i < bytes.len() {
            let c = bytes[i];
            i += 1;
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
        keep(&mut out, lit);
    }
    out
}

/// Record a literal if it is a PIN rather than an assertion message or a path.
///
/// The word-count heuristic this replaced dropped real pins — eight of them in
/// this repo, including `this.changesError = msg || "";` and
/// `replaying = connOpts.id != null` — because a fragment of JS has spaces in
/// it exactly like a sentence does. Length is the honest discriminator: a
/// failure message in this codebase is a wrapped sentence, a pin is a fragment.
/// It is still a heuristic, and the count is an estimate of the contract by
/// shape rather than an audit of individual strings.
fn keep(out: &mut Vec<String>, lit: String) {
    if lit.is_empty() || lit.len() >= 120 {
        return;
    }
    // An `include_str!` argument is how a test NAMES an asset, not a claim about
    // one. Counting it added nine phantom claims, three of them from a test this
    // very branch added.
    if lit.starts_with("../assets/ui/") {
        return;
    }
    // A bare format placeholder — `{name}`, `{path}` — is the message's, not a
    // claim about asset text.
    if lit.starts_with('{') && lit.ends_with('}') && !lit.contains(' ') {
        return;
    }
    // Prose: a message long enough to wrap, with no code punctuation in it.
    let wordy = lit.split_whitespace().count() >= 5;
    // Deliberately NOT '.' or '#': a failure message routinely names a file
    // ("index.html must link both favicon forms") or an issue, and those are
    // prose. These five are punctuation prose does not carry.
    let codey = lit.contains(['(', '{', '=', ';', '<', '[', ':']);
    if wordy && !codey {
        return;
    }
    out.push(lit);
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
    // A script-order pin compares two byte offsets into the same document. The
    // offsets are named `<thing>_tag`/`<thing>_at` by convention here. This once
    // booked six of eight such claims to the filename literals in a `for (doc,
    // name)` header near one; that is fixed where the labels are read, not here.
    if stmt.contains("assert!(") && (stmt.contains("_tag <") || stmt.contains("_at <")) {
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

    /// A raw string is not a plain string, and this file's pin tables hold 69 of
    /// them. Scanned as plain, `r#"class="fence-item""#` is cut at its first
    /// inner quote and recorded as the pin `class=`.
    #[test]
    fn a_raw_string_pin_is_read_to_its_real_end() {
        assert_eq!(
            string_literals(r##"["jumpFence(", r#"class="fence-item""#]"##),
            vec!["jumpFence(".to_string(), "class=\"fence-item\"".to_string()]
        );
        // Multiple hashes, and a `"#` that is not the terminator.
        assert_eq!(
            string_literals(r###"[r##"a"#b"##]"###),
            vec!["a\"#b".to_string()]
        );
    }

    /// The word count alone dropped real pins: a fragment of JS has spaces in it
    /// exactly like a sentence does. Code punctuation is what tells them apart.
    #[test]
    fn a_wordy_pin_survives_when_it_carries_code() {
        let kept = string_literals(
            r#"["this.changesError = msg || \"\";", "index.html must link both favicon forms"]"#,
        );
        assert!(kept.iter().any(|l| l.starts_with("this.changesError")));
        assert!(!kept.iter().any(|l| l.contains("must link both")));
    }

    /// An `include_str!` argument NAMES an asset; it is not a claim about one.
    #[test]
    fn an_asset_path_is_not_a_pin() {
        assert_eq!(
            string_literals(r#"[include_str!("../assets/ui/index.html"), "function foo("]"#),
            vec!["function foo(".to_string()]
        );
    }

    /// A test that reads two assets must not book both their pins to whichever
    /// it read last.
    #[test]
    fn a_pin_is_booked_to_the_receiver_its_own_statement_reads() {
        let src = r#"
    fn t() {
        let html = include_str!("../assets/ui/index.html");
        let app = include_str!("../assets/ui/app.js");
        assert!(html.contains("id=\"stage\""), "msg");
        assert!(app.contains("function shell("), "msg");
    }
"#;
        let mut out = Vec::new();
        collect("x.rs", src, &mut out);
        let booked: Vec<(&str, Option<&str>)> = out
            .iter()
            .map(|p| (p.text.as_str(), p.asset.as_deref()))
            .collect();
        assert!(booked
            .iter()
            .any(|(t, a)| t.contains("id=") && *a == Some("index.html")));
        assert!(booked
            .iter()
            .any(|(t, a)| t.contains("function shell(") && *a == Some("app.js")));
    }

    /// The stylesheet is named by a CALL, not a path — and after it became twelve
    /// partials this tool reported ZERO claims on it until it learned that.
    #[test]
    fn the_assembled_stylesheet_is_an_attributable_asset() {
        let src = r#"
    fn t() {
        let css = served_css();
        assert!(css.contains(".runs {"), "msg");
    }
"#;
        let mut out = Vec::new();
        collect("x.rs", src, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].asset.as_deref(), Some("styles/*.css"));
    }

    /// Normalization is a `let` above the loop, so the shape cannot be read off
    /// the loop body. Tracking the BINDING is what makes it precise — a 12-line
    /// lookback booked 16 of 19 such claims to unrelated statements.
    #[test]
    fn a_pin_read_through_a_normalized_binding_is_shape_c() {
        let src = r#"
    fn t() {
        let css = served_css();
        let squeezed = css.split_whitespace().collect::<Vec<_>>().join(" ");
        for decl in ["max-height: 30vh"] {
            assert!(squeezed.contains(decl), "msg");
        }
    }
"#;
        let mut out = Vec::new();
        collect("x.rs", src, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].shape, Shape::Normalized);
    }

    /// `for (doc, name) in [(shell, "index.html"), …]` pairs a value with a
    /// LABEL. The filenames are not claims about asset text.
    #[test]
    fn a_tuple_tables_filenames_are_labels_not_pins() {
        let src = r#"
    fn t() {
        let shell = include_str!("../assets/ui/index.html");
        for (doc, name) in [(shell, "index.html"), (other, "detached-fence.html")] {
            assert!(doc.contains("function foo("), "{name}");
        }
    }
"#;
        let mut out = Vec::new();
        collect("x.rs", src, &mut out);
        let texts: Vec<&str> = out.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(texts, vec!["function foo("]);
    }

    /// The bail must LOSE a block it cannot parse, never hand on the 200 lines it
    /// accumulated — that would count every literal in unrelated code as a claim.
    #[test]
    fn an_unbalanced_block_yields_nothing_rather_than_everything() {
        let lines: Vec<&str> = std::iter::once("for pin in [")
            .chain(std::iter::repeat_n("    \"noise(\",", 260))
            .collect();
        let (block, next) = balanced(&lines, 0, '[', ']');
        assert!(block.is_empty(), "an unbalanced table must yield no text");
        assert_eq!(next, 1, "and must not swallow the lines it read");
    }
}
