//! Extracting the `## Handoff` / `## Plan friction` knowledge a session leaves
//! in its plan, and rendering predecessors' handoffs for the next session.
//! Pure functions over markdown strings — no I/O, no `gh` calls.
//!
//! The flow ("standing on the shoulders of giants"): the executor writes a
//! `## Handoff` section into `.ralphy/plan.md`; at close the runner publishes
//! it (plus `## Plan friction`) as an issue comment; before planning a
//! dependent issue, the runner collects those comments from the closed
//! blockers into `.ralphy/handoffs.md`, which the planner reads as leads.

use regex::Regex;
use std::fmt::Write as _;

/// The marker heading a close-report comment carries. `find_handoff_comment`
/// keys on it, so publishing and collecting stay in lockstep.
const HANDOFF_HEADING: &str = "## Handoff";

/// Build the close-report comment from a plan: the `## Handoff` section plus
/// the `## Plan friction` section, each kept under its own heading. Returns
/// `None` when the plan carries neither (or only blank ones), so callers can
/// skip posting an empty comment.
pub fn close_report(plan_md: &str) -> Option<String> {
    let handoff = section(plan_md, r"(?im)^##\s+Handoff\s*$");
    let friction = section(plan_md, r"(?im)^##\s+Plan friction\s*$");

    let mut out = String::new();
    if !handoff.is_empty() {
        out.push_str(HANDOFF_HEADING);
        out.push_str("\n\n");
        out.push_str(handoff);
        out.push('\n');
    }
    if !friction.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("## Plan friction\n\n");
        out.push_str(friction);
        out.push('\n');
    }
    (!out.is_empty()).then_some(out)
}

/// Extract the planner's reasoning from an infeasible plan: the prose under
/// `## Feasible: ...` (the heading itself carries the verdict, so any tail
/// after "Feasible" is accepted). Returns `None` when the section is absent
/// or blank.
pub fn infeasible_reason(plan_md: &str) -> Option<String> {
    let reason = section(plan_md, r"(?im)^##\s+Feasible\b.*$");
    (!reason.is_empty()).then(|| reason.to_owned())
}

/// Whether an infeasible reason is a bundle verdict: the planner judged the
/// issue a multi-task bundle that needs splitting into child issues, rather
/// than under-specified. Keys on the literal word "bundle", which the planning
/// prompt requires the verdict prose to carry.
pub fn is_bundle_reason(reason: &str) -> bool {
    reason.to_lowercase().contains("bundle")
}

/// Whether a plan carries a bundle verdict. A `## Feasible: yes` heading is
/// never one, whatever its prose says ("Not a bundle: …" contains the word);
/// any other heading defers to [`is_bundle_reason`] over its prose.
///
/// # Panics
///
/// Never: each regex is a valid literal.
pub fn is_bundle_verdict(plan_md: &str) -> bool {
    let says_yes = Regex::new(r"(?im)^##\s+Feasible\s*:\s*yes\b")
        .expect("valid regex")
        .is_match(plan_md);
    !says_yes && infeasible_reason(plan_md).is_some_and(|r| is_bundle_reason(&r))
}

/// Pick the handoff out of an issue's comments: the LAST comment containing a
/// `## Handoff` heading (a re-run of the issue supersedes earlier reports).
///
/// # Panics
///
/// Never: each regex is a valid literal.
pub fn find_handoff_comment(comments: &[String]) -> Option<String> {
    let re = Regex::new(r"(?im)^##\s+Handoff\s*$").expect("valid regex");
    comments
        .iter()
        .rev()
        .find(|c| re.is_match(c))
        .map(|c| c.trim().to_owned())
}

/// Render `.ralphy/handoffs.md` from the collected `(issue number, handoff
/// comment)` pairs. Returns `None` when there is nothing to write, so the
/// caller can remove a stale file instead.
pub fn render_handoffs_file(entries: &[(u64, String)]) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let mut out = String::from(
        "# Handoffs from dependency issues\n\n\
         Knowledge left by the closed issues this one depends on. Entries are\n\
         leads, not truths — they were accurate when each issue closed.\n",
    );
    for (number, handoff) in entries {
        writeln!(out, "\n---\n\n## From #{number}\n\n{handoff}").expect(crate::WRITE_TO_STRING);
    }
    Some(out)
}

/// Whether the plan carries a non-blank `## Handoff` section at all. Lets the
/// runner tell "no handoff" apart from "handoff whose blocks didn't match
/// [`knowledge_note`]'s expected labels" — the second is a lesson silently
/// lost unless surfaced.
pub fn has_handoff(plan_md: &str) -> bool {
    !section(plan_md, r"(?im)^##\s+Handoff\s*$").is_empty()
}

