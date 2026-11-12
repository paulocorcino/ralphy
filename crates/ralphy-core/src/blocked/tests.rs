use super::*;
use std::fmt::Write as _;

#[test]
fn parse_blocked_by_reads_the_bullet_refs_of_its_section() {
    // (case, body, expected refs)
    let rows: [(&str, &str, &[u64]); 6] = [
        ("multiple refs", "## Blocked by\n- #3\n- #7\n", &[3, 7]),
        (
            "none text",
            "## Blocked by\n\nNone - can start immediately\n",
            &[],
        ),
        ("absent section", "## Steps\n- [ ] do something\n", &[]),
        (
            "stops at the next heading",
            "## Blocked by\n- #3\n## Other\n- #9\n",
            &[3],
        ),
        (
            // "#3" appears in prose, not as a bullet item.
            "prose refs are not collected",
            "## Blocked by\nStep #3 must finish before #7 merges\n- #7\n",
            &[7],
        ),
        (
            // Issue bodies are IO-controlled; a digit run past u64::MAX must
            // drop the ref, never panic (regression for the
            // parse().expect() crash).
            "overflowing ref is dropped",
            "## Blocked by\n- #99999999999999999999999\n- #7\n",
            &[7],
        ),
    ];
    for (case, body, want) in rows {
        assert_eq!(parse_blocked_by(body), want, "{case}");
    }
}

#[test]
fn find_consolidated_spec_returns_marked_comment_only() {
    let comments = vec![
        "just a normal comment mentioning ## Blocked by\n- #1\n".to_owned(),
        format!("{CONSOLIDATED_SPEC_MARKER}\n## Consolidated spec\nbody\n"),
    ];
    let spec = find_consolidated_spec(&comments).expect("marked comment found");
    assert!(spec.contains("Consolidated spec"));
    // An unmarked comment alone yields nothing.
    assert!(find_consolidated_spec(&comments[..1]).is_none());
}

#[test]
fn parse_blocked_by_all_unions_body_and_marked_comment() {
    let body = "## Blocked by\n- #3\n";
    let comments = vec![format!(
        "{CONSOLIDATED_SPEC_MARKER}\n## Blocked by\n- #4\n- #3\n"
    )];
    // Body ref #3 leads; the marked comment adds #4; #3 is not duplicated.
    assert_eq!(parse_blocked_by_all(body, &comments), vec![3, 4]);
}

#[test]
fn blocked_by_in_unmarked_comment_is_ignored() {
    let body = "## Steps\n- [ ] do it\n";
    let comments = vec!["## Blocked by\n- #9\n".to_owned()];
    // No marker → the comment's `## Blocked by` does not gate.
    assert_eq!(parse_blocked_by_all(body, &comments), Vec::<u64>::new());
}

#[test]
fn parse_parent_reads_the_first_ref_of_its_section() {
    // (case, body, expected refs)
    let rows: [(&str, &str, &[u64]); 6] = [
        (
            "prose ref",
            "## Parent\n\nSplit from #3 (bundle retired). Part of PRD-0002.\n\n\
             ## Blocked by\n- #16\n",
            &[3],
        ),
        (
            // The real #300 body: the parent is #296; the sibling refs that
            // follow are prose. Reading them as parents made #300 a child of
            // #299 — and once #299 closed, the follow-the-split gate handed
            // #300 back to itself as a blocker.
            "only the first ref",
            "## Parent\n\n#296\n\nSupersedes #210 together with #299.\n\n## What to build\n",
            &[296],
        ),
        ("absent section", "## What to build\nstuff\n", &[]),
        (
            "refless section",
            "## Parent\n\nPart of PRD-0002 only.\n",
            &[],
        ),
        (
            "stops at the next heading",
            "## Parent\n\nSplit from #3.\n\n## Blocked by\n- #16\n",
            &[3],
        ),
        (
            "overflowing ref is dropped",
            "## Parent\n\nSplit from #99999999999999999999999 and #3.\n",
            &[3],
        ),
    ];
    for (case, body, want) in rows {
        assert_eq!(parse_parent(body), want, "{case}");
    }
}

