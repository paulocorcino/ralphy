//! The comments and the Alpine attributes of an HTML document, found by the
//! walk of `html_tags`: a tag's attribute values and the bodies of `script`
//! and `style` are not markup, so a `<!--` there is text.

use crate::html_tags::{read_tag, skip_declaration, skip_past, starts, Attr};

/// The start line of each comment. A line that holds only a part marker,
/// `<!-- part: name.part.html -->`, is not a comment.
pub(super) fn comments(html: &str) -> Vec<usize> {
    walk(html).0
}

/// Every Alpine attribute (`x-*`, `@*`, `:*`) that holds an expression, with
/// its expression and start line. `x-data`, `x-ref` and `x-transition*` hold
/// none, and only the list of `x-for`, right of `in` or `of`, is one.
pub(super) fn attributes(html: &str) -> Vec<(String, usize)> {
    walk(html)
        .1
        .into_iter()
        .filter_map(|a| expression(&a.name, a.value?).map(|e| (e, a.line)))
        .collect()
}

fn expression(name: &str, value: String) -> Option<String> {
    let alpine = name.starts_with("x-") || name.starts_with('@') || name.starts_with(':');
    let holds_none = ["x-data", "x-ref", "x-transition"].iter().any(|n| {
        name == *n || name.starts_with(&format!("{n}:")) || name.starts_with(&format!("{n}."))
    });
    if !alpine || holds_none {
        return None;
    }
    if name != "x-for" {
        return Some(value);
    }
    let list = [" in ", " of "]
        .iter()
        .find_map(|sep| value.split_once(sep));
    list.map(|(_, list)| list.to_owned())
}

/// The comment lines and the attributes of every tag, in document order.
fn walk(html: &str) -> (Vec<usize>, Vec<Attr>) {
    let cs: Vec<char> = html.chars().collect();
    let mut found = Vec::new();
    let mut attrs = Vec::new();
    let mut line = 1;
    let mut i = 0;
    while let Some(&c) = cs.get(i) {
        if starts(&cs, i, "<!") {
            if starts(&cs, i, "<!--") && !is_marker_line(&cs, i) {
                found.push(line);
            }
            i = skip_declaration(&cs, i, &mut line);
        } else if starts(&cs, i, "</") {
            i = skip_past(&cs, i, ">", &mut line);
        } else if c == '<' && cs.get(i + 1).is_some_and(char::is_ascii_alphabetic) {
            let (tag, tag_attrs, _, end) = read_tag(&cs, i + 1, &mut line);
            attrs.extend(tag_attrs);
            i = if tag == "script" || tag == "style" {
                let body_end = skip_past(&cs, end, &format!("</{tag}"), &mut line);
                skip_past(&cs, body_end, ">", &mut line)
            } else {
                end
            };
        } else {
            if c == '\n' {
                line += 1;
            }
            i += 1;
        }
    }
    (found, attrs)
}

/// Whether the comment at `i` is a part marker with nothing else on its line.
fn is_marker_line(cs: &[char], i: usize) -> bool {
    let Some(close) = (i..cs.len()).find(|&k| starts(cs, k, "-->")) else {
        return false;
    };
    let inner: String = cs.get(i + 4..close).unwrap_or_default().iter().collect();
    let head = cs.get(..i).unwrap_or_default();
    let line_start = head.iter().rposition(|&c| c == '\n').map_or(0, |p| p + 1);
    let before_blank = head.iter().skip(line_start).all(|c| c.is_whitespace());
    let after = cs.iter().skip(close + 3).take_while(|&&c| c != '\n');
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
