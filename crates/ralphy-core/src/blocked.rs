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

/// Extract the parent `#N` reference from the `## Parent` section of an issue
/// body (the to-issues skill's provenance field, e.g. "Split from #3 (bundle
/// retired)"). Unlike `parse_blocked_by` this accepts a prose ref, because the
/// template writes the parent inline, not as a bullet list. Returns an empty
/// list when the section is absent or carries no `#N` ref.
///
/// The template declares exactly ONE parent, so only the FIRST ref counts: the
/// same section routinely carries sibling prose ("Supersedes #210 together with
/// #299"), and treating those as parents makes the issue a child of its own
/// siblings — which the follow-the-split blocker gate then reads as a real
/// dependency edge, up to blocking the issue on itself. Kept as a `Vec` so the
/// public shape (and every caller) is unchanged.
pub fn parse_parent(body: &str) -> Vec<u64> {
    let heading_re = Regex::new(r"(?im)^##\s+Parent\s*$").expect("valid regex");
    let section = crate::markdown::section_after_heading(body, &heading_re);
    if section.is_empty() {
        return Vec::new();
    }
    let ref_re = Regex::new(r"#(\d+)").expect("valid regex");
    ref_re
        .captures_iter(section)
        // See `parse_blocked_by`: an overflowing digit run must not panic — it is
        // dropped, so the first PARSEABLE ref is the parent.
        .filter_map(|c| c[1].parse::<u64>().ok())
        .take(1)
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

/// The board fold's assignee-union scope (ADR-0036 Implementation Decisions):
/// keep an issue iff it is UNASSIGNED, or a `queue.assignee` login is configured
/// AND that login is among the issue's assignees. `gh --assignee` cannot express
/// "unassigned OR login" (it excludes unassigned), so the union is applied here,
/// over an UNFILTERED read. With no configured login the default is unassigned-only.
pub fn assignee_union_keep(assignees: &[String], configured: Option<&str>) -> bool {
    assignees.is_empty() || configured.is_some_and(|c| assignees.iter().any(|a| a == c))
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
    let body_of = body_lookup(open, &queue);
    let children_of = children_by_parent(&queue);

    // deps[x] = queue members that must precede x.
    let mut deps: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
    for i in &queue {
        let graph = Graph {
            body_of: &body_of,
            in_queue: &in_queue,
            open_numbers: &open_numbers,
            children_of: &children_of,
        };
        deps.insert(i.number, queue_predecessors(i.number, &graph));
    }

    kahn(queue, &deps)
}

/// The blocked-by graph that [`queue_predecessors`] walks.
struct Graph<'a> {
    body_of: &'a BTreeMap<u64, &'a str>,
    in_queue: &'a BTreeSet<u64>,
    open_numbers: &'a BTreeSet<u64>,
    children_of: &'a BTreeMap<u64, Vec<u64>>,
}

/// Body lookup spanning the open set and the queue (a queue member should
/// always be open, but fall back to its own body if `open` omits it).
fn body_lookup<'a>(open: &'a [Issue], queue: &'a [Issue]) -> BTreeMap<u64, &'a str> {
    let mut body_of: BTreeMap<u64, &str> = BTreeMap::new();
    for i in open {
        body_of.insert(i.number, i.body.as_str());
    }
    for i in queue {
        body_of.entry(i.number).or_insert(i.body.as_str());
    }
    body_of
}

/// children_of[n] = queue members declaring `## Parent` #n — the stand-ins for
/// a retired (closed) bundle #n that is absent from the open graph.
fn children_by_parent(queue: &[Issue]) -> BTreeMap<u64, Vec<u64>> {
    let mut children_of: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for i in queue {
        for p in parse_parent(&i.body) {
            children_of.entry(p).or_default().push(i.number);
        }
    }
    children_of
}

/// The queue members that must precede `root`. Walk the blocked-by graph from
/// `root`: an in-queue ref is a direct predecessor; an open out-of-queue ref is
/// transparent (recurse into its own blockers); a closed ref substitutes its
/// split children.
fn queue_predecessors(root: u64, graph: &Graph) -> BTreeSet<u64> {
    let mut acc: BTreeSet<u64> = BTreeSet::new();
    let mut seen: BTreeSet<u64> = BTreeSet::new();
    let mut stack: Vec<u64> = vec![root];
    while let Some(node) = stack.pop() {
        if !seen.insert(node) {
            continue; // already expanded — also breaks blocked-by cycles
        }
        let Some(body) = graph.body_of.get(&node) else {
            continue;
        };
        for n in parse_blocked_by(body) {
            if n != root {
                // never depend on yourself
                follow_blocker(n, root, graph, &mut acc, &mut stack);
            }
        }
    }
    acc
}

/// One blocker `n` of the walk from `root`: terminal when in the queue,
/// transparent when open, replaced by its split children when closed.
fn follow_blocker(n: u64, root: u64, graph: &Graph, acc: &mut BTreeSet<u64>, stack: &mut Vec<u64>) {
    if graph.in_queue.contains(&n) {
        acc.insert(n); // terminal: a real in-queue predecessor
    } else if graph.open_numbers.contains(&n) {
        stack.push(n); // transparent: keep walking its blockers
    } else if let Some(children) = graph.children_of.get(&n) {
        // closed/absent ref that was split: its queue children stand in.
        acc.extend(children.iter().copied().filter(|&c| c != root));
    }
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
        if let Some(n) = ready {
            placed.insert(n);
            out.push(by_number.remove(&n).expect("key present"));
        } else {
            // Cycle: emit the remainder ascending; the runtime gate decides.
            out.extend(by_number.into_values());
            break;
        }
    }
    out
}

#[cfg(test)]
mod graph_tests;

#[cfg(test)]
mod tests;