#[test]
fn structured_refs_unions_blocked_by_and_parent() {
    // (case, body, the issue's own number, expected refs)
    let rows: [(&str, &str, u64, &[u64]); 3] = [
        (
            // #13 appears in both sections; it surfaces once, blocked-by first.
            "union, deduped",
            "## Parent\n\nSplit from #15. See also #13.\n\n## Blocked by\n- #13\n- #7\n",
            29,
            &[13, 7, 15],
        ),
        (
            // A prose `#99` outside the structured sections is ignored; a
            // self-ref (#5 blocking itself, malformed) is dropped.
            "excludes self and prose mentions",
            "Background mentions #99.\n\n## Blocked by\n- #5\n- #7\n",
            5,
            &[7],
        ),
        (
            "empty without sections",
            "## What to build\nstuff with #3 inline\n",
            1,
            &[],
        ),
    ];
    for (case, body, own, want) in rows {
        assert_eq!(structured_refs(body, own), want, "{case}");
    }
}

#[test]
fn referenced_issues_adds_inline_refs_after_structured() {
    // (case, body, the issue's own number, expected refs)
    let rows: [(&str, &str, u64, &[u64]); 4] = [
        (
            // #13/#7 structured (blocked-by), #15 structured (parent); #28 only
            // inline in prose. All four surface, structured first, inline
            // last, deduped.
            "inline after structured",
            "Uses the provisional corpus from #28.\n\n\
             ## Parent\n\nSplit from #15.\n\n\
             ## Blocked by\n- #13\n- #7\n",
            29,
            &[13, 7, 15, 28],
        ),
        (
            // #13 appears both as a blocked-by bullet and inline in prose.
            "inline deduped against structured",
            "Background references #13 throughout.\n\n## Blocked by\n- #13\n",
            1,
            &[13],
        ),
        (
            // Self (#5) dropped; the hex color and letter-anchor are not refs;
            // the genuine inline #7 survives.
            "excludes self and non-refs",
            "Self ref #5. Color #28a745, anchor #L42, but see #7.",
            5,
            &[7],
        ),
        (
            // A thin body that only names a blocker in prose still yields the
            // ref — the exact case the structured-only set missed.
            "no structured sections",
            "Blocked by #3 until the schema lands.",
            1,
            &[3],
        ),
    ];
    for (case, body, own, want) in rows {
        assert_eq!(referenced_issues(body, own), want, "{case}");
    }
}

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
fn assignee_union_default_hides_assigned() {
    // No configured login ⇒ only unassigned issues survive.
    assert!(assignee_union_keep(&[], None));
    assert!(!assignee_union_keep(&["octo".to_owned()], None));
}

#[test]
fn assignee_union_keeps_unassigned_or_login() {
    // Configured login `octo`: unassigned survives, an `octo`-assigned issue
    // survives, anything assigned only to others is dropped.
    assert!(assignee_union_keep(&[], Some("octo")));
    assert!(assignee_union_keep(
        &["octo".to_owned(), "other".to_owned()],
        Some("octo")
    ));
    assert!(!assignee_union_keep(&["other".to_owned()], Some("octo")));
}