/// Extract the durable-knowledge bullets from a plan's `## Handoff` section:
/// the `**Environment facts & traps**` and `**Commands that work**` entries,
/// verbatim with their sub-lines. `Delivered` and `Residue` are issue-specific
/// and stay out — they already travel to dependents via `handoffs.md`. Returns
/// `None` when the handoff is absent or carries neither, so the runner skips
/// writing an empty knowledge file.
pub fn knowledge_note(plan_md: &str) -> Option<String> {
    let handoff = section(plan_md, r"(?im)^##\s+Handoff\s*$");
    if handoff.is_empty() {
        return None;
    }
    let mut out = String::new();
    for key in ["Environment facts & traps", "Commands that work"] {
        let block = bullet_block(handoff, key);
        if !block.is_empty() {
            out.push_str(block);
            out.push('\n');
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Extract the `**Knowledge used**` citations from a plan's `## Handoff`
/// section: the cache's hit-rate signal — which `KNOWLEDGE.md` / `handoffs.md`
/// bullets the session says it relied on. Returns `None` when the handoff or
/// the key is absent (the caller warns; drift must be visible), `Some(vec![])`
/// for an honest `none` (or a key with nothing parseable under it — tolerant
/// degradation), and `Some(items)` otherwise: the inline tail after the key
/// plus any sub-bullet lines, one citation each.
pub fn knowledge_used(plan_md: &str) -> Option<Vec<String>> {
    let handoff = section(plan_md, r"(?im)^##\s+Handoff\s*$");
    if handoff.is_empty() {
        return None;
    }
    let block = bullet_block(handoff, "Knowledge used");
    if block.is_empty() {
        return None;
    }
    let items: Vec<String> = block
        .lines()
        .filter_map(citation)
        .map(str::to_owned)
        .collect();
    if let [only] = items.as_slice() {
        if only
            .trim_matches(['`', '"', '\'', '.'])
            .eq_ignore_ascii_case("none")
        {
            return Some(Vec::new());
        }
    }
    Some(items)
}

/// The citation one line of the `**Knowledge used**` block carries, without
/// its bullet; `None` for a blank line or a code fence.
fn citation(line: &str) -> Option<&str> {
    // The key line itself contributes only its inline tail after the `:`.
    let line = match line.split_once("**Knowledge used**") {
        Some((_, tail)) => tail.trim_start_matches(':'),
        None => line,
    };
    let item = line.trim().trim_start_matches('-').trim();
    (!item.is_empty() && !item.starts_with("```")).then_some(item)
}

/// The handoff sub-headings the prompts specify — the block boundaries for
/// `bullet_block`. Executors write them either as bullets (`- **Key**: ...`)
/// or as standalone bold lines (`**Key**:` followed by sub-bullets); both
/// shapes are accepted, and a block ends at the next KNOWN sub-heading so a
/// content line that merely starts bold never truncates it.
const HANDOFF_KEYS: [&str; 5] = [
    "Delivered",
    "Environment facts & traps",
    "Commands that work",
    "Residue",
    "Knowledge used",
];

/// The `<key>` sub-heading line and everything under it, up to the next known
/// sub-heading (or end of input). Empty when the key is absent.
fn bullet_block<'a>(md: &'a str, key: &str) -> &'a str {
    let key_re = Regex::new(&format!(
        r"(?im)^\s*(?:-\s*)?\*\*{}\*\*",
        regex::escape(key)
    ))
    .expect("valid regex");
    let Some(start) = key_re.find(md).map(|m| m.start()) else {
        return "";
    };
    let any_key = HANDOFF_KEYS.map(regex::escape).join("|");
    let boundary_re =
        Regex::new(&format!(r"(?im)^\s*(?:-\s*)?\*\*(?:{any_key})\*\*")).expect("valid regex");
    let end = boundary_re
        .find_iter(md)
        .map(|m| m.start())
        .find(|&s| s > start)
        .unwrap_or(md.len());
    md.get(start..end)
        .expect("both ends are regex match starts, so char boundaries")
        .trim_end()
}

/// Trimmed section body following the first heading matching `re`.
fn section<'a>(md: &'a str, re: &str) -> &'a str {
    let heading_re = Regex::new(re).expect("valid regex");
    crate::markdown::section_after_heading(md, &heading_re).trim()
}

#[cfg(test)]
mod tests;
