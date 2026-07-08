//! Parsing the `## Blocked by` and `## Parent` sections from an issue body,
//! and ordering a queue by those edges. Pure functions — no I/O, no `gh` calls.

use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;

use crate::Issue;

/// The stable machine marker on the single consolidated-spec comment `ralphy
/// triage` authors (ADR-0017). Its presence makes that comment the authoritative
/// spec (over the body), and its `## Blocked by` section gates the queue exactly
/// like one in the body. Re-triage finds this marker to edit its own comment
/// rather than stacking a second one.
pub const CONSOLIDATED_SPEC_MARKER: &str = "<!-- ralphy:consolidated-spec -->";

/// The stable machine marker on the single evidence-stamp comment a `promote`
/// verdict authors (ADR-0027). Unlike [`CONSOLIDATED_SPEC_MARKER`] it is NOT an
/// authoritative spec — it records the ADR-0018 evidence-gate citations that
/// justified admitting the issue to the queue, and its `## Blocked by` (if any)
/// is never parsed for gating. Re-triage finds this marker to edit its own
/// comment rather than stacking a second one.
pub const PROMOTE_EVIDENCE_MARKER: &str = "<!-- ralphy:promote-evidence -->";

/// The first comment in `comments` carrying [`CONSOLIDATED_SPEC_MARKER`] — the
/// authoritative consolidated spec `ralphy triage` posted (ADR-0017), or `None`
/// when no comment is marked. Unmarked comments (the author's body, ordinary
/// discussion) are never treated as a spec.
pub fn find_consolidated_spec(comments: &[String]) -> Option<&str> {
    comments
        .iter()
        .find(|c| c.contains(CONSOLIDATED_SPEC_MARKER))
        .map(String::as_str)
}

/// The union of `## Blocked by` refs from the body AND from the marked
/// consolidated-spec comment (ADR-0017): a dependency the triage agent captured
/// in the consolidation gates the queue exactly like one in the body. Deduped,
/// body refs first, order preserved. An unmarked comment contributes nothing.
pub fn parse_blocked_by_all(body: &str, comments: &[String]) -> Vec<u64> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut push = |refs: Vec<u64>| {
        for n in refs {
            if seen.insert(n) {
                out.push(n);
            }
        }
    };
    push(parse_blocked_by(body));
    if let Some(spec) = find_consolidated_spec(comments) {
        push(parse_blocked_by(spec));
    }
    out
}

/// Extract `#N` issue references from the `## Blocked by` section of an issue
/// body. Returns an empty list when the section is absent or contains no refs
/// (e.g. "None - can start immediately").
///
/// Stops at the next `##` heading so refs in unrelated sections are not collected.
pub fn parse_blocked_by(body: &str) -> Vec<u64> {
    let heading_re = Regex::new(r"(?im)^##\s+Blocked by\s*$").expect("valid regex");
    let section = crate::markdown::section_after_heading(body, &heading_re);
    if section.is_empty() {
        return Vec::new();
    }

    // Match only bullet-list items: `- #N` (leading whitespace optional).
    // This avoids treating prose references like "step #3" as issue refs.
    let ref_re = Regex::new(r"(?m)^\s*-\s*#(\d+)").expect("valid regex");
    ref_re
        .captures_iter(section)
        // `\d+` is unbounded, so an absurd digit run (issue bodies are
        // IO-controlled) can overflow u64; drop the impossible ref rather than
        // panic and take down the whole run.
        .filter_map(|c| c[1].parse::<u64>().ok())
        .collect()
}

/// Extract `#N` issue references from the `## Parent` section of an issue
/// body (the to-issues skill's provenance field, e.g. "Split from #3 (bundle
/// retired)"). Unlike `parse_blocked_by` this accepts prose refs, because the
/// template writes the parent inline, not as a bullet list. Returns an empty
/// list when the section is absent or carries no `#N` ref.
pub fn parse_parent(body: &str) -> Vec<u64> {
    let heading_re = Regex::new(r"(?im)^##\s+Parent\s*$").expect("valid regex");
    let section = crate::markdown::section_after_heading(body, &heading_re);
    if section.is_empty() {
        return Vec::new();
    }
    let ref_re = Regex::new(r"#(\d+)").expect("valid regex");
    ref_re
        .captures_iter(section)
        // See `parse_blocked_by`: an overflowing digit run must not panic.
        .filter_map(|c| c[1].parse::<u64>().ok())
        .collect()
}

