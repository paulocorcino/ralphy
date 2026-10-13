//! `asset-pins` — an inventory of every assertion the Rust tests make about the
//! workbench's served assets.
//!
//! ## Why this is a command and not a document
//!
//! `crates/ralphy-daemon/src/tests.rs` holds a large, undocumented contract:
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

mod collect;
use collect::collect;

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
pub(crate) struct Pin {
    pub(crate) file: String,
    pub(crate) line: usize,
    shape: Shape,
    pub(crate) asset: Option<String>,
    text: String,
    /// Every string literal of the claim, unfiltered and not squeezed. `text`
    /// is cut at 96 chars and `keep` drops prose, so neither can answer "does
    /// a test pin this exact piece of UI text" (`ui-copy`).
    pub(crate) needles: Vec<String>,
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
    let pins = gather(&root)?;
    report(&pins, verbose);
    Ok(())
}

/// Every claim the daemon's tests make over the served assets under `root`.
pub(crate) fn gather(root: &Path) -> Result<Vec<Pin>> {
    let mut pins = Vec::new();
    for rel in [
        "crates/ralphy-daemon/src/tests.rs",
        "crates/ralphy-daemon/src/dispatch.rs",
        "crates/ralphy-daemon/src/spend/tests.rs",
        "crates/ralphy-daemon/src/usage/tests.rs",
    ] {
        let path = root.join(rel);
        if !path.exists() {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        collect(rel, &text, &mut pins);
    }
    Ok(pins)
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

/// Every string literal in a fragment that `keep` accepts as a pin, in source
/// order.
fn string_literals(fragment: &str) -> Vec<String> {
    let mut out = Vec::new();
    for lit in raw_literals(fragment) {
        keep(&mut out, lit);
    }
    out
}

/// Every non-empty string literal of a Rust fragment, with no pin filtering.
///
/// Rust has two spellings and they need different scanners. `"…"` ends at the
/// first unescaped quote; `r#"…"#` ends at the matching `"#` and treats an
/// embedded `"` as content. Scanning a raw string as a plain one cuts it at its
/// first inner quote — `r#"class="fence-item""#` was recorded as the pin
/// `class=` — and 69 raw-string elements sit in this file's pin tables, so the
/// text this tool prints was wrong for every one of them.
fn raw_literals(fragment: &str) -> Vec<String> {
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
                    if at > 0 {
                        out.push(rest[..at].to_string());
                    }
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
        if !lit.is_empty() {
            out.push(lit);
        }
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

pub(crate) fn repo_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `crates/xtask`; the root is two up. Not `git
    // rev-parse`: this must run in a source tarball with no `.git`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests;
