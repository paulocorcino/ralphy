//! Parsing the `## Acceptance ledger` from a plan and applying it to a GitHub
//! issue body. Pure functions over markdown strings — no I/O, no `gh` calls.

use regex::Regex;

/// Whether a ledger verdict was automatically verified or requires human review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerdictKind {
    Verified,
    ReviewOnly,
}

/// One entry in the acceptance ledger: the criterion text (verbatim, for
/// matching against the issue body), the verdict kind, and any evidence string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub criterion: String,
    pub kind: VerdictKind,
    pub evidence: String,
}

/// Parse the `## Acceptance ledger` section of a plan markdown into a list of
/// [`Verdict`]s. Returns an empty list when the section is absent or empty.
///
/// Grammar (one bullet per criterion):
///   `- [verified] <criterion text> — evidence: <text>`
///   `- [review-only] <criterion text> — evidence: <text>`
///
/// A dash (`—` U+2014, `–` U+2013, `--`, or `-`) separates the criterion from
/// the evidence key. Evidence text may be empty.
pub fn parse_ledger(md: &str) -> Vec<Verdict> {
    let heading_re = Regex::new(r"(?im)^##\s+Acceptance ledger\s*$").expect("valid regex");
    let section = crate::markdown::section_after_heading(md, &heading_re);

    // Match lines: `- [verified|review-only] <criterion> — evidence: <evidence>`
    // Any dash spelling separates them: an em dash normalized to ASCII on the
    // way to disk used to make the line vanish from the ledger entirely, which
    // the protocol lint then reported as "no verdict line parsed" — a true
    // failure naming the wrong cause. The separator is only punctuation; the
    // `evidence:` key immediately after it is what identifies the split.
    let line_re = Regex::new(concat!(
        r"(?m)^\s*-\s*\[(verified|review-only)\]\s+(.+?)\s*",
        r"(?:\u{2014}|\u{2013}|-{1,2})\s*evidence:\s*(.*?)\s*$",
    ))
    .expect("valid regex");

    let mut verdicts = Vec::new();
    for cap in line_re.captures_iter(section) {
        let kind = match &cap[1] {
            "verified" => VerdictKind::Verified,
            _ => VerdictKind::ReviewOnly,
        };
        verdicts.push(Verdict {
            criterion: cap[2].trim().to_owned(),
            kind,
            evidence: cap[3].trim().to_owned(),
        });
    }
    verdicts
}

/// Result of applying a ledger to an issue body.
pub struct TickResult {
    /// The updated issue body with verified criteria ticked.
    pub new_body: String,
    /// Criterion texts that were found and ticked (`- [ ]` → `- [x]`).
    pub ticked: Vec<String>,
    /// Verified criteria whose verbatim text was not found as an unchecked line.
    pub unmatched: Vec<String>,
}

/// Normalize an acceptance line for matching only: drop the inline-markdown
/// delimiters (`*`, `_`, and backtick) that a ledger criterion routinely loses
/// when it is transcribed from the issue's AC bullet, collapse runs of
/// whitespace, and drop trailing sentence punctuation (`.`, `;`, `:`, `,`) that
/// the transcription just as routinely drops when it rewrites the AC bullet as a
/// bare clause. Applied to BOTH sides of the comparison, so an identifier like
/// `blob_id` reduces identically on each side and still matches — this affects
/// matching alone; the ticked line keeps its original text verbatim.
fn normalize_ac(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '*' | '_' | '`'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(['.', ';', ':', ','])
        .to_owned()
}

/// Collect the checkbox list items of a body as *logical* bullets: each item
/// is the physical line index where its `- [` bullet starts, paired with the
/// bullet's full text — soft-wrapped continuation lines joined by a single
/// space. Issue bodies hard-wrap their AC prose at ~78 columns, so a criterion
/// routinely spans several physical lines; matching against the physical line
/// alone can never tick those (the #266 failure). A continuation line is a
/// non-empty indented line that does not itself start a new bullet or heading;
/// a blank line ends the item.
fn logical_checkbox_items(lines: &[String]) -> Vec<(usize, String)> {
    let mut items = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim_start().starts_with("- [") {
            let (text, next) = join_continuations(lines, i);
            items.push((i, text));
            i = next;
        } else {
            i += 1;
        }
    }
    items
}

/// The bullet that starts at `lines[start]` with its continuation lines joined
/// by a single space, and the index of the first line after it.
fn join_continuations(lines: &[String], start: usize) -> (String, usize) {
    let mut text = lines[start].clone();
    let mut next = start + 1;
    while let Some(trimmed) = lines.get(next).and_then(|line| continuation(line)) {
        text.push(' ');
        text.push_str(trimmed);
        next += 1;
    }
    (text, next)
}

