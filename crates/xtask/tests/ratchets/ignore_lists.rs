//! The ignore-list ratchet: each list that hides files from a linter keeps its
//! size, so a new entry is a decision in review and a removed one lowers the
//! constant.

use super::*;

/// Entries of `ignorePatterns` in `.oxlintrc.json`.
const OXLINT_IGNORE_PATTERNS: usize = 1;
/// Entries of `ignore` in `.jscpd.json`.
const JSCPD_IGNORE: usize = 6;
/// Entries of `extend-exclude` in `_typos.toml`.
const TYPOS_EXTEND_EXCLUDE: usize = 7;
/// Entries of `extend-ignore-re` in `_typos.toml`.
const TYPOS_EXTEND_IGNORE_RE: usize = 10;
/// Words of `[default.extend-words]` in `_typos.toml`.
const TYPOS_EXTEND_WORDS: usize = 8;

/// The quoted strings in the first `key: [ ... ]` (or `key = [ ... ]`) array of
/// `text`. A comment line (`//` or `#`) is not read, and a `]` inside a string
/// does not close the array. A missing array has 0 entries.
fn quoted_entries(text: &str, key: &str) -> usize {
    let code: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("//") && !l.trim_start().starts_with('#'))
        .collect();
    let code = code.join("\n");
    let opening =
        Regex::new(&format!(r#""?{}"?\s*[:=]\s*\["#, regex::escape(key))).expect("a fixed pattern");
    let Some(array) = opening.find(&code) else {
        return 0;
    };
    let mut entries = 0;
    let mut quote = None;
    let mut chars = code.get(array.end()..).unwrap_or_default().chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (None, ']') => break,
            (None, '"' | '\'') => {
                quote = Some(c);
                entries += 1;
            }
            (Some('"'), '\\') => {
                chars.next();
            }
            (Some(q), _) if q == c => quote = None,
            _ => {}
        }
    }
    entries
}

/// The `name = value` lines of the `[section]` table of a TOML text.
fn table_entries(text: &str, section: &str) -> usize {
    let header = format!("[{section}]");
    text.lines()
        .skip_while(|l| l.trim() != header)
        .skip(1)
        .take_while(|l| !l.trim_start().starts_with('['))
        .filter(|l| l.contains('=') && !l.trim_start().starts_with('#'))
        .count()
}

#[test]
fn ignore_lists_do_not_grow() {
    let root = workspace_root();
    let typos = read(&root.join("_typos.toml"));
    let oxlint = read(&root.join(".oxlintrc.json"));
    let jscpd = read(&root.join(".jscpd.json"));
    let lists = [
        (
            ".oxlintrc.json ignorePatterns",
            quoted_entries(&oxlint, "ignorePatterns"),
            OXLINT_IGNORE_PATTERNS,
        ),
        (
            ".jscpd.json ignore",
            quoted_entries(&jscpd, "ignore"),
            JSCPD_IGNORE,
        ),
        (
            "_typos.toml extend-exclude",
            quoted_entries(&typos, "extend-exclude"),
            TYPOS_EXTEND_EXCLUDE,
        ),
        (
            "_typos.toml extend-ignore-re",
            quoted_entries(&typos, "extend-ignore-re"),
            TYPOS_EXTEND_IGNORE_RE,
        ),
        (
            "_typos.toml [default.extend-words]",
            table_entries(&typos, "default.extend-words"),
            TYPOS_EXTEND_WORDS,
        ),
    ];
    let errors: Vec<String> = lists
        .iter()
        .filter(|(_, found, baseline)| found != baseline)
        .map(|(list, found, baseline)| format!("{list}: {baseline} -> {found}"))
        .collect();
    assert!(
        errors.is_empty(),
        "an ignore list hides files from a linter: a new entry needs a reason in review, \
         and a removed one lowers its constant in ignore_lists.rs in the same change:\n{}",
        errors.join("\n")
    );
}

#[test]
fn the_list_scans_count_entries_and_not_comments() {
    let json = "{\n  // \"a\"\n  \"ignore\": [\"x\", \"y\"],\n  \"other\": [\"z\"]\n}";
    assert_eq!(quoted_entries(json, "ignore"), 2);
    let toml = "extend-exclude = [\n    # \"c\"\n    \"a\",\n    'b]',\n]\n[t]\nk = 1\n";
    assert_eq!(quoted_entries(toml, "extend-exclude"), 2);
    let table = "[a]\nx = 1\n# y = 2\nz = 3\n[b]\nw = 4\n";
    assert_eq!(table_entries(table, "a"), 2);
}