#[test]
fn board_order_matches_shared_fixture() {
    // The machine-verified parity spine (issue #198): the board fold's Ready
    // order MUST equal the core's queue order. Load the shared fixture the JS
    // `orderGraph` port cross-checks against, derive the queue as the issues
    // named in `expectedOrder` (open = all `issues`), run the same sort, and
    // assert the numbers equal `expectedOrder` verbatim. Diverging the fixture
    // or the sort reds this test.
    #[derive(serde::Deserialize)]
    struct FixtureIssue {
        number: u64,
        #[serde(default, rename = "blockedBy")]
        blocked_by: Vec<u64>,
    }
    #[derive(serde::Deserialize)]
    struct Fixture {
        issues: Vec<FixtureIssue>,
        #[serde(rename = "expectedOrder")]
        expected_order: Vec<u64>,
    }

    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/graph-parity.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let fixture: Fixture = serde_json::from_str(&raw).expect("valid parity fixture JSON");

    // Synthesize each body as a `## Blocked by` bullet list so `parse_blocked_by`
    // reconstructs the edges the fixture declares.
    let body_of = |blocked_by: &[u64]| -> String {
        let mut b = String::from("## Blocked by\n");
        for n in blocked_by {
            writeln!(b, "- #{n}").expect(crate::WRITE_TO_STRING);
        }
        b
    };
    let open: Vec<Issue> = fixture
        .issues
        .iter()
        .map(|fi| issue(fi.number, &body_of(&fi.blocked_by)))
        .collect();
    let queue: Vec<Issue> = open
        .iter()
        .filter(|i| fixture.expected_order.contains(&i.number))
        .cloned()
        .collect();

    assert_eq!(
        numbers(&sort_queue_in_graph(queue, &open)),
        fixture.expected_order,
        "board fold order must equal the core queue order (fixture parity)"
    );
}

#[test]
fn sort_queue_no_edges_keeps_ascending() {
    let q = vec![issue(9, ""), issue(2, ""), issue(5, "")];
    assert_eq!(numbers(&sort_queue(q)), vec![2, 5, 9]);
}

#[test]
fn sort_queue_orders_split_children_before_dependent_epics() {
    // The OCS shape: epics #4..#10 chained, #4 blocked by retired bundle #3
    // (absent from the queue) whose children #16..#21 are queue members.
    let q = vec![
        issue(4, "## Blocked by\n- #2\n- #3\n"),
        issue(5, "## Blocked by\n- #4\n- #3\n"),
        issue(6, "## Blocked by\n- #5\n- #2\n"),
        issue(7, "## Blocked by\n- #6\n"),
        issue(8, "## Blocked by\n- #7\n"),
        issue(9, "## Blocked by\n- #8\n"),
        issue(10, "## Blocked by\n- #9\n"),
        issue(16, "## Parent\n\nSplit from #3.\n"),
        issue(17, "## Parent\n\nSplit from #3.\n\n## Blocked by\n- #16\n"),
        issue(
            18,
            "## Parent\n\nSplit from #3.\n\n## Blocked by\n- #16\n- #17\n",
        ),
        issue(19, "## Parent\n\nSplit from #3.\n\n## Blocked by\n- #18\n"),
        issue(20, "## Parent\n\nSplit from #3.\n\n## Blocked by\n- #18\n"),
        issue(
            21,
            "## Parent\n\nSplit from #3.\n\n## Blocked by\n- #17\n- #18\n",
        ),
    ];
    assert_eq!(
        numbers(&sort_queue(q)),
        vec![16, 17, 18, 19, 20, 21, 4, 5, 6, 7, 8, 9, 10]
    );
}

#[test]
fn sort_queue_refs_outside_queue_impose_no_order() {
    // #5 blocked by closed #2 with no children in the queue → stays put.
    let q = vec![issue(5, "## Blocked by\n- #2\n"), issue(7, "")];
    assert_eq!(numbers(&sort_queue(q)), vec![5, 7]);
}

#[test]
fn sort_queue_cycle_falls_back_to_ascending() {
    let q = vec![
        issue(1, "## Blocked by\n- #2\n"),
        issue(2, "## Blocked by\n- #1\n"),
        issue(3, ""),
    ];
    // #3 is orderable; the 1↔2 cycle is appended ascending.
    assert_eq!(numbers(&sort_queue(q)), vec![3, 1, 2]);
}

