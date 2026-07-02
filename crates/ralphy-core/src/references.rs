//! Pre-fetching the issues the current issue's body references — its
//! `## Blocked by` and `## Parent` sections plus any inline `#N` mention — so the
//! planner reads their source spec instead of paraphrasing a `#N`. Pure rendering
//! lives here; the fetch is I/O ([`crate::github::fetch_reference`]) and the
//! orchestration is the runner ([`write_references`](../runner) before the plan
//! pass).
//!
//! Why the whole body, not just the structured sections: a `#N` written as a
//! `- ` bullet under `## Blocked by`, inline under `## Parent`, OR mentioned in
//! prose is a load-bearing dependency, provenance, or context link the planner is
//! apt to restate as fact in a child issue — and an inlined `#N` caveat is exactly
//! how a second-hand qualifier got laundered into a confident claim before. The
//! reference set ([`crate::blocked::referenced_issues`]) leads with the structured
//! refs, then adds inline ones, matching GitHub's autolink boundary so hex colors
//! and anchors are not swept in. Depth is one: the fetched bodies' own references
//! are not followed; their comment threads are not downloaded (open the URL on
//! demand).

/// One referenced issue, fetched from source: its number, lifecycle state
/// (`OPEN`/`CLOSED`, verbatim from `gh`), title, body, and the issue URL. The
/// `url` is the handle the planner follows to pull what this file deliberately
/// omits — the comment thread — on demand when a decision hinges on it, rather
/// than the runner dumping every referenced issue's full discussion up front.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub number: u64,
    pub state: String,
    pub title: String,
    pub body: String,
    pub url: String,
}

/// Render `.ralphy/references.md` from the fetched references. Returns `None`
/// when there is nothing to write, so the caller removes a stale file instead.
pub fn render_references_file(refs: &[Reference]) -> Option<String> {
    if refs.is_empty() {
        return None;
    }
    let mut out = String::from(
        "# Referenced issues\n\n\
         The current issue's body references these issues — via its `## Blocked by`\n\
         and `## Parent` sections and any inline `#N` mention; each one's source\n\
         title and body are reproduced below so you read the referenced spec, not\n\
         a paraphrase of it. The `state` shown was current at fetch time —\n\
         treat it as a lead and re-check if a decision hinges on it. Only the\n\
         body is reproduced; to read a referenced issue's\n\
         comment thread (caveats, clarifications), open its URL or run\n\
         `gh issue view <n>`.\n",
    );
    for r in refs {
        out.push_str(&format!(
            "\n---\n\n## #{} ({}) — {}\n{}\n\n{}\n",
            r.number,
            r.state,
            r.title.trim(),
            r.url.trim(),
            r.body.trim(),
        ));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(number: u64, state: &str, title: &str, body: &str) -> Reference {
        Reference {
            number,
            state: state.into(),
            title: title.into(),
            body: body.into(),
            url: format!("https://github.com/o/r/issues/{number}"),
        }
    }

    #[test]
    fn render_lists_each_reference_with_state_url_and_body() {
        let refs = vec![
            reference(
                13,
                "OPEN",
                "S2a — corpus real",
                "## What\nground truth corpus",
            ),
            reference(
                15,
                "CLOSED",
                "Spike S2c (bundle)",
                "## Parent\nSplit target",
            ),
        ];
        let file = render_references_file(&refs).expect("file content");
        assert!(file.contains("## #13 (OPEN) — S2a — corpus real"));
        assert!(file.contains("https://github.com/o/r/issues/13"));
        assert!(file.contains("ground truth corpus"));
        assert!(file.contains("## #15 (CLOSED) — Spike S2c (bundle)"));
        assert!(file.contains("https://github.com/o/r/issues/15"));
        assert!(file.contains("Split target"));
        // The header frames entries as leads and points at the URL for comments.
        assert!(file.contains("treat it as a lead"));
        assert!(file.contains("comment thread"));
    }

    #[test]
    fn render_empty_is_none() {
        assert_eq!(render_references_file(&[]), None);
    }

    #[test]
    fn render_puts_url_under_heading_and_trims_whitespace() {
        let refs = vec![reference(7, "OPEN", "  padded  ", "\n\nbody\n\n")];
        let file = render_references_file(&refs).expect("file content");
        assert!(file.contains("## #7 (OPEN) — padded\nhttps://github.com/o/r/issues/7\n\nbody\n"));
        assert!(!file.contains("padded  \n"));
    }
}
