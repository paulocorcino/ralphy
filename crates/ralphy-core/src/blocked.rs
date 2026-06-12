//! Parsing the `## Blocked by` and `## Parent` sections from an issue body,
//! and ordering a queue by those edges. Pure functions — no I/O, no `gh` calls.

use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;

use crate::Issue;

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
        .map(|c| c[1].parse::<u64>().expect("matched digits"))
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
        .map(|c| c[1].parse::<u64>().expect("matched digits"))
        .collect()
}

/// Order a queue so every issue comes after the issues it depends on, with
/// ascending number as the tie-break — the sequence shown to the user IS the
/// sequence executed. Two kinds of edges, both derived from bodies already in
/// hand (no tracker calls):
///
/// - `## Blocked by` refs that point at another QUEUE member;
/// - for refs absent from the queue (e.g. a closed, retired bundle), the queue
///   members whose `## Parent` references that bundle stand in for it — the
///   dependent must come after the split's children.
///
/// Refs to issues that are neither in the queue nor split into queue members
/// impose no order here; the runner's blocked-by gate still owns correctness
/// at execution time. On a dependency cycle the un-orderable remainder is
/// appended in ascending order (the gate will skip what is truly blocked).
pub fn sort_queue(queue: Vec<Issue>) -> Vec<Issue> {
    let numbers: BTreeSet<u64> = queue.iter().map(|i| i.number).collect();
    // children_of[n] = queue members declaring `## Parent` #n.
    let mut children_of: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for i in &queue {
        for p in parse_parent(&i.body) {
            children_of.entry(p).or_default().push(i.number);
        }
    }
    // deps[x] = queue members that must precede x.
    let mut deps: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
    for i in &queue {
        let entry = deps.entry(i.number).or_default();
        for n in parse_blocked_by(&i.body) {
            if numbers.contains(&n) {
                entry.insert(n);
            } else if let Some(children) = children_of.get(&n) {
                entry.extend(children.iter().copied().filter(|&c| c != i.number));
            }
        }
    }
    // Kahn's algorithm, always taking the smallest ready number first.
    let mut by_number: BTreeMap<u64, Issue> = queue.into_iter().map(|i| (i.number, i)).collect();
    let mut placed: BTreeSet<u64> = BTreeSet::new();
    let mut out: Vec<Issue> = Vec::with_capacity(by_number.len());
    while !by_number.is_empty() {
        let ready = by_number
            .keys()
            .copied()
            .find(|n| deps[n].iter().all(|d| placed.contains(d)));
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

    fn issue(number: u64, body: &str) -> Issue {
        Issue {
            number,
            title: format!("issue {number}"),
            body: body.into(),
            labels: vec![],
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
}
