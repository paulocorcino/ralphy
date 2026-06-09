//! Parsing the `## Blocked by` section from an issue body. Pure functions over
//! markdown strings — no I/O, no `gh` calls.

use regex::Regex;

/// Extract `#N` issue references from the `## Blocked by` section of an issue
/// body. Returns an empty list when the section is absent or contains no refs
/// (e.g. "None - can start immediately").
///
/// Stops at the next `##` heading so refs in unrelated sections are not collected.
pub fn parse_blocked_by(body: &str) -> Vec<u64> {
    let heading_re = Regex::new(r"(?im)^##\s+Blocked by\s*$").expect("valid regex");
    let end_re = Regex::new(r"(?m)^##\s+").expect("valid regex");

    let Some(start_m) = heading_re.find(body) else {
        return Vec::new();
    };

    let after = &body[start_m.end()..];
    let end = end_re.find(after).map(|m| m.start()).unwrap_or(after.len());
    let section = &after[..end];

    // Match only bullet-list items: `- #N` (leading whitespace optional).
    // This avoids treating prose references like "step #3" as issue refs.
    let ref_re = Regex::new(r"(?m)^\s*-\s*#(\d+)").expect("valid regex");
    ref_re
        .captures_iter(section)
        .map(|c| c[1].parse::<u64>().expect("matched digits"))
        .collect()
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
}
