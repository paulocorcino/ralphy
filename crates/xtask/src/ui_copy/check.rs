//! `ui-copy --check`: the checkable rules of ADR-0065, read from
//! `docs/ui-copy-rules.json` and applied to every row of the inventory (#425).
//!
//! It reports and never judges meaning: whether a verb fits its act (§4) or a
//! sentence is clear (§10) is for the editorial passes. In report mode the
//! exit code is always 0; #431 makes a violation fail.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use super::{Row, RULES_FILE};

#[derive(Deserialize, Debug)]
pub(crate) struct Rules {
    casing: Casing,
    proper_nouns: Vec<String>,
    banned: Vec<Term>,
    punctuation: Vec<Punctuation>,
    sentence_max_words: usize,
    plain: Vec<Term>,
    contractions: Contractions,
    #[serde(default)]
    pub(crate) copy_helpers: Vec<String>,
    #[serde(default)]
    exemptions: Vec<Exemption>,
}

#[derive(Deserialize, Debug)]
struct Casing {
    exempt_kinds: Vec<String>,
    state_words: Vec<String>,
    key_names: Vec<String>,
}

#[derive(Deserialize, Debug)]
struct Term {
    term: String,
    #[serde(rename = "use")]
    instead: String,
    #[serde(default)]
    whole_word: bool,
    #[serde(default)]
    case_sensitive: bool,
}

#[derive(Deserialize, Debug)]
struct Punctuation {
    find: String,
    #[serde(rename = "use")]
    instead: String,
}

#[derive(Deserialize, Debug)]
struct Contractions {
    banned: Vec<String>,
}

/// A text that breaks one rule on purpose. It is keyed by text, not by line,
/// so it survives edits around it.
#[derive(Deserialize, Debug)]
struct Exemption {
    file: String,
    text: String,
    rule: String,
    reason: String,
}

pub(crate) fn load(root: &Path) -> Result<Rules> {
    let path = root.join(RULES_FILE);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    parse(&text).with_context(|| format!("parsing {}", path.display()))
}

fn parse(text: &str) -> Result<Rules> {
    let rules: Rules = serde_json::from_str(text)?;
    if let Some(e) = rules.exemptions.iter().find(|e| e.reason.trim().is_empty()) {
        anyhow::bail!(
            "the exemption for {} rule {} ({:?}) has no reason; ADR-0065 §9 requires one",
            e.file,
            e.rule,
            e.text
        );
    }
    Ok(rules)
}

#[derive(Debug, PartialEq)]
pub(crate) struct Violation {
    pub(crate) file: String,
    pub(crate) line: usize,
    pub(crate) rule: String,
    pub(crate) text: String,
    pub(crate) hint: String,
}

pub(crate) struct Report<'a> {
    pub(crate) violations: Vec<Violation>,
    pub(crate) concatenated: usize,
    stale: Vec<&'a Exemption>,
}

pub(crate) fn check<'a>(rows: &[Row], rules: &'a Rules) -> Report<'a> {
    let mut used = vec![false; rules.exemptions.len()];
    let mut violations = Vec::new();
    for row in rows {
        for (rule, hint) in row_violations(row, rules) {
            let exempt = rules
                .exemptions
                .iter()
                .position(|e| e.file == row.file && e.text == row.text && e.rule == rule);
            if let Some(k) = exempt {
                used[k] = true;
                continue;
            }
            violations.push(Violation {
                file: row.file.clone(),
                line: row.line,
                rule,
                text: row.text.clone(),
                hint,
            });
        }
    }
    violations.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    Report {
        violations,
        concatenated: rows.iter().filter(|r| r.concatenated).count(),
        stale: rules
            .exemptions
            .iter()
            .zip(used)
            .filter(|(_, used)| !used)
            .map(|(e, _)| e)
            .collect(),
    }
}

