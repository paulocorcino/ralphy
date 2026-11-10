use super::*;

#[test]
fn parse_issue_meta_list_reads_assignees_and_state_reason() {
    let json = concat!(
        r#"[{"number":7,"assignees":[{"login":"alice"},{"login":"bob"}],"stateReason":null},"#,
        r#"{"number":8,"assignees":[],"stateReason":"COMPLETED"}]"#,
    );
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
    let json = concat!(
        r#"[{"number":7,"title":"t7","labels":[{"name":"ready-for-agent"}],"#,
        r#""assignees":[{"login":"octo"}],"createdAt":"2026-07-01T08:00:00Z","#,
        r###""updatedAt":"2026-07-02T09:00:00Z","body":"## Blocked by\n- #3\n- #4\n"}]"###,
    );
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
    let json = concat!(
        r#"[{"number":8,"title":"done","labels":[],"assignees":[],"stateReason":"COMPLETED","#,
        r#""createdAt":"2026-06-01T08:00:00Z","updatedAt":"2026-06-02T09:00:00Z"},"#,
        r#"{"number":9,"title":"nope","labels":[],"assignees":[],"stateReason":"NOT_PLANNED","#,
        r#""createdAt":"2026-06-03T08:00:00Z","updatedAt":"2026-06-04T09:00:00Z"}]"#,
    );
    let rows = parse_closed_board(json).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].state, "closed");
    assert_eq!(rows[0].reason.as_deref(), Some("completed"));
    assert_eq!(rows[0].blocked_by, Vec::<u64>::new());
    assert_eq!(rows[1].reason.as_deref(), Some("not_planned"));
}
