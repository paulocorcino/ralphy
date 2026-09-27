//! Text a user reads never cites an ADR. An ADR number or a `docs/adr` path
//! means something only to a developer of Ralphy; to the person who runs it,
//! it is noise. Comments cite ADRs; string literals do not.
//!
//! The gate reads every string literal in production Rust: the text of a log
//! line, an error, a generated file, a comment posted to GitHub. `xtask` is
//! left out, because its readers are the developers. `--help` text comes from
//! doc comments, not literals; `ralphy-cli` has its own test for that.

use std::path::{Path, PathBuf};

use regex::Regex;

#[test]
fn no_production_string_literal_cites_an_adr() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    collect_rs(&crates, &mut files);
    assert!(
        files.len() > 100,
        "expected the workspace's sources under {}, found {} .rs files",
        crates.display(),
        files.len()
    );

    let cites = cites_an_adr();
    let mut found = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        for (line, literal) in string_literals(production(&text)) {
            if cites.is_match(&literal) {
                found.push(format!("{}:{line}: {}", path.display(), literal.trim()));
            }
        }
    }
    assert!(
        found.is_empty(),
        "string literals that cite an ADR — a user reads these; keep the ADR in a \
         comment instead:\n{}",
        found.join("\n")
    );
}

#[test]
fn a_literal_is_read_but_a_comment_a_char_and_a_lifetime_are_not() {
    let src = r####"
// "ADR-0001 in a comment"
/* "ADR-0002" /* nested "ADR-0003" */ */
fn f<'a>(x: &'a str) -> char {
    let _ = "one \" ADR-0004";
    let _ = r#"raw "ADR-0005""#;
    let _ = b"bytes";
    '"'
}
"####;
    let literals: Vec<String> = string_literals(src).into_iter().map(|(_, s)| s).collect();
    assert_eq!(
        literals,
        vec![
            r#"one \" ADR-0004"#.to_string(),
            r#"raw "ADR-0005""#.to_string(),
            "bytes".to_string()
        ]
    );
    assert_eq!(string_literals(src)[0].0, 5);
}

#[test]
fn the_pattern_matches_an_adr_number_and_a_path_but_not_a_word() {
    let cites = cites_an_adr();
    assert!(cites.is_match("refused (ADR-0052 §2)"));
    assert!(cites.is_match("see docs/adr/0003"));
    assert!(!cites.is_match("the ADRESS field"));
    assert!(!cites.is_match("a padre"));
}

fn cites_an_adr() -> Regex {
    Regex::new(r"ADR-\d|docs/adr").expect("the pattern is a valid regex literal")
}

/// The part of a file above its first inline test module, the same cut the
/// file-split rule uses (ADR-0022).
fn production(text: &str) -> &str {
    let mut offset = 0;
    let mut lines = text.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        if line.trim() == "#[cfg(test)]"
            && lines
                .peek()
                .is_some_and(|next| next.trim_start().starts_with("mod "))
        {
            return &text[..offset];
        }
        offset += line.len();
    }
    text
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("reading an entry of {}: {e}", dir.display()))
            .path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            let skipped = matches!(name, "target" | "node_modules" | "tests" | "xtask")
                || name.starts_with('.');
            if !skipped {
                collect_rs(&path, out);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_test_child.rs") {
            out.push(path);
        }
    }
}

/// `(1-based line, contents)` of each string literal: plain, byte and raw
/// strings. Comments (nested block comments too), char literals and lifetimes
/// are stepped over, so a quote inside them opens nothing.
fn string_literals(src: &str) -> Vec<(usize, String)> {
    let cs: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut line = 1;
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c == '\n' {
            line += 1;
            i += 1;
        } else if c == '/' && cs.get(i + 1) == Some(&'/') {
            while i < cs.len() && cs[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && cs.get(i + 1) == Some(&'*') {
            let mut depth = 0;
            while i < cs.len() {
                if cs[i] == '/' && cs.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if cs[i] == '*' && cs.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    if cs[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
            }
        } else if c == '\'' {
            // A char literal is `'x'` or `'\…'`; anything else is a lifetime.
            if cs.get(i + 1) == Some(&'\\') {
                i += 2;
                while i < cs.len() && cs[i] != '\'' {
                    i += 1;
                }
                i += 1;
            } else if cs.get(i + 2) == Some(&'\'') {
                i += 3;
            } else {
                i += 1;
            }
        } else if let Some((hashes, body)) = raw_start(&cs, i) {
            let start_line = line;
            let close: Vec<char> = std::iter::once('"')
                .chain(std::iter::repeat_n('#', hashes))
                .collect();
            let mut j = body;
            while j < cs.len() && !cs[j..].starts_with(&close) {
                if cs[j] == '\n' {
                    line += 1;
                }
                j += 1;
            }
            out.push((start_line, cs[body..j].iter().collect()));
            i = j + close.len();
        } else if c == '"' || (c == 'b' && cs.get(i + 1) == Some(&'"') && !ident_before(&cs, i)) {
            let start_line = line;
            let body = if c == 'b' { i + 2 } else { i + 1 };
            let mut j = body;
            while j < cs.len() && cs[j] != '"' {
                if cs[j] == '\\' {
                    j += 1;
                }
                if cs.get(j) == Some(&'\n') {
                    line += 1;
                }
                j += 1;
            }
            out.push((start_line, cs[body..j.min(cs.len())].iter().collect()));
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// `r"…"`, `r#"…"#`, `br"…"`: the number of `#` and where the body starts.
fn raw_start(cs: &[char], i: usize) -> Option<(usize, usize)> {
    if ident_before(cs, i) {
        return None;
    }
    let mut j = i;
    if cs.get(j) == Some(&'b') {
        j += 1;
    }
    if cs.get(j) != Some(&'r') {
        return None;
    }
    j += 1;
    let hashes = cs[j..].iter().take_while(|c| **c == '#').count();
    j += hashes;
    (cs.get(j) == Some(&'"')).then_some((hashes, j + 1))
}

fn ident_before(cs: &[char], i: usize) -> bool {
    i > 0 && (cs[i - 1].is_alphanumeric() || cs[i - 1] == '_')
}
