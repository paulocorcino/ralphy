use super::*;
use crate::ui_copy::Kind;

const RULES: &str = r#"{
  "casing": {
    "exempt_kinds": ["js:term-notice"],
    "state_words": ["idle"],
    "key_names": ["esc"]
  },
  "proper_nouns": ["Ralphy", "GitHub", "Ctrl"],
  "banned": [
    { "term": "plane", "use": "stage", "reason": "r", "whole_word": true },
    { "term": "ralphy", "use": "Ralphy", "reason": "r", "whole_word": true, "case_sensitive": true }
  ],
  "punctuation": [{ "find": "...", "use": "…", "reason": "r" }],
  "sentence_max_words": 5,
  "plain": [
    { "term": "just", "use": "(remove it)", "whole_word": true },
    { "term": "e.g.", "use": "for example", "whole_word": false }
  ],
  "contractions": { "banned": ["n't", "it's"], "use": "u", "reason": "r" },
  "copy_helpers": ["note"],
  "exemptions": [
    { "file": "index.html", "text": "ralphy update", "rule": "banned:ralphy", "reason": "A command." },
    { "file": "index.html", "text": "gone from the tree", "rule": "casing:first", "reason": "Stale." }
  ]
}"#;

fn rules() -> Rules {
    parse(RULES).expect("the fixture rules parse")
}

fn row(kind: Kind, text: &str) -> Row {
    Row {
        text: text.to_string(),
        file: "index.html".to_string(),
        line: 7,
        kind,
        area: String::new(),
        concatenated: text.contains('{'),
        pinned_by: Vec::new(),
        continues: false,
    }
}

/// The rule ids one text breaks.
fn broken(kind: Kind, text: &str) -> Vec<String> {
    let rules = rules();
    check(&[row(kind, text)], &rules)
        .violations
        .into_iter()
        .map(|v| v.rule)
        .collect()
}

#[test]
fn a_banned_term_is_reported_and_a_path_that_holds_it_is_not() {
    assert_eq!(
        broken(Kind::Title, "Fences on the plane"),
        vec!["banned:plane"]
    );
    assert_eq!(broken(Kind::Text, "About ralphy"), vec!["banned:ralphy"]);
    assert!(broken(Kind::Text, "Edit <repo>/.ralphy/settings.json").is_empty());
    assert!(broken(Kind::Text, "Airplane mode").is_empty());
    assert!(broken(Kind::Text, "Remove from Ralphy").is_empty());
}

#[test]
fn casing_reports_a_lowercase_start_and_title_case() {
    assert_eq!(broken(Kind::Title, "close"), vec!["casing:first"]);
    assert_eq!(broken(Kind::Text, "Staged Changes"), vec!["casing:title"]);
    let rules = rules();
    let report = check(&[row(Kind::Text, "Branch & Git Status")], &rules);
    assert_eq!(report.violations[0].hint, "lowercase Git, Status");
}

#[test]
fn text_the_guide_allows_is_not_reported() {
    for (kind, text) in [
        (Kind::Title, "Refresh projects"),
        (Kind::Text, "Open on GitHub"),
        (Kind::Text, "idle"),
        (Kind::KeyBar, "esc"),
        (Kind::TermNotice, "[session closed]"),
        (Kind::Confirm, "Delete “My Notes”?"),
        (
            Kind::Confirm,
            "Reopen as {label}? Unsaved changes are lost.",
        ),
        (Kind::Title, "Search files (Ctrl+Shift+F)"),
        (Kind::Title, "Sort by title A–Z"),
        (Kind::Title, "{p.name} stops now."),
        (Kind::Title, "({n} consoles)"),
        (Kind::Text, "Ralphy · Detached file"),
        (Kind::Text, "This path's changes"),
        (Kind::Text, "Opens htop, btop, lazygit… The console closes."),
        (Kind::Text, "Sent as ‘Bearer Token’ here."),
        (Kind::Text, "A file’s tab reopens it."),
    ] {
        assert!(broken(kind, text).is_empty(), "{text:?} was reported");
    }
}

