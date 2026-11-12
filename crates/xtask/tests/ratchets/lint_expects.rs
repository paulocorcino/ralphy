//! The `#[expect]` ratchet of the lint gate (#889): each lint that a phase
//! turned into an error keeps an exact count of the `#[expect(clippy::<lint>`
//! exceptions under `crates/`, so a new exception is a reviewed change of a
//! constant here. Phases 4 and 7 added their lints to the table.

use super::*;

/// `(lint, exact count)`. Lower a count in the same change that removes an
/// exception: the count is exact, so it cannot grow back.
const EXPECTS: &[(&str, usize)] = &[
    ("as_conversions", 14),
    ("cast_possible_truncation", 5),
    ("cast_possible_wrap", 0),
    ("cast_precision_loss", 7),
    ("cast_sign_loss", 2),
    ("exit", 3),
    ("panic", 0),
    ("print_stderr", 8),
    ("print_stdout", 8),
    ("string_slice", 0),
    ("too_many_arguments", 43),
    ("undocumented_unsafe_blocks", 0),
];

/// The lints each `#[expect(` attribute of `text` names, the attribute read
/// up to its `reason` or its closing `)]`, so the lints of a list written over
/// several lines are counted. A comment is not an attribute.
fn expected_lints(text: &str) -> Vec<String> {
    let lint = Regex::new(r"clippy::([a-z_]+)").expect("a fixed pattern");
    let mut found = Vec::new();
    let mut open: Option<String> = None;
    for line in text.lines().map(str::trim_start) {
        if open.is_none() && (line.starts_with("#[expect(") || line.starts_with("#![expect(")) {
            open = Some(String::new());
        }
        let Some(attr) = open.as_mut() else { continue };
        attr.push_str(line);
        attr.push('\n');
        if line.contains("reason") || line.contains(")]") {
            let head = attr.split("reason").next().unwrap_or("");
            found.extend(lint.captures_iter(head).map(|c| c[1].to_owned()));
            open = None;
        }
    }
    found
}

#[test]
fn lint_expects_do_not_grow() {
    let root = workspace_root();
    let mut files = Vec::new();
    skipped_tests::rust_sources(&root.join("crates"), &mut files);
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for path in &files {
        for lint in expected_lints(&read(path)) {
            *counts.entry(lint).or_default() += 1;
        }
    }
    let errors: Vec<String> = EXPECTS
        .iter()
        .map(|(lint, expected)| (lint, expected, counts.get(*lint).copied().unwrap_or(0)))
        .filter(|(_, expected, found)| *expected != found)
        .map(|(lint, expected, found)| {
            format!("clippy::{lint}: {found} `#[expect]` exceptions, the baseline is {expected}")
        })
        .collect();
    assert!(
        errors.is_empty(),
        "fix the site instead of adding an exception; a removed exception lowers its \
         count in EXPECTS in the same change:\n{}",
        errors.join("\n")
    );
}

#[test]
fn the_scan_reads_a_list_over_several_lines() {
    // Built from pieces, so no line of this file starts with the attribute.
    let attr = "#[expect(";
    let text = format!(
        "// {attr}clippy::panic)]\n{attr}\n    clippy::as_conversions,\n    \
         clippy::cast_sign_loss,\n    reason = \"clippy::panic\"\n)]\n\
         {attr}clippy::panic, reason = \"x\")]\n"
    );
    assert_eq!(
        expected_lints(&text),
        ["as_conversions", "cast_sign_loss", "panic"]
    );
}
