//! The comments of an HTML document, found by the walk of `html_tags`: a
//! tag's attribute values and the bodies of `script` and `style` are not
//! markup, so a `<!--` there is text.

use crate::html_tags::{read_tag, skip_declaration, skip_past, starts};

/// The start line of each comment. A line that holds only a part marker,
/// `<!-- part: name.part.html -->`, is not a comment.
pub(super) fn comments(html: &str) -> Vec<usize> {
    let cs: Vec<char> = html.chars().collect();
    let mut found = Vec::new();
    let mut line = 1;
    let mut i = 0;
    while i < cs.len() {
        if starts(&cs, i, "<!") {
            if starts(&cs, i, "<!--") && !is_marker_line(&cs, i) {
                found.push(line);
            }
            i = skip_declaration(&cs, i, &mut line);
        } else if starts(&cs, i, "</") {
            i = skip_past(&cs, i, ">", &mut line);
        } else if cs[i] == '<' && cs.get(i + 1).is_some_and(char::is_ascii_alphabetic) {
            let (tag, _, _, end) = read_tag(&cs, i + 1, &mut line);
            i = if tag == "script" || tag == "style" {
                let body_end = skip_past(&cs, end, &format!("</{tag}"), &mut line);
                skip_past(&cs, body_end, ">", &mut line)
            } else {
                end
            };
        } else {
            if cs[i] == '\n' {
                line += 1;
            }
            i += 1;
        }
    }
    found
}

/// Whether the comment at `i` is a part marker with nothing else on its line.
fn is_marker_line(cs: &[char], i: usize) -> bool {
    let Some(close) = (i..cs.len()).find(|&k| starts(cs, k, "-->")) else {
        return false;
    };
    let inner: String = cs[i + 4..close].iter().collect();
    let line_start = cs[..i]
        .iter()
        .rposition(|&c| c == '\n')
        .map_or(0, |p| p + 1);
    let before_blank = cs[line_start..i].iter().all(|c| c.is_whitespace());
    let after = cs[close + 3..].iter().take_while(|&&c| c != '\n');
    let after_blank = after.into_iter().all(|c| c.is_whitespace());
    !inner.contains('\n') && inner.trim().starts_with("part:") && before_blank && after_blank
}

#[cfg(test)]
mod tests {
    use super::comments;

    #[test]
    fn finds_comments_outside_values() {
        let html = concat!(
            "<!-- part: head.part.html -->\n",
            "<div title=\"<!-- in a value -->\">\n",
            "  <!-- part: a.part.html --> text\n",
            "  <!-- one\n",
            "  two -->\n",
            "</div>\n",
            "<script>var a = '<!-- in a script -->';</script>\n",
            "<style>/* <!-- in a style --> */</style>\n",
            "<p>x</p> <!-- part: b.part.html -->\n",
            "   <!-- part: c.part.html -->\n",
            "<!-- after -->\n",
        );
        assert_eq!(comments(html), vec![3, 4, 9, 11]);
    }
}
