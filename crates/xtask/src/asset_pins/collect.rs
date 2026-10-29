//! Walks one source file and classifies each assertion that bears on an asset.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    attribute, balanced, binds, classify, is_asset_name, raw_literals, reads_normalized, squeeze,
    string_literals, Pin, Shape,
};

/// The two ways a test names an asset by a path-like literal: it embeds the
/// file, or it asks the router for it. A test file one directory deeper
/// embeds it with one more `../`, so `asset_after` skips any number of them.
const ASSET_MARKERS: [&str; 2] = ["include_str!(\"../", "get_local(\"/"];

/// What follows the `../` steps of an embedded asset's path.
const EMBEDDED_ASSET_DIR: &str = "assets/ui/";

/// The constants through which a test reads a page that the build assembled,
/// and the page each one holds. A claim made through one is a claim about that
/// page, as if the test embedded its source file.
const BUILT_PAGES: [(&str, &str); 3] = [
    ("INDEX_HTML", "index.html"),
    ("DETACHED_HTML", "detached.html"),
    ("DETACHED_FENCE_HTML", "detached-fence.html"),
];

/// The name of the stylesheet, which is assembled from partials and so has no
/// single path.
const ASSEMBLED_CSS: &str = "styles/*.css";

/// Walk one source file, classifying each assertion that bears on an asset.
///
/// The asset a pin is about is tracked by scanning forward from the last
/// `include_str!("../assets/ui/…")` in the same test — which is how these tests
/// are written without exception, and it is why the attribution is reliable
/// rather than heuristic.
pub(super) fn collect(file: &str, text: &str, out: &mut Vec<Pin>) {
    let mut scan = Scan {
        file,
        lines: text.lines().collect(),
        asset: None,
        bindings: BTreeMap::new(),
        normalized: BTreeSet::new(),
        out,
    };
    let mut idx = 0;
    while idx < scan.lines.len() {
        idx = scan.step(idx);
    }
}

/// The state of a walk over one source file.
struct Scan<'a> {
    file: &'a str,
    lines: Vec<&'a str>,
    asset: Option<String>,
    bindings: BTreeMap<String, String>,
    /// Variables bound from a normalizing expression. A pin read through one of
    /// these survives a reformat, which is what shape C means — and the
    /// normalization is virtually always a `let` one or more statements above the
    /// loop that uses it, so the shape cannot be read off the loop body alone.
    normalized: BTreeSet<String>,
    out: &'a mut Vec<Pin>,
}

