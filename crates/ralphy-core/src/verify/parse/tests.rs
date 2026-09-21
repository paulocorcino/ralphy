use super::*;

#[test]
fn parse_commands_one_per_line() {
    let md = "# Plan\n\n## Verify\n\ncargo fmt --check\ncargo test -p ralphy-core\n\n## Next\n";
    let spec = parse_verify(md);
    assert_eq!(
        spec,
        VerifySpec::Commands(vec![
            vec!["cargo".into(), "fmt".into(), "--check".into()],
            vec![
                "cargo".into(),
                "test".into(),
                "-p".into(),
                "ralphy-core".into()
            ],
        ])
    );
}

#[test]
fn parse_none_is_opt_out() {
    assert_eq!(parse_verify("## Verify\n\nnone\n"), VerifySpec::None);
    // Case-insensitive.
    assert_eq!(parse_verify("## Verify\nNONE\n"), VerifySpec::None);
}

#[test]
fn parse_absent_section_is_unspecified() {
    assert_eq!(
        parse_verify("# Plan\n\n## Steps\n- [ ] do\n"),
        VerifySpec::Unspecified
    );
}

#[test]
fn parse_empty_section_is_unspecified() {
    let md = "## Verify\n\n## Notes\nstuff\n";
    assert_eq!(parse_verify(md), VerifySpec::Unspecified);
}

#[test]
fn parse_is_fence_tolerant() {
    let md = "## Verify\n\n```\ncargo test\n```\n";
    assert_eq!(
        parse_verify(md),
        VerifySpec::Commands(vec![vec!["cargo".into(), "test".into()]])
    );
}

#[test]
fn parse_quoted_args_stay_one_token() {
    let md = "## Verify\n\nsh -c \"cargo test --all\"\n";
    assert_eq!(
        parse_verify(md),
        VerifySpec::Commands(vec![vec![
            "sh".into(),
            "-c".into(),
            "cargo test --all".into()
        ]])
    );
}

#[test]
fn parse_single_quotes_too() {
    assert_eq!(
        tokenize("echo 'hello world' bye"),
        vec!["echo", "hello world", "bye"]
    );
}

/// A bulleted checklist is rejected at parse time: the leading `-` would tokenize
/// into a bogus `-` program the gate spawn-fails on, so the real command never runs
/// (#181). The error must name the offending line so the plan author can fix it.
#[test]
fn parse_bulleted_list_is_invalid() {
    let md = "## Verify\n\n- cargo test\n- cargo fmt --check\n";
    match parse_verify(md) {
        VerifySpec::Invalid(error) => {
            assert!(
                error.contains("one bare command per line"),
                "names the contract: {error}"
            );
            assert!(error.contains("`- cargo test`"), "names offender: {error}");
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}

/// A `- [ ]` checkbox line is markdown prose, not a command — rejected (#181).
#[test]
fn parse_checkbox_list_is_invalid() {
    let md = "## Verify\n\n- [ ] cargo test\n";
    match parse_verify(md) {
        VerifySpec::Invalid(error) => {
            assert!(
                error.contains("`- [ ] cargo test`"),
                "names offender: {error}"
            );
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}

/// The real-world trigger (#181): a backtick-wrapped command with trailing prose.
/// The leading `-` and the backticks both mark it as markdown, not a command.
#[test]
fn parse_backtick_prose_is_invalid() {
    let md = "## Verify\n\n- `npm run lint` — passou, sem warnings\n";
    assert!(matches!(parse_verify(md), VerifySpec::Invalid(_)));
}

/// A `*`/`+` bullet or a bare backtick-wrapped command (no leading bullet) is also
/// markdown prose — the leading backtick alone marks it (#181).
#[test]
fn parse_star_bullet_and_bare_backtick_are_invalid() {
    assert!(matches!(
        parse_verify("## Verify\n\n* cargo test\n"),
        VerifySpec::Invalid(_)
    ));
    assert!(matches!(
        parse_verify("## Verify\n\n`cargo test`\n"),
        VerifySpec::Invalid(_)
    ));
}

/// The clean bare-command path is untouched: a section that is one bare command
/// per line still parses to `VerifySpec::Commands` (regression guard for #181).
#[test]
fn parse_bare_commands_unaffected() {
    let md = "## Verify\n\ncargo test\ncargo fmt --check\n";
    assert_eq!(
        parse_verify(md),
        VerifySpec::Commands(vec![
            vec!["cargo".into(), "test".into()],
            vec!["cargo".into(), "fmt".into(), "--check".into()],
        ])
    );
}

/// The live #268 trigger: a cursor plan authored a defensive `sh -c` check with a
/// nested backslash-escaped quote. The tokenizer cannot honor `\"`, so instead of
/// mis-splitting it into a garbage argv that fails opaquely at runtime (spending the
/// repair budget), it is rejected at parse time with an actionable message.
#[test]
fn parse_nested_escaped_quote_is_invalid() {
    let md = "## Verify\n\nsh -c \"test \\\"$(git diff-tree --name-only -r HEAD)\\\" = \\\"README.md\\\"\"\n";
    match parse_verify(md) {
        VerifySpec::Invalid(error) => {
            assert!(
                error.contains("no shell") && error.contains("backslash-escaped quotes"),
                "names the contract: {error}"
            );
            assert!(error.contains("sh -c"), "names offender: {error}");
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}

/// The two tokenizer-safe rewrites of the same check parse cleanly to `Commands`:
/// single outer quotes (double quotes stay literal inside `'...'`) and a bare
/// `python -c` one-liner (regression guard for #268 — no over-broad rejection).
#[test]
fn parse_escape_free_quotes_still_parse() {
    // Single outer quotes: the inner double quotes are literal, no backslash needed.
    let single = "## Verify\n\nsh -c 'test \"$x\" = y'\n";
    assert_eq!(
        parse_verify(single),
        VerifySpec::Commands(vec![vec![
            "sh".into(),
            "-c".into(),
            "test \"$x\" = y".into(),
        ]])
    );
    // A python one-liner with double-quoted arg — no escaped quotes, parses fine.
    let py = "## Verify\n\npython -c \"assert open('LAB.md').read()\"\n";
    assert!(matches!(parse_verify(py), VerifySpec::Commands(_)));
}

/// A Windows path in a verify command carries single backslashes before non-quote
/// chars — these must NOT be mistaken for escaped quotes (#268 false-positive guard).
#[test]
fn parse_windows_path_backslashes_are_not_escaped_quotes() {
    let md = "## Verify\n\npython C:\\tools\\check.py\n";
    assert_eq!(
        parse_verify(md),
        VerifySpec::Commands(vec![vec!["python".into(), "C:\\tools\\check.py".into(),]])
    );
}

#[test]
fn parse_stops_at_next_heading() {
    let md = "## Verify\ncargo test\n## Other\ncargo bogus\n";
    assert_eq!(
        parse_verify(md),
        VerifySpec::Commands(vec![vec!["cargo".into(), "test".into()]])
    );
}
