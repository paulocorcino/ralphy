//! Shared markdown parsing utilities.

use regex::Regex;

/// Return the text of the section that follows `heading_re` in `md`, stopping
/// at the next `## ` heading (or end of input). Returns `""` when the heading
/// is not found.
pub(crate) fn section_after_heading<'a>(md: &'a str, heading_re: &Regex) -> &'a str {
    let Some(start_m) = heading_re.find(md) else {
        return "";
    };
    let after = &md[start_m.end()..];
    let end_re = Regex::new(r"(?m)^##\s+").expect("valid regex");
    let end = end_re.find(after).map(|m| m.start()).unwrap_or(after.len());
    &after[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_section_until_next_heading() {
        let re = Regex::new(r"(?im)^##\s+Target\s*$").unwrap();
        let md = "## Target\nhello\nworld\n## Next\nother\n";
        assert_eq!(section_after_heading(md, &re), "\nhello\nworld\n");
    }

    #[test]
    fn absent_heading_returns_empty() {
        let re = Regex::new(r"(?im)^##\s+Missing\s*$").unwrap();
        let md = "## Something else\ncontent\n";
        assert_eq!(section_after_heading(md, &re), "");
    }

    #[test]
    fn stops_at_next_heading() {
        let re = Regex::new(r"(?im)^##\s+First\s*$").unwrap();
        let md = "## First\nfirst content\n## Second\nsecond content\n";
        let section = section_after_heading(md, &re);
        assert!(section.contains("first content"));
        assert!(!section.contains("second content"));
    }
}
