use super::*;

fn fragment(kind: &str, prose: &str) -> String {
    format!("---\nkind: {kind}\n---\n{prose}\n")
}

#[test]
fn a_fragment_parses_into_its_front_matter_and_its_prose() {
    let entry = |id: &str, kind, text: &str, topic: Option<&str>, headline: Option<&str>| Entry {
        id: id.into(),
        kind,
        text: text.into(),
        topic: topic.map(str::to_owned),
        headline: headline.map(str::to_owned),
    };
    // (case, file stem, fragment, expected entry)
    let rows = [
        (
            "kind and prose",
            "389",
            fragment("feature", "Paste a screenshot into a console."),
            entry(
                "389",
                Kind::Feature,
                "Paste a screenshot into a console.",
                None,
                None,
            ),
        ),
        (
            // CRLF is what a Windows editor writes.
            "windows line endings",
            "1",
            "---\r\nkind: fix\r\n---\r\nIt no longer does that.\r\n".to_owned(),
            entry("1", Kind::Fix, "It no longer does that.", None, None),
        ),
        (
            "topic and headline",
            "1",
            "---
kind: feature
topic: notes
headline: **Notes** — on the stage
---
Prose.
"
            .to_owned(),
            entry(
                "1",
                Kind::Feature,
                "Prose.",
                Some("notes"),
                Some("**Notes** — on the stage"),
            ),
        ),
    ];
    for (case, id, text, want) in rows {
        let got = parse_fragment(id, &text)
            .unwrap_or_else(|e| panic!("{case}: a well-formed fragment must parse: {e:#}"));
        assert_eq!(got, want, "{case}");
    }
}

#[test]
fn an_unknown_kind_is_refused_by_name() {
    let err =
        parse_fragment("1", &fragment("chore", "Something.")).expect_err("chore is not a kind");
    assert!(err.to_string().contains("chore"), "{err}");
}

#[test]
fn a_fragment_with_no_prose_is_refused() {
    let err =
        parse_fragment("1", "---\nkind: fix\n---\n\n").expect_err("a kind alone says nothing");
    assert!(err.to_string().contains("no prose"), "{err}");
}

#[test]
fn a_fragment_with_no_front_matter_is_refused() {
    assert!(parse_fragment("1", "Just some prose.\n").is_err());
    assert!(parse_fragment("1", "---\nkind: fix\nnever closed\n").is_err());
}

#[test]
fn internal_entries_are_consumed_and_never_rendered() {
    let mut history = History::default();
    fold(
        &mut history,
        "v0.1.0-rc.20",
        "2026-09-08",
        vec![
            parse_fragment("1", &fragment("internal", "Split a module.")).expect("parse"),
            parse_fragment(
                "2",
                &fragment("fix", "The keyboard stops covering the prompt."),
            )
            .expect("parse"),
        ],
    );
    let record = &history.releases[0];
    assert_eq!(record.entries.len(), 1, "internal must not be recorded");
    assert_eq!(record.entries[0].kind, Kind::Fix);
    assert!(!render_changelog(&history).contains("Split a module"));
}

#[test]
fn only_a_feature_grade_release_is_announceable() {
    let mut history = History::default();
    fold(
        &mut history,
        "v1",
        "2026-09-08",
        vec![parse_fragment("1", &fragment("fix", "A fix.")).expect("parse")],
    );
    assert!(!history.releases[0].announce, "fixes alone are not news");

    fold(
        &mut history,
        "v2",
        "2026-09-09",
        vec![parse_fragment("2", &fragment("feature", "A feature.")).expect("parse")],
    );
    assert!(history.releases[0].announce);
}

#[test]
fn a_release_is_rendered_breaking_first_and_fixes_last() {
    let mut history = History::default();
    fold(
        &mut history,
        "v0.1.0-rc.20",
        "2026-09-08",
        vec![
            parse_fragment("3", &fragment("fix", "A fix.")).expect("parse"),
            parse_fragment("1", &fragment("breaking", "A break.")).expect("parse"),
            parse_fragment("2", &fragment("feature", "A feature.")).expect("parse"),
        ],
    );
    let text = render_changelog(&history);
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle} missing"))
    };
    assert!(at("### Breaking") < at("### New"));
    assert!(at("### New") < at("### Fixed"));
    assert!(text.contains("- A break. (#1)"));
    assert!(text.contains("## v0.1.0-rc.20 — 2026-09-08"));
}

#[test]
fn the_newest_release_is_rendered_first() {
    let mut history = History::default();
    fold(
        &mut history,
        "v0.1.0-rc.20",
        "2026-09-08",
        vec![parse_fragment("1", &fragment("fix", "Older.")).expect("parse")],
    );
    fold(
        &mut history,
        "v0.1.0-rc.21",
        "2026-09-09",
        vec![parse_fragment("2", &fragment("fix", "Newer.")).expect("parse")],
    );
    let text = render_changelog(&history);
    assert!(text.find("rc.21").expect("rc.21") < text.find("rc.20").expect("rc.20"));
}