/// Collect the issue numbers named in the two STRUCTURED reference sections —
/// `## Blocked by` and `## Parent` — deduped (first occurrence wins) and with
/// `self_number` removed. Blocked-by refs lead (the harder dependency), then the
/// parent. Prose `#N` mentions outside these sections are excluded —
/// `parse_blocked_by`'s bullet-only match and `parse_parent`'s section scoping
/// draw that line.
///
/// This is the strictly-structured set, kept for callers that want only the
/// load-bearing dependency/provenance edges (e.g. ordering evidence). The runner
/// pre-fetches the broader [`referenced_issues`] set into `.ralphy/references.md`.
pub fn structured_refs(body: &str, self_number: u64) -> Vec<u64> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for n in parse_blocked_by(body).into_iter().chain(parse_parent(body)) {
        if n != self_number && seen.insert(n) {
            out.push(n);
        }
    }
    out
}

/// Extract every `#N` issue reference appearing anywhere in `body`, in order of
/// first appearance, deduped. Unlike [`parse_blocked_by`]'s bullet-only rule this
/// is a whole-body scan: an inline `see #28 for the provisional corpus` mention
/// counts, because such a reference can be just as load-bearing as a structured
/// one — an inlined `#N` caveat was exactly what got laundered into a confident
/// claim before any structured section was involved.
///
/// The match mirrors GitHub's autolink boundary so non-references are not swept
/// in: `#` must start a line or follow a non-word char, and the digits must end
/// on a word boundary. That rejects hex colors (`#28a745`), letter anchors
/// (`#L42`), and `word#3`.
fn parse_body_refs(body: &str) -> Vec<u64> {
    let ref_re = Regex::new(r"(?:^|\W)#(\d+)\b").expect("valid regex");
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for c in ref_re.captures_iter(body) {
        // `\d+` is unbounded; an absurd digit run (bodies are IO-controlled) can
        // overflow u64 — drop the impossible ref rather than panic.
        if let Ok(n) = c[1].parse::<u64>() {
            if seen.insert(n) {
                out.push(n);
            }
        }
    }
    out
}

/// Collect every issue number referenced by `body` — the two STRUCTURED sections
/// (`## Blocked by`, then `## Parent`) first because they are the load-bearing
/// dependency/provenance links, then every other inline `#N` mention in order of
/// appearance — deduped (first occurrence wins) and with `self_number` removed.
///
/// This is what the runner pre-fetches into `.ralphy/references.md`: a planner
/// that would restate a `#N` as fact should read the source, and an inlined
/// mention is as apt to be restated as a structured one (the laundered-caveat
/// failure mode). Depth is still one — the fetched bodies' own refs aren't
/// followed.
pub fn referenced_issues(body: &str, self_number: u64) -> Vec<u64> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for n in parse_blocked_by(body)
        .into_iter()
        .chain(parse_parent(body))
        .chain(parse_body_refs(body))
    {
        if n != self_number && seen.insert(n) {
            out.push(n);
        }
    }
    out
}

/// Order a queue so every issue comes after the issues it depends on, with
/// ascending number as the tie-break — the sequence shown to the user IS the
/// sequence executed. Edges are derived from bodies already in hand (no tracker
/// calls):
///
/// - `## Blocked by` refs that point at another QUEUE member;
/// - for refs absent from the queue (e.g. a closed, retired bundle), the queue
///   members whose `## Parent` references that bundle stand in for it — the
///   dependent must come after the split's children.
///
/// Refs to issues that are neither in the queue nor split into queue members
/// impose no order here; the runner's blocked-by gate still owns correctness at
/// execution time. On a dependency cycle the un-orderable remainder is appended
/// in ascending order (the gate will skip what is truly blocked).
///
/// This is the context-free form: every out-of-queue blocker is treated as
/// satisfied. When the broader set of open issues is available, prefer
/// [`sort_queue_in_graph`], which follows transitive edges *through* issues
/// outside the queue.
pub fn sort_queue(queue: Vec<Issue>) -> Vec<Issue> {
    // With no broader context, the only issues that can carry edges are the queue
    // members themselves — pass the queue as its own context to reuse one code path.
    let context = queue.clone();
    sort_queue_in_graph(queue, &context)
}

