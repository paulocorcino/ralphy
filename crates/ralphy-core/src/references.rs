//! Pre-fetching the issues named in the current issue's STRUCTURED reference
//! sections (`## Blocked by` and `## Parent`) so the planner reads their source
//! spec instead of paraphrasing a `#N` mention. Pure rendering lives here; the
//! fetch is I/O ([`crate::github::fetch_reference`]) and the orchestration is the
//! runner ([`write_references`](../runner) before the plan pass).
//!
//! Why only the structured sections: a `#N` written as a `- ` bullet under
//! `## Blocked by`, or inline under `## Parent`, is a load-bearing dependency or
//! provenance link the planner is apt to restate as fact in a child issue's
//! body — the exact way a second-hand caveat got laundered into a confident
//! claim before. Prose `#N` mentions elsewhere are deliberately left out (the
//! same scoping [`crate::blocked::parse_blocked_by`] draws): too noisy to fetch
//! wholesale, and the planning prompt instructs verifying those at source on
//! demand. Depth is one: the fetched bodies' own references are not followed.

/// One referenced issue, fetched from source: its number, lifecycle state
/// (`OPEN`/`CLOSED`, verbatim from `gh`), title, and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub number: u64,
    pub state: String,
    pub title: String,
    pub body: String,
}

/// Render `.ralphy/references.md` from the fetched references. Returns `None`
/// when there is nothing to write, so the caller removes a stale file instead.
pub fn render_references_file(refs: &[Reference]) -> Option<String> {
    if refs.is_empty() {
        return None;
    }
    let mut out = String::from(
        "# Referenced issues (Blocked by / Parent)\n\n\
         The current issue's `## Blocked by` and `## Parent` sections name these\n\
         issues; each one's source title and body are reproduced below so you read\n\
         the referenced spec, not a paraphrase of it. The `state` shown was current\n\
         at fetch time — treat it as a lead and re-check if a decision hinges on it.\n",
    );
    for r in refs {
        out.push_str(&format!(
            "\n---\n\n## #{} ({}) — {}\n\n{}\n",
            r.number,
            r.state,
            r.title.trim(),
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
        }
    }

    #[test]
    fn render_lists_each_reference_with_state_and_body() {
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
        assert!(file.contains("ground truth corpus"));
        assert!(file.contains("## #15 (CLOSED) — Spike S2c (bundle)"));
        assert!(file.contains("Split target"));
        // The header frames entries as leads, not the live state of record.
        assert!(file.contains("treat it as a lead"));
    }

    #[test]
    fn render_empty_is_none() {
        assert_eq!(render_references_file(&[]), None);
    }

    #[test]
    fn render_trims_title_and_body_whitespace() {
        let refs = vec![reference(7, "OPEN", "  padded  ", "\n\nbody\n\n")];
        let file = render_references_file(&refs).expect("file content");
        assert!(file.contains("## #7 (OPEN) — padded\n\nbody\n"));
        assert!(!file.contains("padded  \n"));
    }
}