impl Scan<'_> {
    /// Read the line at `idx`; return the index of the next line to read.
    fn step(&mut self, idx: usize) -> usize {
        let line = self.lines[idx];
        let trimmed = line.trim();
        self.reset_on_new_test(trimmed);
        self.note_named_assets(line, trimmed);
        self.note_normalized(trimmed);
        self.note_assembled_css(line, trimmed);
        // A comment describing a pin is not a pin.
        if trimmed.starts_with("//") {
            return idx + 1;
        }
        if let Some(next) = self.for_table(idx, trimmed) {
            return next;
        }
        if let Some(next) = self.const_table(idx, trimmed) {
            return next;
        }
        if let Some(next) = self.assertion(idx, trimmed) {
            return next;
        }
        self.scoped_slice(idx, trimmed);
        idx + 1
    }

    /// A new test resets the attribution: a pin never refers to an asset
    /// named by a PREVIOUS test.
    fn reset_on_new_test(&mut self, trimmed: &str) {
        if trimmed.starts_with("fn ") || trimmed.starts_with("async fn ") {
            self.asset = None;
            self.bindings.clear();
            self.normalized.clear();
        }
    }

    /// Three ways a test names the asset it is about: it embeds it, it asks
    /// the router for it, or it assembles the stylesheet (`note_assembled_css`).
    ///
    /// `bindings` maps the LOCAL VARIABLE to the asset, because a single test
    /// routinely reads several. Booking every pin to the last file seen was
    /// wrong for 39 of this repo's tests — all sixteen pins of
    /// `the_release_badge_and_panel_are_pinned_in_the_served_assets` went to
    /// `wb-release.js`, which was the last of the four it reads.
    fn note_named_assets(&mut self, line: &str, trimmed: &str) {
        for marker in ASSET_MARKERS {
            if let Some(named) = asset_after(line, marker) {
                self.book_asset(trimmed, named);
            }
        }
        if !trimmed.starts_with("pub(crate) const ") {
            if let Some(page) = built_page_on(line) {
                self.book_asset(trimmed, page.to_string());
            }
        }
    }

    fn note_normalized(&mut self, trimmed: &str) {
        if trimmed.contains("split_whitespace()") || trimmed.contains("strip_css_comments(") {
            if let Some(var) = binds(trimmed) {
                self.normalized.insert(var);
            }
        }
    }

    /// The stylesheet is twelve partials assembled by a helper, so it is
    /// named by a CALL rather than by a path. Without this the tool that was
    /// built to measure this branch's own split could not see the asset it
    /// split: `styles.css` went from 129 claims to zero.
    fn note_assembled_css(&mut self, line: &str, trimmed: &str) {
        if line.contains("served_css()") {
            self.book_asset(trimmed, ASSEMBLED_CSS.to_string());
        }
    }

    /// Make `named` the test's current asset, and the asset of the variable
    /// the line binds, if any.
    fn book_asset(&mut self, trimmed: &str, named: String) {
        if let Some(var) = binds(trimmed) {
            self.bindings.insert(var, named.clone());
        }
        self.asset = Some(named);
    }

    fn push(
        &mut self,
        idx: usize,
        shape: Shape,
        asset: Option<String>,
        needles: Vec<String>,
        text: String,
    ) {
        self.out.push(Pin {
            file: self.file.to_string(),
            line: idx + 1,
            shape,
            asset,
            needles,
            text,
        });
    }

    /// One claim per literal: its text and its only needle are the literal.
    fn push_literal(&mut self, idx: usize, shape: Shape, asset: &Option<String>, literal: String) {
        self.push(idx, shape, asset.clone(), vec![literal.clone()], literal);
    }

    /// The dominant idiom in these tests is a LIST of pinned strings driven
    /// through one assertion:
    ///
    /// ```text
    /// for pin in ["function foo(", "id=\"bar\""] {
    ///     assert!(js.contains(pin), "…");
    /// }
    /// ```
    ///
    /// Counting that as one assertion would undercount the contract by an
    /// order of magnitude — it is N independent claims about the asset's
    /// text, and a split breaks them one at a time. So the array's elements
    /// are the pins, and the loop body decides their shape.
    fn for_table(&mut self, idx: usize, trimmed: &str) -> Option<usize> {
        if !(trimmed.starts_with("for ") && trimmed.contains(" in [")) {
            return None;
        }
        let (block, next) = balanced(&self.lines, idx, '[', ']');
        let (body, after) = balanced(&self.lines, next.saturating_sub(1), '{', '}');
        let shape = self.loop_shape(&body);
        let about = attribute(&body, &self.bindings, &self.asset);
        // Where the pins are depends on which kind of table this is. An
        // ordinary `for pin in ["a(", "b("]` carries them in the TABLE. A
        // label table carries documents in the table and the claim in the
        // BODY — `doc.contains("function foo(")` — so reading the block
        // there would count filenames and lose the actual pin.
        let source = if is_label_table(trimmed, &block) {
            &body
        } else {
            &block
        };
        for literal in string_literals(source) {
            if !is_asset_name(&literal) {
                self.push_literal(idx, shape, &about, literal);
            }
        }
        Some(after.max(idx + 1))
    }

    /// Shape comes from the loop's OWN body and nothing else. An earlier
    /// version looked back twelve lines for a `split_whitespace()` to
    /// catch normalization applied to the haystack before the loop — and
    /// it caught unrelated statements instead, booking 16 of the 19
    /// "normalized" claims wrong. A window that reaches outside the
    /// construct it is classifying cannot be made reliable; a narrower
    /// wrong answer beats a wider one.
    fn loop_shape(&self, body: &str) -> Shape {
        if reads_normalized(body, &self.normalized) {
            Shape::Normalized
        } else {
            classify(body).unwrap_or(Shape::Identifier)
        }
    }

    /// A `const FOO: &[&str] = [...]` table of pins, same reasoning.
    fn const_table(&mut self, idx: usize, trimmed: &str) -> Option<usize> {
        if !(trimmed.starts_with("const ") && trimmed.contains("&[&str]")) {
            return None;
        }
        let (block, after) = balanced(&self.lines, idx, '[', ']');
        let asset = self.asset.clone();
        for literal in string_literals(&block) {
            self.push_literal(idx, Shape::Identifier, &asset, literal);
        }
        Some(after.max(idx + 1))
    }

    fn assertion(&mut self, idx: usize, trimmed: &str) -> Option<usize> {
        if !(trimmed.contains("assert!(") || trimmed.contains("assert_eq!(")) {
            return None;
        }
        let (stmt, after) = balanced(&self.lines, idx, '(', ')');
        if let Some(shape) = classify(&stmt) {
            let shape = if reads_normalized(&stmt, &self.normalized) {
                Shape::Normalized
            } else {
                shape
            };
            let asset = attribute(&stmt, &self.bindings, &self.asset);
            self.push(idx, shape, asset, raw_literals(&stmt), squeeze(&stmt));
        }
        Some(after.max(idx + 1))
    }

    /// A scoped slice is a `let` that carves a region out before asserting on
    /// it. It is a pin in its own right — it is the one that panics.
    fn scoped_slice(&mut self, idx: usize, trimmed: &str) {
        if trimmed.contains(".expect(")
            && (trimmed.contains(".find(") || trimmed.contains("split_once("))
        {
            let asset = self.asset.clone();
            self.push(
                idx,
                Shape::ScopedSlice,
                asset,
                raw_literals(trimmed),
                squeeze(trimmed),
            );
        }
    }
}