#[test]
fn a_text_that_goes_on_with_a_sentence_is_not_judged_by_its_first_letter() {
    let rules = rules();
    let mut rest = row(Kind::Text, "to start one.");
    rest.continues = true;
    assert!(check(&[rest], &rules).violations.is_empty());
}

#[test]
fn an_exemption_hides_exactly_its_rule_and_text_and_a_stale_one_is_reported() {
    let rules = rules();
    let rows = [
        row(Kind::Text, "ralphy update"),
        row(Kind::Text, "ralphy update..."),
    ];
    let report = check(&rows, &rules);
    let got: Vec<(&str, &str)> = report
        .violations
        .iter()
        .map(|v| (v.rule.as_str(), v.text.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("casing:first", "ralphy update"),
            ("casing:first", "ralphy update..."),
            ("banned:ralphy", "ralphy update..."),
            ("punctuation:...", "ralphy update..."),
        ]
    );
    let stale: Vec<&str> = report.stale.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(stale, vec!["gone from the tree"]);
    assert!(to_text(&report).contains("index.html: casing:first: gone from the tree"));
}

#[test]
fn an_exemption_without_a_reason_is_refused() {
    let bad = RULES.replace(r#""reason": "A command.""#, r#""reason": "  ""#);
    let err = parse(&bad).expect_err("an empty reason must not load");
    assert!(err.to_string().contains("has no reason"), "{err}");
}

#[test]
fn plain_words_contractions_and_sentence_length_are_reported() {
    assert_eq!(broken(Kind::Text, "Just wait"), vec!["plain:just"]);
    assert_eq!(
        broken(Kind::Placeholder, "For e.g. a login"),
        vec!["plain:e.g."]
    );
    assert!(broken(Kind::Text, "Adjust it").is_empty());
    // A wide whitespace char before a listed term (U+00A0, U+2009).
    assert_eq!(
        broken(Kind::Text, "Wait\u{a0}just a moment."),
        vec!["plain:just"]
    );
    assert_eq!(
        broken(Kind::Text, "Wait\u{2009}just a moment."),
        vec!["plain:just"]
    );
    assert_eq!(
        broken(Kind::Text, "Passwords don't match."),
        vec!["contraction"]
    );
    assert_eq!(broken(Kind::Text, "It’s done."), vec!["contraction"]);
    assert!(broken(Kind::Text, "Commit its changes.").is_empty());
    assert_eq!(
        broken(Kind::Text, "One two three four five six. Short."),
        vec!["sentence-length"]
    );
    // A hole is not a word.
    assert!(broken(Kind::Text, "One {a} two {b} three four five.").is_empty());
}

#[test]
fn the_report_counts_violations_per_rule_and_the_concatenated_texts() {
    let rules = rules();
    let rows = [
        row(Kind::Title, "close"),
        row(Kind::Title, "notes on the plane"),
        row(Kind::Text, "Wake {env}"),
    ];
    let text = to_text(&check(&rows, &rules));
    assert!(text.contains("index.html:7: casing:first: close  (start with a capital letter)"));
    assert!(text.contains("  banned           1\n"));
    assert!(text.contains("  casing:first     2\n"));
    assert!(text.contains("  total            3 in 2 texts\n"));
    assert!(text.contains("Concatenated texts (informational, ADR-0065 §9): 1"));
    assert!(text.contains("Stale exemptions (they match no text; remove them):"));
    assert!(text.ends_with("A violation or a stale exemption fails the check (ADR-0065).\n"));
}

#[test]
fn a_violation_or_a_stale_exemption_fails_and_a_clean_report_passes() {
    let mut rules = rules();
    let clean = [row(Kind::Title, "Close")];
    // The fixture has one stale exemption, so a clean text still fails.
    let stale_only = check(&clean, &rules);
    assert!(stale_only.violations.is_empty() && stale_only.fails());
    rules.exemptions.clear();
    let violation_only = check(&[row(Kind::Title, "close")], &rules);
    assert!(violation_only.stale.is_empty() && violation_only.fails());
    let report = check(&clean, &rules);
    assert!(!report.fails());
    assert!(to_text(&report).ends_with("No violations.\n"));
}