/// Every `(rule, hint)` one text breaks.
fn row_violations(row: &Row, rules: &Rules) -> Vec<(String, String)> {
    let text = mask_holes(&row.text);
    let mut out = Vec::new();
    let casing = &rules.casing;
    let whole = text.trim().to_lowercase();
    let chip_or_key = casing
        .state_words
        .iter()
        .chain(&casing.key_names)
        .any(|w| w.to_lowercase() == whole);
    if !casing.exempt_kinds.iter().any(|k| k == row.kind.label()) && !chip_or_key {
        if !row.continues && starts_lowercase(&text) {
            out.push((
                "casing:first".to_string(),
                "start with a capital letter".to_string(),
            ));
        }
        let capitals = title_case_words(&text, &rules.proper_nouns);
        if !capitals.is_empty() {
            out.push((
                "casing:title".to_string(),
                format!("lowercase {}", capitals.join(", ")),
            ));
        }
    }
    for t in &rules.banned {
        if has_term(&text, &t.term, t.whole_word, t.case_sensitive) {
            out.push((format!("banned:{}", t.term), format!("use {}", t.instead)));
        }
    }
    for p in &rules.punctuation {
        if text.contains(&p.find) {
            out.push((
                format!("punctuation:{}", p.find),
                format!("use {}", p.instead),
            ));
        }
    }
    if let Some(words) = longest_sentence(&text).filter(|&n| n > rules.sentence_max_words) {
        out.push((
            "sentence-length".to_string(),
            format!("{words} words; the limit is {}", rules.sentence_max_words),
        ));
    }
    for t in &rules.plain {
        if has_term(&text, &t.term, t.whole_word, t.case_sensitive) {
            out.push((format!("plain:{}", t.term), format!("use {}", t.instead)));
        }
    }
    let found: Vec<&str> = rules
        .contractions
        .banned
        .iter()
        .filter(|c| has_contraction(&text, c))
        .map(String::as_str)
        .collect();
    if !found.is_empty() {
        out.push((
            "contraction".to_string(),
            format!("write the two full words ({})", found.join(", ")),
        ));
    }
    out
}

/// The text with every `{expr}` hole replaced by `{}`: a hole is code, and no
/// rule reads it.
fn mask_holes(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '{' => {
                if depth == 0 {
                    out.push_str("{}");
                }
                depth += 1;
            }
            '}' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// The first letter is lowercase. A leading quote, bracket or symbol is
/// skipped; a text whose first word is a hole is not judged.
fn starts_lowercase(text: &str) -> bool {
    text.chars()
        .find(|&c| c.is_alphanumeric() || c == '{')
        .is_some_and(char::is_lowercase)
}

/// Words after the first that start with a capital and are not proper nouns.
/// Skipped: a word that starts a sentence (after `.`, `!`, `?`) or a segment
/// (after `·`), and a quoted span (`“…”`, `"…"`), which names a thing.
fn title_case_words(text: &str, proper_nouns: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut starts = true;
    for word in mask_quotes(text).split_whitespace() {
        let core = word.trim_matches(|c: char| !c.is_alphanumeric());
        let core = core
            .strip_suffix("'s")
            .or_else(|| core.strip_suffix("’s"))
            .unwrap_or(core);
        let proper = core
            .split('/')
            .all(|part| !starts_upper(part) || proper_nouns.iter().any(|p| p == part));
        // A key chord (`Ctrl+Shift+F`) names keys, and single letters
        // (`A–Z`) are a range, not a word.
        let chord = core.contains('+');
        let letters = core
            .split(|c: char| !c.is_alphabetic())
            .all(|run| run.chars().count() <= 1);
        if !starts && !proper && !chord && !letters && !out.iter().any(|w| w == core) {
            out.push(core.to_string());
        }
        let ends = word.ends_with(['.', '!', '?']);
        if ends || word == "·" || word.chars().any(char::is_alphanumeric) {
            starts = ends || word == "·";
        }
    }
    out
}

/// The text with each quoted span replaced by `{}`.
fn mask_quotes(text: &str) -> String {
    let mut out = String::new();
    let mut open: Option<char> = None;
    for c in text.chars() {
        match (open, c) {
            (None, '“') => open = Some('”'),
            (None, '"') => open = Some('"'),
            (Some(close), c) if c == close => {
                open = None;
                out.push_str("{}");
            }
            (Some(_), _) => {}
            (None, c) => out.push(c),
        }
    }
    out
}

fn starts_upper(s: &str) -> bool {
    s.chars().next().is_some_and(char::is_uppercase)
}

/// Is `term` in `text`? A whole-word term needs a non-alphanumeric char (or
/// the edge) on each side. A match inside a token that reads as code — it
/// holds `/`, or starts with `.`, `<` or a backtick — is not a match:
/// `.ralphy/settings.json` names a path, not the product.
fn has_term(text: &str, term: &str, whole_word: bool, case_sensitive: bool) -> bool {
    let (hay, needle) = if case_sensitive {
        (text.to_string(), term.to_string())
    } else {
        (text.to_ascii_lowercase(), term.to_ascii_lowercase())
    };
    hay.match_indices(&needle).any(|(at, m)| {
        let before = hay[..at].chars().next_back();
        let after = hay[at + m.len()..].chars().next();
        let bounded =
            !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric);
        (!whole_word || bounded) && !in_code_token(&hay, at, at + m.len())
    })
}