/// The asset a line names after `marker`, if it names one.
///
/// `get_local` also fetches ROUTES (`/api/session`), which are not assets and
/// carry no pins. An extension is what tells the two apart.
fn asset_after(line: &str, marker: &str) -> Option<String> {
    // The `include_str!` marker also matches an embed outside the UI
    // (`../vendor-build/…`), so every match on the line is tried.
    line.match_indices(marker).find_map(|(at, _)| {
        let mut rest = &line[at + marker.len()..];
        if marker.starts_with("include_str!") {
            rest = rest
                .trim_start_matches("../")
                .strip_prefix(EMBEDDED_ASSET_DIR)?;
        }
        let named = &rest[..rest.find('"')?];
        named.contains('.').then(|| named.to_string())
    })
}

/// The page a line reads through one of the `BUILT_PAGES` constants, if any.
fn built_page_on(line: &str) -> Option<&'static str> {
    BUILT_PAGES.iter().find_map(|&(constant, page)| {
        line.match_indices(constant)
            .any(|(at, _)| {
                let before = line[..at].chars().next_back();
                let after = line[at + constant.len()..].chars().next();
                let part_of_name = |c: char| c.is_alphanumeric() || c == '_';
                !before.is_some_and(part_of_name) && !after.is_some_and(part_of_name)
            })
            .then_some(page)
    })
}

/// A TUPLE loop variable — `for (doc, name) in [(shell, "index.html"), …]` —
/// means the table pairs a value with a label, and the bare filenames in it
/// are labels rather than pins. Counting them booked four filenames as claims
/// about asset text.
fn is_label_table(header: &str, block: &str) -> bool {
    header.starts_with("for (")
        || block.contains("include_str!")
        || block.contains("served_css()")
        || built_page_on(block).is_some()
}

#[cfg(test)]
mod tests;
