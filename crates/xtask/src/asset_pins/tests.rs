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

/// `include_str!(…)` as the receiver ends in a macro's `!`, which is not a
/// negation; a negated macro receiver still is.
#[test]
fn a_macro_call_receiver_is_not_a_negation() {
    assert_eq!(
        classify(r#"assert!(include_str!("../../assets/ui/a.ts").contains("x"));"#),
        Some(Shape::Identifier)
    );
    assert_eq!(
        classify(r#"assert!(!include_str!("../../assets/ui/a.ts").contains("x"));"#),
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

/// A raw string with no `#` keeps its backslashes; a plain string decodes them.
#[test]
fn a_raw_string_with_no_hashes_keeps_its_backslashes() {
    assert_eq!(raw_literals(r#"r"a\nb""#), vec![r"a\nb".to_string()]);
}

/// An `r` that does not open a raw string is read as the plain text it is.
#[test]
fn an_r_hash_with_no_quote_is_not_a_raw_string() {
    assert_eq!(raw_literals("r#x \"#y\""), vec!["#y".to_string()]);
}

/// A raw string that never closes falls back to the plain scanner, which ends
/// the literal at the end of input.
#[test]
fn an_unterminated_raw_string_falls_back_to_a_plain_one() {
    assert_eq!(raw_literals("r#\"abc"), vec!["abc".to_string()]);
}

#[test]
fn an_unterminated_plain_string_ends_at_the_end_of_input() {
    assert_eq!(raw_literals("\"abc"), vec!["abc".to_string()]);
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
        let app = include_str!("../assets/ui/app.ts");
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
        .any(|(t, a)| t.contains("function shell(") && *a == Some("app.ts")));
}

/// The `include_str!` marker also matches a path outside the UI
/// (`../vendor-build/…`); a UI asset later on the same line still names the
/// asset.
#[test]
fn a_ui_asset_after_another_embed_on_the_line_is_booked() {
    let src = r#"
fn t() {
    let (_v, js) = (include_str!("../vendor-build/a.js"), include_str!("../../assets/ui/x.ts"));
    assert!(js.contains("needle"));
}
"#;
    let mut out = Vec::new();
    collect("x.rs", src, &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].asset.as_deref(), Some("x.ts"));
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

/// `balanced` closes on the line where the delimiter opened on its first line
/// closes, and reports the index one past it.
#[test]
fn a_block_closes_on_its_balancing_line() {
    // (case, lines, expected block, expected next)
    let rows: [(&str, &[&str], &str, usize); 4] = [
        (
            "an escaped quote does not end the string",
            &[r#"let t = ["a\"]","#, r#""b"];"#],
            r#"let t = ["a\"]", "b"]; "#,
            2,
        ),
        (
            "a delimiter inside a string is not counted",
            &[r#"let t = ["]","#, r#""x"];"#],
            r#"let t = ["]", "x"]; "#,
            2,
        ),
        (
            "a close before any open does not end the block",
            &["foo)", "x = [1,", "2]"],
            "foo) x = [1, 2] ",
            3,
        ),
        (
            "a block that closes on its own line",
            &["x = [1, 2];", "later"],
            "x = [1, 2]; ",
            1,
        ),
    ];
    for (case, lines, block, next) in rows {
        assert_eq!(
            balanced(lines, 0, '[', ']'),
            (block.to_string(), next),
            "{case}"
        );
    }
}

/// A block may close on its 202nd line; one that closes on its 203rd is lost.
#[test]
fn a_block_may_run_to_its_202nd_line() {
    let closes_on = |n: usize| -> (String, usize) {
        let lines: Vec<&str> = std::iter::once("for pin in [")
            .chain(std::iter::repeat_n("    \"noise\",", n - 2))
            .chain(std::iter::once("]"))
            .collect();
        balanced(&lines, 0, '[', ']')
    };
    let (block, next) = closes_on(202);
    assert!(block.ends_with("] "), "a 202-line block is kept");
    assert_eq!(next, 202);
    let (block, next) = closes_on(203);
    assert!(block.is_empty(), "a 203-line block is lost");
    assert_eq!(next, 1);
}

/// `text` is squeezed to 96 chars and `keep` drops prose, so a long assert
/// loses its needle there. `needles` keeps every literal whole, which is what
/// `ui-copy` matches UI text against.
#[test]
fn needles_keep_every_literal_of_a_long_assert_whole() {
    let src = r#"
    fn t() {
        let js = include_str!("../assets/ui/wb-console.js");
        assert!(js.contains("create.title = \"cut a new worktree and restart this console in it\";"), "the create button explains itself");
    }
"#;
    let mut out = Vec::new();
    collect("x.rs", src, &mut out);
    assert_eq!(out.len(), 1);
    assert!(out[0].text.chars().count() <= 96);
    assert_eq!(
        out[0].needles,
        vec![
            r#"create.title = "cut a new worktree and restart this console in it";"#.to_string(),
            "the create button explains itself".to_string(),
        ]
    );
}

/// A comment that quotes an assertion is prose about a pin, not a pin.
#[test]
fn a_comment_describing_a_pin_is_not_a_pin() {
    let src = r#"
    fn t() {
        let js = include_str!("../assets/ui/app.ts");
        // assert!(js.contains("function foo("), "msg");
    }
"#;
    let mut out = Vec::new();
    collect("x.rs", src, &mut out);
    assert!(out.is_empty());
}

/// A `const` table of pins is a list of claims, one per element, with the
/// shape of a plain identifier.
#[test]
fn a_const_table_of_pins_is_one_identifier_claim_per_element() {
    let src = r#"
    fn t() {
        let js = include_str!("../assets/ui/app.ts");
        const PINS: &[&str] = &["function foo(", "function bar("];
    }
"#;
    let mut out = Vec::new();
    collect("x.rs", src, &mut out);
    let texts: Vec<&str> = out.iter().map(|p| p.text.as_str()).collect();
    assert_eq!(texts, vec!["function foo(", "function bar("]);
    assert!(out.iter().all(|p| p.shape == Shape::Identifier));
    assert!(out.iter().all(|p| p.asset.as_deref() == Some("app.ts")));
}

/// A `let` that carves a region with `find(…).expect(…)` is a pin in its own
/// right: the one that panics under a split.
#[test]
fn a_scoped_slice_let_is_a_pin_of_its_own() {
    let src = r#"
    fn t() {
        let js = include_str!("../assets/ui/app.ts");
        let region = js.find("function foo(").expect("foo exists");
    }
"#;
    let mut out = Vec::new();
    collect("x.rs", src, &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].shape, Shape::ScopedSlice);
    assert_eq!(out[0].asset.as_deref(), Some("app.ts"));
    assert_eq!(out[0].line, 4);
}

/// `get_local` fetches assets and routes. Only a name with an extension is an
/// asset.
#[test]
fn a_fetched_route_is_not_an_asset_but_a_fetched_file_is() {
    let src = r#"
    fn route() {
        let body = get_local("/api/session");
        assert!(body.contains("ok"), "msg");
    }
    fn asset() {
        let body = get_local("/app.js");
        assert!(body.contains("function foo("), "msg");
    }
"#;
    let mut out = Vec::new();
    collect("x.rs", src, &mut out);
    let booked: Vec<Option<&str>> = out.iter().map(|p| p.asset.as_deref()).collect();
    assert_eq!(booked, vec![None, Some("app.js")]);
}

/// A new test starts clean: the asset, the bindings and the normalized
/// variables of the previous test must not reach it. `async fn` starts a test
/// like `fn` does.
#[test]
fn a_new_test_forgets_what_the_previous_test_read() {
    let src = r#"
    fn first() {
        let html = include_str!("../assets/ui/index.html");
        let squeezed = html.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    fn second() {
        assert!(html.contains("a("), "msg");
        assert!(squeezed.contains("b("), "msg");
    }
    async fn third() {
        let html = include_str!("../assets/ui/index.html");
    }
    async fn fourth() {
        assert!(html.contains("c("), "msg");
    }
"#;
    let mut out = Vec::new();
    collect("x.rs", src, &mut out);
    assert_eq!(out.len(), 3);
    assert!(out.iter().all(|p| p.asset.is_none()));
    assert!(out.iter().all(|p| p.shape == Shape::Identifier));
}

/// A test file one directory deeper reaches the assets with one more `../`.
/// The daemon's `src/tests/` children are such files.
#[test]
fn gather_reads_the_children_of_the_daemon_tests_folder() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("crates/ralphy-daemon/src/tests");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("child.rs"),
        r#"
#[test]
fn reads_x() {
    let js = include_str!("../../assets/ui/x.ts");
    assert!(js.contains("needle"), "msg");
}
"#,
    )
    .unwrap();
    let pins = gather(root.path()).unwrap();
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0].asset.as_deref(), Some("x.ts"));
    assert!(
        pins[0].file.ends_with("src/tests/child.rs"),
        "{}",
        pins[0].file
    );
}

/// The path an `include_str!` argument names is how a test NAMES an asset, at
/// any depth.
#[test]
fn an_include_path_of_any_depth_is_not_a_claim() {
    let mut out = Vec::new();
    keep(&mut out, "../../assets/ui/a.ts".to_string());
    keep(&mut out, "../assets/ui/a.ts".to_string());
    assert!(out.is_empty(), "{out:?}");
}