/// Like [`sort_queue`], but order the queue within the *full* open-issue graph.
///
/// A blocker that sits outside the queue but is itself open (e.g. a chain of
/// spikes the operator labelled only partway) still constrains a queue member: we
/// walk the `## Blocked by` edges through those out-of-queue nodes — treating them
/// as transparent — until we reach another queue member, which becomes the real
/// predecessor. A blocker that is *closed* (absent from `open`) is pruned, exactly
/// as the runtime gate treats an already-satisfied dependency; if it was a retired
/// bundle, its `## Parent` children in the queue stand in for it.
///
/// `open` is the set of currently-open issues (queue members included). It is read
/// only for edge derivation; the returned order is a permutation of `queue`.
pub fn sort_queue_in_graph(queue: Vec<Issue>, open: &[Issue]) -> Vec<Issue> {
    let in_queue: BTreeSet<u64> = queue.iter().map(|i| i.number).collect();
    let open_numbers: BTreeSet<u64> = open.iter().map(|i| i.number).collect();
    // Body lookup spanning the open set and the queue (a queue member should
    // always be open, but fall back to its own body if `open` omits it).
    let mut body_of: BTreeMap<u64, &str> = BTreeMap::new();
    for i in open {
        body_of.insert(i.number, i.body.as_str());
    }
    for i in &queue {
        body_of.entry(i.number).or_insert(i.body.as_str());
    }
    // children_of[n] = queue members declaring `## Parent` #n — the stand-ins for
    // a retired (closed) bundle #n that is absent from the open graph.
    let mut children_of: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for i in &queue {
        for p in parse_parent(&i.body) {
            children_of.entry(p).or_default().push(i.number);
        }
    }

    // deps[x] = queue members that must precede x. For each queue member, walk the
    // blocked-by graph: an in-queue ref is a direct predecessor; an open
    // out-of-queue ref is transparent (recurse into its own blockers); a closed
    // ref substitutes its split children.
    let mut deps: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
    for i in &queue {
        let mut acc: BTreeSet<u64> = BTreeSet::new();
        let mut seen: BTreeSet<u64> = BTreeSet::new();
        let mut stack: Vec<u64> = vec![i.number];
        while let Some(node) = stack.pop() {
            if !seen.insert(node) {
                continue; // already expanded — also breaks blocked-by cycles
            }
            let body = match body_of.get(&node) {
                Some(b) => *b,
                None => continue,
            };
            for n in parse_blocked_by(body) {
                if n == i.number {
                    continue; // never depend on yourself
                }
                if in_queue.contains(&n) {
                    acc.insert(n); // terminal: a real in-queue predecessor
                } else if open_numbers.contains(&n) {
                    stack.push(n); // transparent: keep walking its blockers
                } else if let Some(children) = children_of.get(&n) {
                    // closed/absent ref that was split: its queue children stand in.
                    acc.extend(children.iter().copied().filter(|&c| c != i.number));
                }
            }
        }
        deps.insert(i.number, acc);
    }

    kahn(queue, &deps)
}