fn in_code_token(text: &str, from: usize, to: usize) -> bool {
    let start = text[..from].rfind(char::is_whitespace).map_or(0, |k| k + 1);
    let end = text[to..]
        .find(char::is_whitespace)
        .map_or(text.len(), |k| to + k);
    let token = &text[start..end];
    token.contains('/') || token.starts_with(['.', '<', '`'])
}

/// A contraction that starts with an apostrophe or `n'` (`'re`, `n't`) is a
/// suffix of a word; any other (`it's`) is a whole word. Curly and straight
/// apostrophes are the same.
fn has_contraction(text: &str, contraction: &str) -> bool {
    let hay = text.replace('’', "'").to_lowercase();
    let c = contraction.to_lowercase();
    let suffix = c.starts_with('\'') || c.starts_with("n'");
    hay.match_indices(&c).any(|(at, m)| {
        let before = hay[..at].chars().next_back();
        let after = hay[at + m.len()..].chars().next();
        let ends = !after.is_some_and(char::is_alphanumeric);
        if suffix {
            before.is_some_and(char::is_alphabetic) && ends
        } else {
            !before.is_some_and(char::is_alphanumeric) && ends
        }
    })
}

/// Word count of the longest sentence. A sentence ends at `.`, `!` or `?`
/// followed by a space or the end; a hole is not a word.
fn longest_sentence(text: &str) -> Option<usize> {
    let mut longest = None;
    let mut count = 0usize;
    for word in text.split_whitespace() {
        if word.chars().any(char::is_alphanumeric) {
            count += 1;
        }
        if word.ends_with(['.', '!', '?']) {
            longest = longest.max(Some(count));
            count = 0;
        }
    }
    longest.max(Some(count)).filter(|&n| n > 0)
}

pub(crate) fn to_text(report: &Report) -> String {
    let mut out = String::new();
    out.push_str("ui-copy --check: the ADR-0065 rules over the inventory (report mode, #425)\n\n");
    for v in &report.violations {
        out.push_str(&format!(
            "{}:{}: {}: {}  ({})\n",
            v.file, v.line, v.rule, v.text, v.hint
        ));
    }
    let mut totals: BTreeMap<&str, usize> = BTreeMap::new();
    for v in &report.violations {
        *totals.entry(family(&v.rule)).or_default() += 1;
    }
    out.push_str("\nViolations by rule:\n");
    for (rule, n) in &totals {
        out.push_str(&format!("  {rule:<16} {n}\n"));
    }
    let texts = {
        let mut seen: Vec<(&str, usize, &str)> = report
            .violations
            .iter()
            .map(|v| (v.file.as_str(), v.line, v.text.as_str()))
            .collect();
        seen.dedup();
        seen.len()
    };
    out.push_str(&format!(
        "  {:<16} {} in {} texts\n",
        "total",
        report.violations.len(),
        texts
    ));
    out.push_str(&format!(
        "\nConcatenated texts (informational, ADR-0065 §9): {}\n",
        report.concatenated
    ));
    if report.stale.is_empty() {
        out.push_str("Stale exemptions: none\n");
    } else {
        out.push_str("Stale exemptions (they match no text; remove them):\n");
        for e in &report.stale {
            out.push_str(&format!("  {}: {}: {}\n", e.file, e.rule, e.text));
        }
    }
    out.push_str("Report mode: the exit code is 0 whatever the count. #431 makes it fail.\n");
    out
}

/// `banned:plane` counts under `banned`; the casing rules stay apart.
fn family(rule: &str) -> &str {
    match rule.split_once(':') {
        Some((head @ ("banned" | "plain" | "punctuation"), _)) => head,
        _ => rule,
    }
}

#[cfg(test)]
mod tests;
