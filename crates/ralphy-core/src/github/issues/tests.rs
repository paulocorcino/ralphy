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
    assert_eq!(
        parse_issue_url("https://github.com/owner/repo/issues/42").unwrap(),
        42
    );
    // Tolerates whitespace, a trailing slash, and a preamble line.
    assert_eq!(
        parse_issue_url("Creating issue...\nhttps://github.com/o/r/issues/7/\n").unwrap(),
        7
    );
}

#[test]
fn parse_issue_url_errors_without_a_number() {
    assert!(parse_issue_url("no url here").is_err());
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
fn parse_issue_state_closed() {
    assert!(parse_issue_state(r#"{"state":"CLOSED"}"#).unwrap());
}

#[test]
fn parse_issue_state_open() {
    assert!(!parse_issue_state(r#"{"state":"OPEN"}"#).unwrap());
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
fn from_ghissue_maps_all_fields() {
    let g = GhIssue {
        number: 42,
        title: "some title".into(),
        body: "some body".into(),
        labels: vec![
            GhLabel { name: "AFK".into() },
            GhLabel { name: "bug".into() },
        ],
    };
    let issue = Issue::from(g);
    assert_eq!(issue.number, 42);
    assert_eq!(issue.title, "some title");
    assert_eq!(issue.body, "some body");
    assert_eq!(issue.labels, vec!["AFK", "bug"]);
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
fn parse_issue_meta_list_reads_assignees_and_state_reason() {
    let json = r#"[{"number":7,"assignees":[{"login":"alice"},{"login":"bob"}],"stateReason":null},{"number":8,"assignees":[],"stateReason":"COMPLETED"}]"#;
    let meta = parse_issue_meta_list(json).unwrap();
    assert_eq!(
        meta[0].assignees,
        vec!["alice".to_string(), "bob".to_string()]
    );
    assert_eq!(meta[0].state_reason, None);
    assert_eq!(meta[1].state_reason.as_deref(), Some("COMPLETED"));
}

#[test]
fn parse_all_open_meta_reads_dates_and_assignees() {
    // The open board read: dates + assignees + a body-derived blocked_by,
    // state stamped "open", reason absent (gh leaves stateReason null on open).
    let json = r###"[{"number":7,"title":"t7","labels":[{"name":"ready-for-agent"}],"assignees":[{"login":"octo"}],"createdAt":"2026-07-01T08:00:00Z","updatedAt":"2026-07-02T09:00:00Z","body":"## Blocked by\n- #3\n- #4\n"}]"###;
    let rows = parse_all_open_meta(json).unwrap();
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.number, 7);
    assert_eq!(r.title, "t7");
    assert_eq!(r.state, "open");
    assert_eq!(r.reason, None);
    assert_eq!(r.labels, vec!["ready-for-agent".to_string()]);
    assert_eq!(r.assignees, vec!["octo".to_string()]);
    assert_eq!(r.blocked_by, vec![3, 4]);
    assert_eq!(r.created, "2026-07-01T08:00:00Z");
    assert_eq!(r.updated, "2026-07-02T09:00:00Z");
}

#[test]
fn parse_closed_board_lowercases_state_reason() {
    // The closed board read: stateReason lowercased into reason, state
    // stamped "closed", no body → empty blocked_by.
    let json = r#"[{"number":8,"title":"done","labels":[],"assignees":[],"stateReason":"COMPLETED","createdAt":"2026-06-01T08:00:00Z","updatedAt":"2026-06-02T09:00:00Z"},{"number":9,"title":"nope","labels":[],"assignees":[],"stateReason":"NOT_PLANNED","createdAt":"2026-06-03T08:00:00Z","updatedAt":"2026-06-04T09:00:00Z"}]"#;
    let rows = parse_closed_board(json).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].state, "closed");
    assert_eq!(rows[0].reason.as_deref(), Some("completed"));
    assert!(rows[0].blocked_by.is_empty());
    assert_eq!(rows[1].reason.as_deref(), Some("not_planned"));
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