/// Kahn's algorithm over `deps` (the must-precede set per issue number), always
/// taking the smallest ready number first so ties break ascending. On a cycle the
/// un-orderable remainder is emitted in ascending order — the runtime gate still
/// owns correctness.
fn kahn(queue: Vec<Issue>, deps: &BTreeMap<u64, BTreeSet<u64>>) -> Vec<Issue> {
    let empty = BTreeSet::new();
    let mut by_number: BTreeMap<u64, Issue> = queue.into_iter().map(|i| (i.number, i)).collect();
    let mut placed: BTreeSet<u64> = BTreeSet::new();
    let mut out: Vec<Issue> = Vec::with_capacity(by_number.len());
    while !by_number.is_empty() {
        let ready = by_number.keys().copied().find(|n| {
            deps.get(n)
                .unwrap_or(&empty)
                .iter()
                .all(|d| placed.contains(d))
        });
        match ready {
            Some(n) => {
                placed.insert(n);
                out.push(by_number.remove(&n).expect("key present"));
            }
            None => {
                // Cycle: emit the remainder ascending; the runtime gate decides.
                out.extend(by_number.into_values());
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_multiple_refs() {
        let body = "## Blocked by\n- #3\n- #7\n";
        assert_eq!(parse_blocked_by(body), vec![3, 7]);
    }

    #[test]
    fn find_consolidated_spec_returns_marked_comment_only() {
        let comments = vec![
            "just a normal comment mentioning ## Blocked by\n- #1\n".to_string(),
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
        let comments = vec!["## Blocked by\n- #9\n".to_string()];
        // No marker → the comment's `## Blocked by` does not gate.
        assert!(parse_blocked_by_all(body, &comments).is_empty());
    }

    #[test]
    fn none_text_returns_empty() {
        let body = "## Blocked by\n\nNone - can start immediately\n";
        assert!(parse_blocked_by(body).is_empty());
    }

    #[test]
    fn absent_section_returns_empty() {
        let body = "## Steps\n- [ ] do something\n";
        assert!(parse_blocked_by(body).is_empty());
    }

    #[test]
    fn stops_at_next_heading() {
        let body = "## Blocked by\n- #3\n## Other\n- #9\n";
        assert_eq!(parse_blocked_by(body), vec![3]);
    }

    #[test]
    fn prose_refs_are_not_collected() {
        // "#3" appears in prose, not as a bullet item — must be ignored.
        let body = "## Blocked by\nStep #3 must finish before #7 merges\n- #7\n";
        assert_eq!(parse_blocked_by(body), vec![7]);
    }

    #[test]
    fn overflowing_ref_is_dropped_not_panicked() {
        // Issue bodies are IO-controlled; a digit run past u64::MAX must drop
        // the ref, never panic (regression for the parse().expect() crash).
        let body = "## Blocked by\n- #99999999999999999999999\n- #7\n";
        assert_eq!(parse_blocked_by(body), vec![7]);
        let parent = "## Parent\n\nSplit from #99999999999999999999999 and #3.\n";
        assert_eq!(parse_parent(parent), vec![3]);
    }

    #[test]
    fn parse_parent_reads_prose_ref() {
        let body = "## Parent\n\nSplit from #3 (bundle retired). Part of PRD-0002.\n\n## Blocked by\n- #16\n";
        assert_eq!(parse_parent(body), vec![3]);
    }

    #[test]
    fn parse_parent_absent_or_refless_is_empty() {
        assert!(parse_parent("## What to build\nstuff\n").is_empty());
        assert!(parse_parent("## Parent\n\nPart of PRD-0002 only.\n").is_empty());
    }

    #[test]
    fn parse_parent_stops_at_next_heading() {
        let body = "## Parent\n\nSplit from #3.\n\n## Blocked by\n- #16\n";
        assert_eq!(parse_parent(body), vec![3]);
    }

    #[test]
    fn structured_refs_unions_blocked_by_and_parent_deduped() {
        // #13 appears in both sections; it must surface once, blocked-by first.
        let body = "## Parent\n\nSplit from #15. See also #13.\n\n## Blocked by\n- #13\n- #7\n";
        assert_eq!(structured_refs(body, 29), vec![13, 7, 15]);
    }

    #[test]
    fn structured_refs_excludes_self_and_prose_mentions() {
        // A prose `#99` outside the structured sections is ignored; a self-ref
        // (#5 blocking itself, malformed) is dropped.
        let body = "Background mentions #99.\n\n## Blocked by\n- #5\n- #7\n";
        assert_eq!(structured_refs(body, 5), vec![7]);
    }

    #[test]
    fn structured_refs_empty_without_sections() {
        assert!(structured_refs("## What to build\nstuff with #3 inline\n", 1).is_empty());
    }

    #[test]
    fn referenced_issues_includes_inline_body_refs_after_structured() {
        // #13/#7 structured (blocked-by), #15 structured (parent); #28 only inline
        // in prose. All four surface, structured first, inline last, deduped.
        let body = "Uses the provisional corpus from #28.\n\n\
            ## Parent\n\nSplit from #15.\n\n\
            ## Blocked by\n- #13\n- #7\n";
        assert_eq!(referenced_issues(body, 29), vec![13, 7, 15, 28]);
    }

    #[test]
    fn referenced_issues_dedupes_inline_against_structured() {
        // #13 appears both as a blocked-by bullet and inline in prose — once only.
        let body = "Background references #13 throughout.\n\n## Blocked by\n- #13\n";
        assert_eq!(referenced_issues(body, 1), vec![13]);
    }

    #[test]
    fn referenced_issues_excludes_self_and_non_refs() {
        // Self (#5) dropped; the hex color and letter-anchor are not refs; the
        // genuine inline #7 survives.
        let body = "Self ref #5. Color #28a745, anchor #L42, but see #7.";
        assert_eq!(referenced_issues(body, 5), vec![7]);
    }

    #[test]
    fn referenced_issues_finds_refs_with_no_structured_sections() {
        // A thin body that only names a blocker in prose still yields the ref —
        // the exact case the structured-only set missed.
        assert_eq!(
            referenced_issues("Blocked by #3 until the schema lands.", 1),
            vec![3]
        );
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
}
