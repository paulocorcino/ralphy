use super::*;

/// The bootstrap must not stamp one label with another's identity: before
/// #313 it hardcoded `needs-split`'s description (and a colour matching no
/// spec at all) onto every label it created.
#[test]
fn label_bootstrap_spec_reads_the_canonical_roster() {
    for name in [
        crate::runner::NEEDS_HUMAN_REVIEW_LABEL,
        crate::runner::NEEDS_SPLIT_LABEL,
    ] {
        let spec = super::super::labels::ralphy_label_specs(None)
            .into_iter()
            .find(|s| s.name == name)
            .expect("the roster carries every label the runner applies");
        assert_eq!(label_bootstrap_spec(name), (spec.color, spec.description));
    }
    assert_eq!(
        label_bootstrap_spec("not-a-ralphy-label"),
        (
            "ededed".to_string(),
            "Ralphy: not-a-ralphy-label".to_string()
        )
    );
}

/// The non-`@me` arm returns the value verbatim and spawns NO process — so it
/// resolves against a nonexistent `repo_root` without error (proof of no `gh`).
#[test]
fn resolve_login_passes_through_concrete_login() {
    assert_eq!(
        resolve_login("ralphy-bot", Path::new("/nonexistent")).unwrap(),
        "ralphy-bot"
    );
}

/// The `@me` arm hits the live `gh api user`. Ignored by default (needs network
/// + `gh` auth); mirrors the `e2e_references_for_bioledger_29` ignore pattern.
///   cargo test -p ralphy-core resolve_login_at_me_hits_gh_api_user -- --ignored --nocapture
#[test]
#[ignore = "live network: gh api user"]
fn resolve_login_at_me_hits_gh_api_user() {
    let login = resolve_login("@me", Path::new(".")).expect("gh api user");
    println!("resolve_login(@me) = {login:?}");
    assert!(!login.is_empty(), "@me must resolve to a non-empty login");
}

#[test]
fn parse_issue_url_reads_trailing_number() {
    // (case, `gh issue create` output, expected number; `None` is an error)
    let rows = [
        (
            "plain url",
            "https://github.com/owner/repo/issues/42",
            Some(42),
        ),
        (
            "whitespace, trailing slash and a preamble line",
            "Creating issue...\nhttps://github.com/o/r/issues/7/\n",
            Some(7),
        ),
        ("no url", "no url here", None),
    ];
    for (case, text, want) in rows {
        assert_eq!(parse_issue_url(text).ok(), want, "{case}");
    }
}

#[test]
fn parse_issue_labels_reads_names_and_tolerates_empty() {
    // `gh issue view --json labels` shape: a `labels` array of `{name,...}`.
    let json = r#"{"labels": [{"name": "ready-for-human"}, {"name": "needs-triage"}]}"#;
    assert_eq!(
        parse_issue_labels(json).unwrap(),
        vec!["ready-for-human".to_string(), "needs-triage".to_string()]
    );
    // No labels → empty, not an error.
    assert!(parse_issue_labels(r#"{"labels": []}"#).unwrap().is_empty());
}

#[test]
fn parse_issue_state_is_true_only_when_closed() {
    // (case, `gh issue view --json state` output, expected "closed")
    let rows = [
        ("closed", r#"{"state":"CLOSED"}"#, true),
        ("open", r#"{"state":"OPEN"}"#, false),
    ];
    for (case, json, want) in rows {
        assert_eq!(parse_issue_state(json).unwrap(), want, "{case}");
    }
}

#[test]
fn parses_issue_with_labels() {
    let json = r#"{"number":7,"title":"t","body":"b","labels":[{"name":"AFK"},{"name":"bug"}]}"#;
    let issue = parse_issue(json).unwrap();
    assert_eq!(issue.number, 7);
    assert_eq!(issue.labels, vec!["AFK", "bug"]);
}

#[test]
fn tolerates_missing_body_and_labels() {
    let issue = parse_issue(r#"{"number":1,"title":"t"}"#).unwrap();
    assert_eq!(issue.body, "");
    assert!(issue.labels.is_empty());
}

fn issue(number: u64) -> Issue {
    Issue {
        number,
        title: format!("issue {number}"),
        body: String::new(),
        labels: vec![],
        comments: vec![],
    }
}

#[test]
fn build_queue_unions_dedupes_and_sorts() {
    // Two labels, one issue (#5) shared across both, out of order within each.
    let ready = vec![issue(9), issue(5)];
    let afk = vec![issue(5), issue(2)];
    let queue = build_queue(vec![ready, afk]);
    let numbers: Vec<u64> = queue.iter().map(|i| i.number).collect();
    assert_eq!(
        numbers,
        vec![2, 5, 9],
        "union, deduped by number, ascending"
    );
}

#[test]
fn build_queue_keeps_first_seen_for_duplicates() {
    // The shared issue's first occurrence wins, but identity (number) is what
    // matters — assert it appears exactly once regardless of batch order.
    let queue = build_queue(vec![vec![issue(3)], vec![issue(3)], vec![issue(1)]]);
    let numbers: Vec<u64> = queue.iter().map(|i| i.number).collect();
    assert_eq!(numbers, vec![1, 3]);
}

#[test]
fn parse_issue_list_reads_array() {
    let json = r#"[{"number":2,"title":"b","labels":[{"name":"AFK"}]},{"number":1,"title":"a"}]"#;
    let list = parse_issue_list(json).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].number, 2);
    assert_eq!(list[0].labels, vec!["AFK"]);
    assert_eq!(list[1].number, 1);
}

#[test]
fn queue_list_args_appends_assignee_only_when_present() {
    let with = queue_list_args("ready-for-agent", Some("@me"));
    let idx = with
        .iter()
        .position(|a| a == "--assignee")
        .expect("--assignee must be present when assignee is Some");
    assert_eq!(with.get(idx + 1).map(String::as_str), Some("@me"));

    let without = queue_list_args("ready-for-agent", None);
    assert!(
        !without.iter().any(|a| a == "--assignee"),
        "no --assignee token when assignee is None"
    );
}
