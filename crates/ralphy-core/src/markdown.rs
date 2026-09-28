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
    fn section_after_heading_stops_at_the_next_heading() {
        // (case, heading, markdown, expected section)
        let rows = [
            (
                "until the next heading",
                "Target",
                "## Target\nhello\nworld\n## Next\nother\n",
                "\nhello\nworld\n",
            ),
            (
                "absent heading",
                "Missing",
                "## Something else\ncontent\n",
                "",
            ),
            (
                "the next section is left out",
                "First",
                "## First\nfirst content\n## Second\nsecond content\n",
                "\nfirst content\n",
            ),
        ];
        for (case, heading, md, want) in rows {
            let re = Regex::new(&format!(r"(?im)^##\s+{heading}\s*$")).unwrap();
            assert_eq!(section_after_heading(md, &re), want, "{case}");
        }
    }
}
