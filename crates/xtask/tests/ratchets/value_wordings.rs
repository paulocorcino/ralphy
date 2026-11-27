//! The run value wordings have one source, `assets/wording/run.json` (#684):
//! a surface does not write a status icon or fallback by hand. The counter
//! reads string literals, in the production code of the three surfaces, that
//! equal an icon or a fallback of the catalog.

use super::*;

/// `(file, literals)` counted at the integrated tip: 14 literals, none of them
/// a status. The 13 in `line.rs` are the icons and fallbacks of events: the
/// usage-limit sleep (3 sites), issue started, plan written, agent waiting
/// and deadline passed. They use the icon of a status for another fact, and
/// the event wordings take them later (#688). The one in `wb-runs.ts` is the
/// checked plan step glyph. The target is 0 statuses.
const COUNTED_SITES: &[(&str, usize)] = &[
    ("crates/ralphy-cli/src/ui/render/line.rs", 13),
    ("crates/ralphy-daemon/assets/ui/wb-runs.ts", 1),
];

/// A literal with an icon form (U+FE0F removed) or a fallback of `values`.
fn catalog_literals(kinds: &[&str]) -> BTreeSet<String> {
    let text = read(&workspace_root().join("assets/wording/run.json"));
    let json: serde_json::Value = serde_json::from_str(&text).expect("run.json is JSON");
    let mut set = BTreeSet::new();
    for kind in kinds {
        let entries = json.pointer(&format!("/values/{kind}"));
        let entries = entries.and_then(|v| v.as_object()).expect("a value set");
        for entry in entries.values() {
            for field in ["icon", "fallback"] {
                let text = entry.get(field).and_then(|v| v.as_str());
                set.insert(plain(text.expect("a text field")));
            }
        }
    }
    set
}

/// The literal without the emoji variation selector.
fn plain(text: &str) -> String {
    text.chars().filter(|c| *c != '\u{fe0f}').collect()
}

/// The production code of a Rust source: the text before an inline
/// `#[cfg(test)] mod x {`, without comment lines.
fn rust_production(src: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let mut code = String::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let next = lines.get(i + 1).map_or("", |l| l.trim_start());
        if trimmed.starts_with("#[cfg(test)]") && next.starts_with("mod ") && next.ends_with('{') {
            break;
        }
        if !trimmed.starts_with("//") {
            code.push_str(line);
            code.push('\n');
        }
    }
    code
}

/// Literals of a Rust source that are in `wanted`, `\u{..}` escapes decoded.
pub(super) fn rust_hits(src: &str, wanted: &BTreeSet<String>) -> Vec<String> {
    let literal = Regex::new(r#""((?:[^"\\]|\\.)*)""#).expect("a fixed pattern");
    let escape = Regex::new(r"\\u\{([0-9a-fA-F]+)\}").expect("a fixed pattern");
    let mut hits = Vec::new();
    let code = rust_production(src);
    // Line by line: a string that spans lines would unpair the quotes after it.
    for caps in code.lines().flat_map(|line| literal.captures_iter(line)) {
        let decoded = escape.replace_all(&caps[1], |c: &regex::Captures<'_>| {
            u32::from_str_radix(&c[1], 16)
                .ok()
                .and_then(char::from_u32)
                .map_or_default(String::from)
        });
        if wanted.contains(&plain(&decoded)) {
            hits.push(decoded.into_owned());
        }
    }
    hits
}

/// Literals of a workbench module that are in `wanted`; comments are not read.
pub(super) fn ts_hits(src: &str, wanted: &BTreeSet<String>) -> Vec<String> {
    lex::lex(src, 1)
        .into_iter()
        .filter_map(|t| match t.tok {
            lex::Tok::Str(s) if wanted.contains(&plain(&s)) => Some(s),
            lex::Tok::Str(_)
            | lex::Tok::Ident(_)
            | lex::Tok::Punct(_)
            | lex::Tok::Tpl(_)
            | lex::Tok::Other => None,
        })
        .collect()
}

/// A test file: `tests.rs`, `*_tests.rs`, or anything under a `tests` folder.
fn is_test_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name == "tests.rs"
        || name.ends_with("_tests.rs")
        || path.components().any(|c| c.as_os_str() == "tests")
}

/// `(rel path, hits)` for each file with a hit, over the three surfaces.
fn measure() -> Vec<(String, Vec<String>)> {
    let root = workspace_root();
    let status = catalog_literals(&["issue-status"]);
    let outcome = catalog_literals(&["run-outcome"]);
    let both: BTreeSet<String> = status.union(&outcome).cloned().collect();
    let mut files = Vec::new();
    skipped_tests::rust_sources(&root.join("crates/ralphy-cli/src/ui"), &mut files);
    skipped_tests::rust_sources(&root.join("crates/ralphy-cli/src/telegram"), &mut files);
    let mut found = Vec::new();
    for file in files.iter().filter(|f| !is_test_file(f)) {
        let in_ui = file.components().any(|c| c.as_os_str() == "ui");
        let hits = rust_hits(&read(file), if in_ui { &both } else { &status });
        found.push((rel(file), hits));
    }
    for file in ts_modules(&root.join(UI_DIR)) {
        found.push((rel(&file), ts_hits(&read(&file), &status)));
    }
    found.retain(|(_, hits)| !hits.is_empty());
    found
}

/// The path from the workspace root, with `/`.
fn rel(path: &Path) -> String {
    let root = workspace_root().canonicalize().expect("the root exists");
    let full = path.canonicalize().expect("the file exists");
    let tail = full.strip_prefix(root).expect("under the root");
    tail.to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

#[test]
fn value_wordings_have_one_source() {
    let found: Vec<(String, usize)> = measure()
        .into_iter()
        .map(|(file, hits)| (file, hits.len()))
        .collect();
    let baseline: Vec<(String, usize)> = COUNTED_SITES
        .iter()
        .map(|(file, n)| ((*file).to_owned(), *n))
        .collect();
    assert_eq!(
        found, baseline,
        "a status icon or fallback is written by hand in a surface: take it from the \
         catalog, and a count that goes down lowers COUNTED_SITES"
    );
}

#[test]
fn the_counter_sees_a_literal_icon() {
    let wanted = catalog_literals(&["issue-status"]);
    let src = "fn a() -> &'static str { \"\u{2705}\" }\n// \"\u{2705}\"\n\
               #[cfg(test)]\nmod tests {\n    let _ = \"\u{2705}\";\n}\n";
    assert_eq!(rust_hits(src, &wanted).len(), 1);
    let ts = "const a = \"[ok]\"; // \"[ok]\"\n";
    assert_eq!(ts_hits(ts, &wanted).len(), 1);
}
