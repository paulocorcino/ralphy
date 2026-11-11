use super::*;

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
    let over: Vec<String> = report(root)
        .expect("report")
        .iter()
        .map(|row| row.path().to_owned())
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
fn a_line_of_rust_may_have_100_characters() {
    let text = format!(
        "{}\n{}\n{}\n",
        "a".repeat(100),
        "b".repeat(101),
        "é".repeat(100)
    );
    assert_eq!(
        wide_line_rows(&text),
        [(2, 101)],
        "101 characters; 100 accented letters are 200 bytes but 100 characters"
    );
    assert!(Lang::Rust.has_width() && Lang::TypeScript.has_width());
    assert!(!Lang::Html.has_width(), "an HTML line is not measured");
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
    assert_eq!(
        report(root).expect("report"),
        [Row::Width {
            path: "crates/a/src/wide.rs".to_owned(),
            line: 1,
            width: 111
        }]
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

/// A temp tree with `files` written under it, as `(path, text)` pairs.
fn tree(files: &[(&str, String)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    for (rel, text) in files {
        let path = dir.path().join(rel);
        let parent = path.parent().expect("a relative path with a parent");
        std::fs::create_dir_all(parent).expect("create the parent");
        std::fs::write(&path, text).expect("write the file");
    }
    dir
}

fn size_row(path: &str, lines: usize) -> Row {
    Row::Size {
        path: path.to_owned(),
        lines,
    }
}

#[test]
fn strict_counts_every_line() {
    let text = format!(
        "{}{}{}",
        "let a = 1;
"
        .repeat(400),
        "// note
"
        .repeat(50),
        "
"
        .repeat(51)
    );
    let dir = tree(&[("crates/a/src/lib.rs", text.clone())]);
    assert_eq!(
        report(dir.path()).expect("report"),
        [size_row("crates/a/src/lib.rs", 501)]
    );
}

#[test]
fn strict_counts_the_inline_test_module() {
    let text = format!(
        "{}#[cfg(test)]
mod tests {{
{}}}
",
        "let a = 1;
"
        .repeat(300),
        "    let b = 2;
"
        .repeat(198)
    );
    assert_eq!(text.lines().count(), 501);
    let dir = tree(&[("crates/a/src/lib.rs", text)]);
    assert_eq!(
        report(dir.path()).expect("report"),
        [size_row("crates/a/src/lib.rs", 501)]
    );
}

#[test]
fn strict_width_reads_typescript_lines_with_their_number() {
    let text = format!(
        "const a = 1;
const b = 2;
{}
{}
",
        "x".repeat(101),
        "é".repeat(100)
    );
    let dir = tree(&[("crates/ui/a.ts", text)]);
    assert_eq!(
        report(dir.path()).expect("report"),
        [Row::Width {
            path: "crates/ui/a.ts".to_owned(),
            line: 3,
            width: 101
        }]
    );
}

#[test]
fn strict_width_skips_html() {
    let page = format!(
        "{}<p>{}</p>
",
        "<p>x</p>
"
        .repeat(500),
        "y".repeat(101)
    );
    let dir = tree(&[("crates/ui/index.html", page)]);
    assert_eq!(
        report(dir.path()).expect("report"),
        [size_row("crates/ui/index.html", 501)]
    );
}

#[test]
fn strict_report_lists_and_does_not_fail() {
    let dir = tree(&[
        (
            "crates/b/src/big.rs",
            "let a = 1;
"
            .repeat(501),
        ),
        (
            "crates/a/src/wide.ts",
            format!(
                "{}
",
                "x".repeat(101)
            ),
        ),
    ]);
    let rows = report(dir.path()).expect("it lists, and does not fail");
    assert_eq!(
        rows.iter().map(Row::render).collect::<Vec<_>>(),
        [
            "crates/a/src/wide.ts:1	101	characters, over the width of 100",
            "crates/b/src/big.rs	501	lines, over the limit of 500 (every line counted)",
        ]
    );
}

/// The error of `check` on a tree of `files`.
fn check_error(files: &[(&str, String)]) -> String {
    let dir = tree(files);
    let error = check(dir.path()).expect_err("the tree breaks the rule");
    format!("{error:#}")
}

#[test]
fn check_fails_a_file_of_501_lines_whatever_they_hold() {
    let error = check_error(&[("crates/a/src/notes.rs", "// a note\n".repeat(501))]);
    assert!(error.contains("crates/a/src/notes.rs"), "{error}");
}

#[test]
fn check_fails_a_typescript_line_of_101_characters() {
    let text = format!("const a = 1;\n{}\n", "x".repeat(101));
    let error = check_error(&[("crates/ui/wide.ts", text)]);
    assert!(error.contains("crates/ui/wide.ts:2"), "{error}");
}

#[test]
fn check_measures_html_size_but_not_width() {
    let error = check_error(&[("crates/ui/big.html", "<p>x</p>\n".repeat(501))]);
    assert!(error.contains("crates/ui/big.html"), "{error}");
    let wide = format!("{}<p>{}</p>\n", "<p>x</p>\n".repeat(499), "y".repeat(101));
    let dir = tree(&[("crates/ui/wide.html", wide)]);
    assert!(check(dir.path()).is_ok(), "an HTML line is not measured");
}

#[test]
fn check_passes_a_tree_within_the_rule() {
    let line = format!("{}\n", "a".repeat(100));
    let dir = tree(&[
        ("crates/a/src/lib.rs", line.repeat(500)),
        ("crates/ui/app.ts", line.repeat(500)),
        ("crates/ui/index.html", line.repeat(500)),
    ]);
    check(dir.path()).expect("500 lines of 100 characters fit");
}

#[test]
fn the_base_and_strict_flags_are_refused() {
    for args in [vec!["--base", "main"], vec!["--strict"]] {
        let args: Vec<String> = args.into_iter().map(String::from).collect();
        let error = oversized_cmd(&args).expect_err("the flag is gone");
        assert!(
            error.to_string().contains("usage: oversized [--check]"),
            "{error}"
        );
    }
}
