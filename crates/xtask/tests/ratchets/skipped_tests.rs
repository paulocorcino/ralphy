//! The skipped-tests ratchet (#808): a Rust test that is ignored says why, the
//! number of ignored tests only goes down, and no workbench test is narrowed
//! with `.only(` or left out with `.skip(` or `.todo(`, as a call or as an
//! option of the test.

use super::*;

/// The `#[ignore` attributes in the Rust sources under `crates/`, doc comments
/// and `//` comments that name the attribute not counted. Lower it in the same
/// change that removes an ignored test: the count is exact, so it cannot grow
/// back. 12 at 11f28d4e.
const IGNORED_TESTS: usize = 12;

/// The `.rs` files under `dir`, `target/`, `node_modules/` and hidden folders
/// excepted. Unlike `collect_rs`, it reads the test files too.
fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("reading an entry of {}: {e}", dir.display()))
            .path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            if !matches!(name, "target" | "node_modules") && !name.starts_with('.') {
                rust_sources(&path, out);
            }
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

/// `(line, has a reason)` of each `#[ignore` attribute in `text`. An attribute
/// is a line that starts with it, so a comment is not one.
fn ignore_attributes(text: &str) -> Vec<(usize, bool)> {
    let with_reason = Regex::new(r#"^#\[ignore\s*=\s*"[^"]+"\s*\]"#).expect("a fixed pattern");
    text.lines()
        .enumerate()
        .map(|(i, line)| (i + 1, line.trim_start()))
        .filter(|(_, line)| line.starts_with("#[ignore"))
        .map(|(n, line)| (n, with_reason.is_match(line)))
        .collect()
}

/// `(rel path, line, has a reason)` of every ignored test under `crates/`.
fn ignored_tests() -> Vec<(String, usize, bool)> {
    let root = workspace_root();
    let mut files = Vec::new();
    rust_sources(&root.join("crates"), &mut files);
    assert!(
        files.len() > 100,
        "expected the workspace's sources under {}, found {} .rs files",
        root.display(),
        files.len()
    );
    let mut sites = Vec::new();
    for path in &files {
        for (line, reason) in ignore_attributes(&read(path)) {
            sites.push((rust_coupling::rel_path(&root, path), line, reason));
        }
    }
    sites
}

#[test]
fn every_ignored_test_says_why() {
    let bare: Vec<String> = ignored_tests()
        .into_iter()
        .filter(|(_, _, reason)| !reason)
        .map(|(file, line, _)| format!("{file}:{line}: `#[ignore]` has no reason"))
        .collect();
    assert!(
        bare.is_empty(),
        "write `#[ignore = \"<why>\"]` so the next reader knows when the test runs:\n{}",
        bare.join("\n")
    );
}

#[test]
fn ignored_tests_do_not_grow() {
    let found = ignored_tests().len();
    assert_eq!(
        found, IGNORED_TESTS,
        "{found} ignored tests, the baseline is {IGNORED_TESTS}: an ignored test is a check \
         the gate does not run, so run the new one in the gate; a removed one lowers \
         IGNORED_TESTS in the same change"
    );
}

/// How many `.only(`, `.skip(` and `.todo(` calls `src` has, and how many
/// `test`, `it`, `describe` or `suite` calls have an options object that sets
/// `only`, `skip` or `todo` to a value other than `false`. The lexer drops
/// comments and strings, so a sentence that names one is not counted.
fn narrowed_tests(src: &str) -> usize {
    let toks = lex::lex(src, 1);
    let methods = (0..toks.len().saturating_sub(2))
        .filter(|&i| {
            toks[i].is(".")
                && matches!(toks[i + 1].ident(), Some("only" | "skip" | "todo"))
                && toks[i + 2].is("(")
        })
        .count();
    let options = (0..toks.len().saturating_sub(1))
        .filter(|&i| {
            matches!(toks[i].ident(), Some("test" | "it" | "describe" | "suite"))
                && toks[i + 1].is("(")
                && options_narrow(&toks, i + 1)
        })
        .count();
    methods + options
}

fn opens(t: &lex::Token) -> bool {
    t.is("(") || t.is("[") || t.is("{")
}

fn closes(t: &lex::Token) -> bool {
    t.is(")") || t.is("]") || t.is("}")
}

/// Whether the call whose `(` is `toks[open]` has an object argument, other
/// than a function body, that sets `only`, `skip` or `todo`.
fn options_narrow(toks: &[lex::Token], open: usize) -> bool {
    let mut depth = 0usize;
    for (j, t) in toks.iter().enumerate().skip(open) {
        if closes(t) {
            depth -= 1;
            if depth == 0 {
                break;
            }
        } else if opens(t) {
            let argument = depth == 1 && (toks[j - 1].is(",") || toks[j - 1].is("("));
            if argument && t.is("{") && sets_a_skip_key(toks, j) {
                return true;
            }
            depth += 1;
        }
    }
    false
}

/// Whether the object literal that opens at `toks[open]` has a key `only`,
/// `skip` or `todo` of its own, whose value is not `false`, `null` or
/// `undefined`.
fn sets_a_skip_key(toks: &[lex::Token], open: usize) -> bool {
    let mut depth = 0usize;
    for (j, t) in toks.iter().enumerate().skip(open) {
        if closes(t) {
            depth -= 1;
            if depth == 0 {
                break;
            }
        } else if opens(t) {
            depth += 1;
        } else if depth == 1 && matches!(t.ident(), Some("only" | "skip" | "todo")) {
            let key = toks[j - 1].is("{") || toks[j - 1].is(",");
            let value = toks.get(j + 2).filter(|_| toks[j + 1].is(":"));
            let falsy = |v: &lex::Token| matches!(v.ident(), Some("false" | "null" | "undefined"));
            if key && value.is_some_and(|v| !falsy(v)) {
                return true;
            }
        }
    }
    false
}

#[test]
fn no_ui_test_is_only_or_skipped() {
    let root = workspace_root();
    let dir = root.join("crates/ralphy-daemon/ui-tests");
    let mut files = Vec::new();
    rust_coupling::collect_all(&dir, &mut files);
    files.retain(|p| {
        let rel = p.strip_prefix(&dir).unwrap_or(p);
        p.extension().is_some_and(|e| e == "mjs") && !rel.starts_with("fixtures")
    });
    assert!(
        files.len() > 50,
        "expected the workbench tests under {}, found {} .mjs files",
        dir.display(),
        files.len()
    );
    let narrowed: Vec<String> = files
        .iter()
        .filter(|p| narrowed_tests(&read(p)) > 0)
        .map(|p| rust_coupling::rel_path(&root, p))
        .collect();
    assert!(
        narrowed.is_empty(),
        "a test with `.only(`, `.skip(` or `.todo(`, or with `only`, `skip` or `todo` set in its \
         options, hides the others; run it or delete it:\n{}",
        narrowed.join("\n")
    );
}

#[test]
fn the_scans_read_code_and_not_comments() {
    let text = "/// `#[ignore]`d\n    #[ignore]\n#[ignore = \"why\"]\n    #[ignore = \"\"]\n";
    assert_eq!(
        ignore_attributes(text),
        [(2, false), (3, true), (4, false)],
        "a comment is not an attribute, and an empty reason is no reason"
    );
    let src = "// test.skip(x)\ntest.only('a.skip(b)', () => {});\nit.todo(1);\nconst skip = 1;\n";
    assert_eq!(narrowed_tests(src), 2);
}

#[test]
fn the_options_form_of_only_skip_and_todo_is_narrowing_too() {
    let narrowing = [
        "test('a', { skip: true }, () => {});",
        "it(\"b\", { only: 'why', timeout: 1 }, fn);",
        "describe('c', { todo: true }, () => {});",
        "t.test('d', { concurrency: 2, skip: reason }, fn);",
    ];
    for src in narrowing {
        assert_eq!(narrowed_tests(src), 1, "{src}");
    }
    let plain = [
        "test('a', { skip: false }, () => {});",
        "test('a', { timeout: 1 }, () => { g({ skip: PART }); });",
        "test('a', () => { f({ skip: true }); });",
        "test('a', { timeout: { skip: true } }, fn);",
        "const page = assembledPage('index.html', { skip: PART });",
    ];
    for src in plain {
        assert_eq!(narrowed_tests(src), 0, "{src}");
    }
}
