//! Tests of the whole-board fold.

use super::*;

#[test]
fn render_board_json_folds_whole_tracker_with_union_and_graph_order() {
    use ralphy_core::blocked::sort_queue_in_graph;
    use ralphy_core::github::BoardIssue;

    // Board row factory: a whole-tracker BoardIssue with the given state,
    // assignees, blocked_by, and (closed) reason.
    let bi =
        |number: u64, state: &str, assignees: &[&str], blocked_by: &[u64], reason: Option<&str>| {
            BoardIssue {
                number,
                title: format!("issue {number}"),
                state: state.to_string(),
                reason: reason.map(str::to_string),
                labels: vec!["ready-for-agent".to_string()],
                assignees: assignees.iter().map(ToString::to_string).collect(),
                blocked_by: blocked_by.to_vec(),
                created: "2026-07-01T00:00:00Z".to_string(),
                updated: "2026-07-02T00:00:00Z".to_string(),
            }
        };

    // A ready chain (#7 blocked by #8) + a ready issue (#50) DELIBERATELY absent
    // from the open read below (simulating a queue-labeled issue older than the
    // open read's limit) → the graph order the fold must preserve, with #50 kept
    // via a degraded synthesized row (M1 fix).
    let ready_q = vec![
        issue(7, &[], "## Blocked by\n- #8\n"),
        issue(8, &[], ""),
        issue(50, &[], ""),
    ];
    let ready_sorted = sort_queue_in_graph(ready_q.clone(), &ready_q);
    let ready_order: Vec<u64> = ready_sorted.iter().map(|i| i.number).collect();
    assert_eq!(
        ready_order,
        vec![8, 7, 50],
        "#8 (root) precedes #7, then #50"
    );

    // Whole open tracker: the two ready issues + a backlog issue + one assigned to
    // someone-else + one assigned to `octo`. #50 is NOT here (proves the fallback).
    // One recent-closed row.
    let open = vec![
        bi(7, "open", &[], &[8], None),
        bi(8, "open", &[], &[], None),
        bi(30, "open", &[], &[], None),
        bi(31, "open", &["someone-else"], &[], None),
        bi(32, "open", &["octo"], &[], None),
    ];
    let closed = vec![bi(99, "closed", &[], &[], Some("not_planned"))];
    let repo_labels = vec![("ready-for-agent".to_string(), "0e8a16".to_string())];

    let nums = |val: &Value| -> Vec<u64> {
        val["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["number"].as_u64().unwrap())
            .collect()
    };

    // (a)+(b) default (login=None): Ready subset first in graph order (incl. the
    // synthesized #50), then the remaining UNASSIGNED open (#30), then closed
    // (#99). The someone-else (#31) and octo (#32) rows are dropped — only
    // empty-assignees rows survive.
    let json = render_board_json(&ready_sorted, &open, &closed, None, &repo_labels).unwrap();
    let val: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        nums(&val),
        vec![8, 7, 50, 30, 99],
        "default hides assigned; #50 kept via degraded row: {val}"
    );

    // (d) the closed row carries state + lowercased reason.
    let closed_row = val["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["number"] == 99)
        .unwrap();
    assert_eq!(closed_row["state"], "closed");
    assert_eq!(closed_row["reason"], "not_planned");

    // (e) labels[] carries the repo {name,color} vocabulary.
    assert!(
        val["labels"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!({"name":"ready-for-agent","color":"0e8a16"})),
        "labels must carry the repo vocabulary with colors: {val}"
    );

    // (c) a configured login keeps unassigned OR that-login (#32 survives), while
    // an issue assigned only to someone-else (#31) stays dropped.
    let json2 =
        render_board_json(&ready_sorted, &open, &closed, Some("octo"), &repo_labels).unwrap();
    let val2: Value = serde_json::from_str(&json2).unwrap();
    let n2 = nums(&val2);
    assert!(
        n2.contains(&32),
        "octo-assigned survives under login: {val2}"
    );
    assert!(!n2.contains(&31), "someone-else stays dropped: {val2}");
    assert_eq!(n2, vec![8, 7, 50, 30, 32, 99]);

    // The shared reply shows every field the board folds, so one row must
    // carry assignees and one must carry blocked_by.
    let rows = val2["issues"].as_array().unwrap();
    assert!(rows.iter().any(|r| r["assignees"] != serde_json::json!([])));
    assert!(rows
        .iter()
        .any(|r| r["blocked_by"] != serde_json::json!([])));
    crate::golden::check(
        "board.list",
        serde_json::json!({ "status": "ok", "board": val2 }),
        &[],
    );
}
