//! Branches of `sort_queue_in_graph` that the main tests do not reach.

use super::*;

fn issue(number: u64, body: &str) -> Issue {
    Issue {
        number,
        title: format!("issue {number}"),
        body: body.into(),
        labels: vec![],
        comments: vec![],
    }
}

fn numbers(queue: &[Issue]) -> Vec<u64> {
    queue.iter().map(|i| i.number).collect()
}

#[test]
fn a_queue_member_missing_from_open_keeps_its_own_body() {
    let q = vec![issue(5, "## Blocked by\n- #7\n"), issue(7, "")];
    let open = vec![issue(7, "")]; // #5 is in the queue but not in `open`
    assert_eq!(numbers(&sort_queue_in_graph(q, &open)), vec![7, 5]);
}

#[test]
fn an_issue_that_names_itself_as_blocker_is_not_blocked_by_itself() {
    let q = vec![
        issue(1, "## Blocked by\n- #2\n"),
        issue(2, "## Blocked by\n- #2\n"),
        issue(3, ""),
    ];
    assert_eq!(numbers(&sort_queue(q)), vec![2, 1, 3]);
}

#[test]
fn a_cycle_through_out_of_queue_nodes_still_reaches_every_blocker() {
    // #8 and #9 are open, outside the queue, and block each other. #10 is
    // outside the queue too and leads to queue member #3.
    let q = vec![issue(1, "## Blocked by\n- #10\n- #8\n"), issue(3, "")];
    let open = vec![
        issue(1, "## Blocked by\n- #10\n- #8\n"),
        issue(3, ""),
        issue(8, "## Blocked by\n- #9\n"),
        issue(9, "## Blocked by\n- #8\n"),
        issue(10, "## Blocked by\n- #3\n"),
    ];
    assert_eq!(numbers(&sort_queue_in_graph(q, &open)), vec![3, 1]);
}
