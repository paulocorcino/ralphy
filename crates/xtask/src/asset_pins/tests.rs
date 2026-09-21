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
        let app = include_str!("../assets/ui/app.js");
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
        .any(|(t, a)| t.contains("function shell(") && *a == Some("app.js")));
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
