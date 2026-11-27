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

/// Markdown prose and an unparseable quote are rejected at parse time, and the error
/// names the offending line (#181, #268). Each row: name, section body, substrings.
#[test]
fn parse_invalid_sections_name_the_offender() {
    let nested = "sh -c \"test \\\"$(git diff-tree --name-only -r HEAD)\\\" = \
    \\\"README.md\\\"\"\n";
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "bulleted",
            "- cargo test\n- cargo fmt --check\n",
            &["one bare command per line", "`- cargo test`"],
        ),
        ("checkbox", "- [ ] cargo test\n", &["`- [ ] cargo test`"]),
        (
            "backtick prose",
            "- `npm run lint` - passou, sem warnings\n",
            &[],
        ),
        ("star bullet", "* cargo test\n", &[]),
        ("bare backtick", "`cargo test`\n", &[]),
        (
            "nested escaped quote",
            nested,
            &["no shell", "backslash-escaped quotes", "sh -c"],
        ),
    ];
    for (name, body, wanted) in cases {
        match parse_verify(&format!("## Verify\n\n{body}")) {
            VerifySpec::Invalid(error) => {
                for part in *wanted {
                    assert!(error.contains(part), "{name}: wants {part}: {error}");
                }
            }
            other @ (VerifySpec::None | VerifySpec::Unspecified | VerifySpec::Commands(_)) => {
                panic!("{name}: expected Invalid, got {other:?}")
            }
        }
    }
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