#[test]
fn in_graph_orders_through_open_out_of_queue_blocker() {
    // #5 is blocked by open #8 (NOT in the queue), which is itself blocked by
    // in-queue #7. Context-free `sort_queue` sees #5 as a root and floats it to
    // the front; the graph form follows #5 → #8 → #7 and orders #5 after #7.
    let q = vec![issue(5, "## Blocked by\n- #8\n"), issue(7, "")];
    let open = vec![
        issue(5, "## Blocked by\n- #8\n"),
        issue(7, ""),
        issue(8, "## Blocked by\n- #7\n"),
    ];
    // Without context: #5 looks unblocked and sorts first.
    assert_eq!(numbers(&sort_queue(q.clone())), vec![5, 7]);
    // With the open graph: #5 lands after its true predecessor #7.
    assert_eq!(numbers(&sort_queue_in_graph(q, &open)), vec![7, 5]);
}

#[test]
fn in_graph_prunes_closed_out_of_queue_blocker() {
    // #5 is blocked by #8, but #8 is closed (absent from `open`). The edge is
    // pruned — #5 is genuinely unblocked and sorts ascending. Mirrors the
    // runtime gate, which skips nothing for an already-closed blocker.
    let q = vec![issue(5, "## Blocked by\n- #8\n"), issue(7, "")];
    let open = vec![issue(5, "## Blocked by\n- #8\n"), issue(7, "")]; // #8 not open
    assert_eq!(numbers(&sort_queue_in_graph(q, &open)), vec![5, 7]);
}

#[test]
fn in_graph_resolves_the_bioledger_chain() {
    // The real bioledger queue {5,7,8,9,10,14,15,16,17,18,19,20,21}: the chain
    // is severed at out-of-queue nodes (#5→#26→…→#11→#19; #15→#13→#10), so the
    // context-free order floats #5 and #15 to the front. The graph form follows
    // the transitive edges and produces the single true chain.
    let queue_bodies: &[(u64, &str)] = &[
        (5, "## Blocked by\n- #26\n"),
        (7, "## Blocked by\n- #14\n"),
        (8, "## Blocked by\n- #7\n"),
        (9, "## Blocked by\n- #8\n"),
        (10, "## Blocked by\n- #9\n"),
        (14, "## Blocked by\n- #20\n"),
        (15, "## Blocked by\n- #13\n"),
        (16, "## Blocked by\n- #15\n"),
        (17, "## Blocked by\n- #16\n"),
        (18, "## Blocked by\n- #17\n"),
        (19, "## Blocked by\n- #18\n"),
        (20, "## Blocked by\n- #21\n"),
        (21, ""),
    ];
    // The open issues outside the queue that bridge the chain.
    let bridge_bodies: &[(u64, &str)] = &[
        (6, "## Blocked by\n- #12\n"),
        (11, "## Blocked by\n- #19\n"),
        (12, "## Blocked by\n- #25\n"),
        (13, "## Blocked by\n- #10\n"),
        (22, "## Blocked by\n- #11\n"),
        (23, "## Blocked by\n- #22\n"),
        (24, "## Blocked by\n- #23\n"),
        (25, "## Blocked by\n- #24\n"),
        (26, "## Blocked by\n- #6\n"),
    ];
    let q: Vec<Issue> = queue_bodies.iter().map(|(n, b)| issue(*n, b)).collect();
    let open: Vec<Issue> = queue_bodies
        .iter()
        .chain(bridge_bodies)
        .map(|(n, b)| issue(*n, b))
        .collect();

    // Context-free: the severed chain floats #5 and #15.. to the front.
    assert_eq!(
        numbers(&sort_queue(q.clone())),
        vec![5, 15, 16, 17, 18, 19, 21, 20, 14, 7, 8, 9, 10]
    );
    // Graph-aware: one clean chain, #21 first and #5 last.
    assert_eq!(
        numbers(&sort_queue_in_graph(q, &open)),
        vec![21, 20, 14, 7, 8, 9, 10, 15, 16, 17, 18, 19, 5]
    );
}