/// The trimmed text of `line` when it continues the bullet above it: a
/// non-empty indented line that does not start a new bullet or heading.
fn continuation(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let continues = !trimmed.is_empty()
        && line.starts_with(char::is_whitespace)
        && !trimmed.starts_with('-')
        && !trimmed.starts_with('#');
    continues.then_some(trimmed)
}

/// Apply verified verdicts to an issue body by flipping `- [ ] <criterion>` to
/// `- [x] <criterion>`. Matching is verbatim *modulo inline markdown,
/// whitespace and soft wrapping* (see `normalize_ac` and
/// `logical_checkbox_items`) — the ledger criterion is frequently transcribed
/// without the issue line's `**bold**`/`` `code` `` markers and as one logical
/// line where the issue hard-wraps, and an exact per-line match would silently
/// drop those ticks. Review-only verdicts are never ticked. Already-ticked
/// lines are left untouched.
pub fn apply_ledger(body: &str, verdicts: &[Verdict]) -> TickResult {
    let mut body_lines: Vec<String> = body.lines().map(str::to_owned).collect();
    let had_trailing_newline = body.ends_with('\n');
    let mut ticked = Vec::new();
    let mut unmatched = Vec::new();

    for verdict in verdicts {
        if verdict.kind != VerdictKind::Verified {
            continue;
        }
        if tick_criterion(&mut body_lines, &verdict.criterion) {
            ticked.push(verdict.criterion.clone());
        } else {
            unmatched.push(verdict.criterion.clone());
        }
    }

    let mut new_body = body_lines.join("\n");
    if had_trailing_newline {
        new_body.push('\n');
    }

    TickResult {
        new_body,
        ticked,
        unmatched,
    }
}

/// Tick the first unchecked bullet of `body_lines` whose normalized logical
/// text is `- [ ] <criterion>`. Returns whether one was found.
fn tick_criterion(body_lines: &mut [String], criterion: &str) -> bool {
    let target = normalize_ac(&format!("- [ ] {criterion}"));
    // Recomputed per verdict so a bullet ticked by an earlier verdict no
    // longer matches (its `[x]` normalizes differently), letting duplicate
    // criteria tick successive occurrences exactly as before.
    let found = logical_checkbox_items(body_lines)
        .into_iter()
        .find(|(_, text)| normalize_ac(text) == target);
    let Some((start, _)) = found else {
        return false;
    };
    // Compare normalized logical bullets, but flip the box on the original
    // first line so its markdown and wrapping survive. Replacing `[ ]` (not
    // `- [ ]`) tolerates bullet/whitespace variants the normalized match also
    // accepts.
    body_lines[start] = body_lines[start].replacen("[ ]", "[x]", 1);
    true
}

/// One verdict's entry in the evidence comment: its tag, criterion and
/// evidence, flagged `NEEDS REVIEW` when it is unmatched or review-only.
fn evidence_entry(verdict: &Verdict, unmatched: &[String]) -> String {
    let tag = match verdict.kind {
        VerdictKind::Verified => "verified",
        VerdictKind::ReviewOnly => "review-only",
    };
    let evidence = if verdict.evidence.is_empty() {
        "(no evidence recorded)"
    } else {
        &verdict.evidence
    };

    if unmatched.contains(&verdict.criterion) {
        format!(
            "- **[{tag}]** {}  \n  Evidence: {}  \n  [NEEDS REVIEW: criterion \
             not found in issue body — not auto-ticked]\n",
            verdict.criterion, evidence
        )
    } else if verdict.kind == VerdictKind::ReviewOnly {
        format!(
            "- **[{tag}]** {}  \n  Evidence: {}  \n  [NEEDS REVIEW: review-only — \
             not auto-ticked]\n",
            verdict.criterion, evidence
        )
    } else {
        format!(
            "- **[{tag}]** {}  \n  Evidence: {}\n",
            verdict.criterion, evidence
        )
    }
}

/// Render a structured evidence comment pairing each criterion with its verdict
/// and evidence. Review-only and unmatched verified criteria are explicitly
/// flagged for the human reviewer.
pub fn evidence_comment(verdicts: &[Verdict], unmatched: &[String]) -> String {
    let mut out = String::from("## Acceptance ledger\n\n");

    for verdict in verdicts {
        out.push_str(&evidence_entry(verdict, unmatched));
    }

    if !unmatched.is_empty() {
        out.push_str("\n### Unmatched criteria\n\n");
        out.push_str(
            "The following verified criteria could not be ticked automatically \
             because their verbatim text was not found as an unchecked line in the issue body:\n\n",
        );
        for c in unmatched {
            out.push_str(&format!("- {c}\n"));
        }
    }

    out
}

#[cfg(test)]
mod tests;