#[test]
fn refolding_a_version_replaces_it_rather_than_duplicating_it() {
    let mut history = History::default();
    for prose in ["First attempt.", "Second attempt."] {
        fold(
            &mut history,
            "v0.1.0-rc.20",
            "2026-09-08",
            vec![parse_fragment("1", &fragment("fix", prose)).expect("parse")],
        );
    }
    assert_eq!(history.releases.len(), 1);
    assert_eq!(history.releases[0].entries[0].text, "Second attempt.");
}

#[test]
fn a_release_with_nothing_user_visible_says_so_in_its_notes() {
    let mut history = History::default();
    fold(
        &mut history,
        "v0.1.0-rc.20",
        "2026-09-08",
        vec![parse_fragment("1", &fragment("internal", "Split a module.")).expect("parse")],
    );
    assert_eq!(
        render_notes(&history.releases[0]),
        "No user-visible changes in this release.\n"
    );
}

#[test]
fn a_multi_line_fragment_stays_one_bullet() {
    let mut history = History::default();
    fold(
        &mut history,
        "v1",
        "2026-09-08",
        vec![
            parse_fragment("1", &fragment("feature", "One sentence.\nAnd another."))
                .expect("parse"),
        ],
    );
    let text = render_changelog(&history);
    assert!(text.contains("- One sentence. And another. (#1)"), "{text}");
}

#[test]
fn a_slug_named_fragment_does_not_fake_a_pull_request_link() {
    let mut history = History::default();
    fold(
        &mut history,
        "v1",
        "2026-09-08",
        vec![parse_fragment("release-watch", &fragment("feature", "A feature.")).expect("parse")],
    );
    let text = render_notes(&history.releases[0]);
    assert_eq!(text.trim_end().lines().last(), Some("- A feature."));
    assert!(!text.contains("(#"), "no issue number was claimed: {text}");
}

#[test]
fn an_absent_topic_field_stays_absent_in_json() {
    let mut history = History::default();
    fold(
        &mut history,
        "v0.1.0-rc.20",
        "2026-09-08",
        vec![parse_fragment("1", &fragment("feature", "A feature.")).expect("parse")],
    );
    let json = serde_json::to_string_pretty(&history).expect("serialize");
    assert!(
        !json.contains("\"topic\""),
        "an absent field stays absent, so past records re-serialize unchanged: {json}"
    );
}

fn tagged(id: &str, kind: &str, topic: &str, headline: Option<&str>, prose: &str) -> Entry {
    let headline = headline.map_or(String::new(), |h| format!("headline: {h}\n"));
    parse_fragment(
        id,
        &format!("---\nkind: {kind}\ntopic: {topic}\n{headline}---\n{prose}\n"),
    )
    .expect("a well-formed fragment must parse")
}

#[test]
fn over_long_prose_and_headlines_are_refused() {
    let long = "x".repeat(MAX_PROSE_CHARS + 1);
    let err = parse_fragment("1", &fragment("fix", &long)).expect_err("over the cap");
    assert!(err.to_string().contains("cap"), "{err}");
    assert!(parse_fragment("1", &fragment("fix", &"é".repeat(MAX_PROSE_CHARS))).is_ok());

    let headline = "h".repeat(MAX_HEADLINE_CHARS + 1);
    assert!(parse_fragment(
        "1",
        &format!("---\nkind: fix\nheadline: {headline}\n---\nShort.\n")
    )
    .is_err());
}

#[test]
fn the_page_gives_a_topic_one_line_and_drops_its_launch_fixes() {
    let mut history = History::default();
    fold(
        &mut history,
        "v1",
        "2026-09-24",
        vec![
            tagged(
                "1",
                "feature",
                "notes",
                None,
                "Write a note. It saves itself.",
            ),
            tagged(
                "2",
                "feature",
                "notes",
                Some("**Notes** — markdown on the stage"),
                "More.",
            ),
            tagged(
                "3",
                "fix",
                "notes",
                None,
                "A note keeps its last character.",
            ),
            tagged(
                "4",
                "fix",
                "consoles",
                None,
                "A console reconnects. Even behind a tunnel.",
            ),
            tagged("5", "security", "notes", None, "A note no longer leaks."),
        ],
    );
    let page = render_notes(&history.releases[0]);
    assert_eq!(
        page,
        "\n### 🔒 Security\n\n- A note no longer leaks. (#5)\n\
             \n### ✨ New\n\n- **Notes** — markdown on the stage (#2)\n\
             \n### 🐛 Fixed\n\n- A console reconnects. (#4)\n"
    );

    let record = render_changelog(&history);
    assert!(
        record.contains("### New"),
        "the record keeps its plain headings"
    );
    assert!(
        record.contains("A note keeps its last character. (#3)"),
        "{record}"
    );
    assert!(
        record.contains("Write a note. It saves itself. (#1)"),
        "{record}"
    );
}

#[test]
fn features_with_no_topic_each_keep_their_own_line() {
    let mut history = History::default();
    fold(
        &mut history,
        "v1",
        "2026-09-24",
        vec![
            parse_fragment("1", &fragment("feature", "Paste an image. It lands.")).unwrap(),
            parse_fragment("2", &fragment("feature", "Rename a file. It moves.")).unwrap(),
        ],
    );
    assert_eq!(
        render_notes(&history.releases[0]),
        "\n### ✨ New\n\n- Paste an image. (#1)\n- Rename a file. (#2)\n"
    );
}
