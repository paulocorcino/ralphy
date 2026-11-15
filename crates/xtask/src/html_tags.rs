//! A reader of the tags and attributes of an HTML document.

pub(crate) struct Attr {
    pub(crate) name: String,
    pub(crate) value: Option<String>,
    pub(crate) line: usize,
}

/// Index one past the comment (`<!-- … -->`) or the declaration
/// (`<!DOCTYPE …>`) that starts at `i`.
pub(crate) fn skip_declaration(cs: &[char], i: usize, line: &mut usize) -> usize {
    let end = if starts(cs, i, "<!--") { "-->" } else { ">" };
    skip_past(cs, i, end, line)
}

pub(crate) fn starts(cs: &[char], i: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(k, c)| cs.get(i + k) == Some(&c))
}

/// Index one past `needle` at or after `from`, counting lines; the end of
/// input when it is absent.
pub(crate) fn skip_past(cs: &[char], from: usize, needle: &str, line: &mut usize) -> usize {
    let mut i = from;
    while let Some(&c) = cs.get(i) {
        if starts(cs, i, needle) {
            return i + needle.chars().count();
        }
        if c == '\n' {
            *line += 1;
        }
        i += 1;
    }
    i
}

/// A tag whose name starts at `i`: its lowercase name, its attributes, whether
/// it closes itself, and the index one past its `>`.
pub(crate) fn read_tag(
    cs: &[char],
    i: usize,
    line: &mut usize,
) -> (String, Vec<Attr>, bool, usize) {
    let (name, mut i) = tag_name(cs, i);
    let mut attrs = Vec::new();
    let mut self_closing = false;
    while let Some(&c) = cs.get(i) {
        if c.is_whitespace() {
            if c == '\n' {
                *line += 1;
            }
            i += 1;
            continue;
        }
        if c == '>' {
            i += 1;
            break;
        }
        if starts(cs, i, "/>") {
            self_closing = true;
            i += 2;
            break;
        }
        let (attr, next) = read_attr(cs, i, line);
        attrs.extend(attr);
        i = next;
    }
    (name, attrs, self_closing, i)
}

/// The attribute at `i`, on the line it starts on, and the index after it.
/// `None` when no name starts at `i`: the character there is skipped.
fn read_attr(cs: &[char], i: usize, line: &mut usize) -> (Option<Attr>, usize) {
    let attr_line = *line;
    let (name, after_name) = attr_name(cs, i);
    let (value, after_value) = attr_value(cs, after_name, line);
    if name.is_empty() {
        return (None, after_value + 1);
    }
    let attr = Attr {
        name,
        value,
        line: attr_line,
    };
    (Some(attr), after_value)
}

/// The tag name at `i`, made of ASCII letters, digits and `-`, in lowercase,
/// and the index after it.
fn tag_name(cs: &[char], i: usize) -> (String, usize) {
    let end = end_where(cs, i, |_, c| !(c.is_ascii_alphanumeric() || c == '-'));
    let name = span(cs, i, end)
        .iter()
        .map(char::to_ascii_lowercase)
        .collect();
    (name, end)
}

/// The attribute name at `i` as written, and the index after it. It ends at
/// white space, `=`, `>` or `/>`.
fn attr_name(cs: &[char], i: usize) -> (String, usize) {
    let end = end_where(cs, i, |k, c| {
        c.is_whitespace() || matches!(c, '=' | '>') || starts(cs, k, "/>")
    });
    (span(cs, i, end).iter().collect(), end)
}

/// The value after an attribute name, which starts at `i` with `=`, and the
/// index after it. With no `=` there is no value.
fn attr_value(cs: &[char], i: usize, line: &mut usize) -> (Option<String>, usize) {
    if cs.get(i) != Some(&'=') {
        return (None, i);
    }
    let (value, end) = if matches!(cs.get(i + 1), Some('"' | '\'')) {
        quoted_value(cs, i + 1, line)
    } else {
        bare_value(cs, i + 1)
    };
    (Some(value), end)
}

/// The text between the quote at `i` and the next same quote, counting its
/// lines, and the index one past that quote. With no closing quote, the
/// value runs to the end of input.
fn quoted_value(cs: &[char], i: usize, line: &mut usize) -> (String, usize) {
    let quote = cs.get(i).copied().unwrap_or('"');
    let close = end_where(cs, i + 1, |_, c| c == quote);
    let value: String = span(cs, i + 1, close).iter().collect();
    *line += value.matches('\n').count();
    (value, close + 1)
}

/// An unquoted value at `i`: it ends at white space or `>`.
fn bare_value(cs: &[char], i: usize) -> (String, usize) {
    let end = end_where(cs, i, |_, c| c.is_whitespace() || c == '>');
    (span(cs, i, end).iter().collect(), end)
}

/// The first index at or after `from` whose character, given with its index,
/// satisfies `stop`; the end of input when none does.
fn end_where(cs: &[char], from: usize, stop: impl Fn(usize, char) -> bool) -> usize {
    (from..cs.len())
        .find(|&k| cs.get(k).is_some_and(|&c| stop(k, c)))
        .unwrap_or(cs.len())
}

fn span(cs: &[char], from: usize, to: usize) -> &[char] {
    cs.get(from..to).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs_of(html: &str) -> Vec<(String, Option<String>, usize)> {
        let cs: Vec<char> = html.chars().collect();
        let mut line = 1;
        let mut i = 0;
        while !starts(&cs, i, "<a") {
            i = if starts(&cs, i, "<!") {
                skip_declaration(&cs, i, &mut line)
            } else {
                if cs[i] == '\n' {
                    line += 1;
                }
                i + 1
            };
        }
        let (_, attrs, _, _) = read_tag(&cs, i + 1, &mut line);
        attrs
            .into_iter()
            .map(|a| (a.name, a.value, a.line))
            .collect()
    }

    #[test]
    fn reads_attributes_with_their_lines() {
        let html = "<a x-data=\"one\ntwo\" id=bare\n hidden\n>";
        let got = attrs_of(html);
        let owned = |n: &str, v: Option<&str>, l| (n.to_owned(), v.map(str::to_owned), l);
        assert_eq!(
            got,
            vec![
                owned("x-data", Some("one\ntwo"), 1),
                owned("id", Some("bare"), 2),
                owned("hidden", None, 3),
            ]
        );
        let after_comment = attrs_of("<!-- one\ntwo -->\n<a href=\"x\">");
        assert_eq!(after_comment, vec![owned("href", Some("x"), 3)]);
    }
}
