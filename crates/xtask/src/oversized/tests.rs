use super::*;

#[test]
fn comments_blanks_and_the_test_module_do_not_count() {
    let src = "//! crate doc\n\
               /// item doc\n\
               fn a() {\n\
               \n\
                   // why\n\
                   let x = 1; // trailing comment, still code\n\
               }\n\
               #[cfg(test)]\n\
               mod tests;\n\
               fn b() {}\n\
               #[cfg(test)]\n\
               pub(crate) mod more {\n\
               fn t() {}\n\
               }\n";
    assert_eq!(
        code_lines(src),
        6,
        "fn a, let, close, the `mod tests;` attribute and line, fn b"
    );
}

#[test]
fn a_block_comment_does_not_count() {
    let src = "/**\n\
                * A doc block.\n\
                * let x = 1;\n\
                */\n\
               const a = 1;\n\
               /* one line */\n\
               let b = *p;\n";
    assert_eq!(code_lines(src), 2, "`const a` and `let b` only");
}

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
    let over: Vec<String> = measure(root)
        .expect("measure")
        .into_iter()
        .map(|file| file.path)
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
fn an_html_comment_is_not_code() {
    let src = "<div>
               <!-- a comment
               that spans
               <p>not code</p>
               -->
               <!-- one line -->
               
               <b>x</b> <!-- after code -->
               </div>
";
    assert_eq!(
        lines_of_code(Lang::Html, src),
        3,
        "`<div>`, the line with code before a comment, `</div>`"
    );
    let after = "<!-- opens
closes --> <i>y</i>
";
    assert_eq!(lines_of_code(Lang::Html, after), 1, "code after `-->`");
}

#[test]
fn a_page_that_grows_past_its_limit_fails() {
    let page = |code: usize| {
        let mut text = String::from(
            "<!-- a long
comment
of three lines -->
",
        );
        text.push_str(
            &"<p>x</p>
"
            .repeat(code),
        );
        text
    };
    let file = Oversized {
        rule: Rule::Size,
        path: "crates/a/ui/index.html".to_string(),
        lines: lines_of_code(Lang::Html, &page(501)),
    };
    assert_eq!(file.lines, 501);
    let grown = growth_since(&file, &page(500)).expect("it grew by one code line");
    assert!(
        grown.starts_with("crates/a/ui/index.html: 500 -> 501"),
        "{grown}"
    );
    assert_eq!(growth_since(&file, &page(501)), None, "the same size");
}

#[test]
fn a_file_over_its_limit_may_shrink_but_not_grow() {
    let file = Oversized {
        rule: Rule::Size,
        path: "crates/a/src/big.rs".to_string(),
        lines: 900,
    };
    assert_eq!(growth_error(&file, 900), None, "the same size");
    assert_eq!(growth_error(&file, 950), None, "it shrank");
    let grown = growth_error(&file, 899).expect("it grew by one line");
    assert!(
        grown.starts_with("crates/a/src/big.rs: 899 -> 900"),
        "{grown}"
    );
    assert!(
        growth_error(&file, 0).is_some(),
        "a file the base does not have"
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
        Rule::Width.count(Lang::Rust, &text),
        Some(1),
        "101 characters; 100 accented letters are 200 bytes but 100 characters"
    );
    assert_eq!(
        Rule::Width.count(Lang::TypeScript, &text),
        None,
        "only Rust is measured"
    );
    assert_eq!(Rule::Width.count(Lang::Html, &text), None);
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
    let over: Vec<(Rule, String, usize)> = measure(root)
        .expect("measure")
        .into_iter()
        .map(|file| (file.rule, file.path, file.lines))
        .collect();
    assert_eq!(over, [(Rule::Width, "crates/a/src/wide.rs".to_string(), 1)]);
}

#[test]
fn a_new_wide_line_fails_and_an_old_one_may_stay() {
    let line = format!("    // {}\n", "x".repeat(100));
    let file = Oversized {
        rule: Rule::Width,
        path: "crates/a/src/lib.rs".to_string(),
        lines: 2,
    };
    assert_eq!(
        growth_since(&file, &line.repeat(2)),
        None,
        "the same two lines"
    );
    let grown = growth_since(&file, &line).expect("one wide line more");
    assert!(
        grown.starts_with("crates/a/src/lib.rs: 1 -> 2 lines wider than 100 characters"),
        "{grown}"
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
