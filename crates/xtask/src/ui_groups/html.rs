//! The bindings of `index.html`: the Alpine attributes in the scope of
//! `x-data="shell"`, and the members each one names.

use std::collections::BTreeSet;

use super::{expression_names, Binding};

/// Alpine attributes that hold no expression of the component's state.
const NOT_BINDINGS: &[&str] = &[
    "x-data",
    "x-ref",
    "x-cloak",
    "x-ignore",
    "x-teleport",
    "x-id",
];

struct Open {
    tag: String,
    in_shell: bool,
    vars: Vec<String>,
}

/// Every binding of `index.html` in the scope of `x-data="shell"`, with the
/// members of `known` it names.
pub(super) fn scan_bindings(html: &str, known: &BTreeSet<String>) -> Vec<Binding> {
    let cs: Vec<char> = html.chars().collect();
    let mut out = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut line = 1;
    let mut i = 0;
    while i < cs.len() {
        if starts(&cs, i, "<!") {
            i = skip_declaration(&cs, i, &mut line);
        } else if starts(&cs, i, "</") {
            i = end_tag(&cs, i, &mut line, &mut stack);
        } else if cs[i] == '<' && cs.get(i + 1).is_some_and(char::is_ascii_alphabetic) {
            i = start_tag(&cs, i, &mut line, &mut stack, known, &mut out);
        } else {
            if cs[i] == '\n' {
                line += 1;
            }
            i += 1;
        }
    }
    out
}

/// Index one past the comment (`<!-- … -->`) or the declaration
/// (`<!DOCTYPE …>`) that starts at `i`.
fn skip_declaration(cs: &[char], i: usize, line: &mut usize) -> usize {
    let end = if starts(cs, i, "<!--") { "-->" } else { ">" };
    skip_past(cs, i, end, line)
}

/// Read the end tag at `i`, close the open element it names with all the
/// elements inside it, and return the index after the tag.
fn end_tag(cs: &[char], i: usize, line: &mut usize, stack: &mut Vec<Open>) -> usize {
    let (tag, _, _, end) = read_tag(cs, i + 2, line);
    if let Some(k) = stack.iter().rposition(|o| o.tag == tag) {
        stack.truncate(k);
    }
    end
}

/// Read the start tag at `i`, add its bindings to `out` when it is in the
/// scope of `shell`, and return the index after it. The body of a `script`
/// or `style` is skipped; an element that has a body is pushed on `stack`.
fn start_tag(
    cs: &[char],
    i: usize,
    line: &mut usize,
    stack: &mut Vec<Open>,
    known: &BTreeSet<String>,
    out: &mut Vec<Binding>,
) -> usize {
    const VOID: &[&str] = &[
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source",
        "track", "wbr",
    ];
    let (tag, attrs, self_closing, end) = read_tag(cs, i + 1, line);
    let open = scope(tag, &attrs, stack.last());
    if open.in_shell {
        out.extend(attrs.iter().filter_map(|a| binding(a, &open.vars, known)));
    }
    if open.tag == "script" || open.tag == "style" {
        let body_end = skip_past(cs, end, &format!("</{}", open.tag), line);
        return skip_past(cs, body_end, ">", line);
    }
    if !self_closing && !VOID.contains(&open.tag.as_str()) {
        stack.push(open);
    }
    end
}

/// The scope a start tag opens. It is in `shell` when its own `x-data` is
/// `shell`, or when it has no `x-data` and its parent is. Its variables are
/// those of its parent and those of its own `x-for`.
fn scope(tag: String, attrs: &[Attr], parent: Option<&Open>) -> Open {
    let x_data = attrs.iter().find(|a| a.name == "x-data");
    let in_shell = match x_data {
        Some(a) => a.value.as_deref().map(str::trim) == Some("shell"),
        None => parent.is_some_and(|p| p.in_shell),
    };
    let mut vars = parent.map(|p| p.vars.clone()).unwrap_or_default();
    if let Some(f) = attrs.iter().find(|a| a.name == "x-for") {
        vars.extend(for_vars(f.value.as_deref().unwrap_or("")));
    }
    Open {
        tag,
        in_shell,
        vars,
    }
}

struct Attr {
    name: String,
    value: Option<String>,
    line: usize,
}

fn binding(a: &Attr, vars: &[String], known: &BTreeSet<String>) -> Option<Binding> {
    let n = a.name.as_str();
    if !(n.starts_with("x-") || n.starts_with('@') || n.starts_with(':')) {
        return None;
    }
    let base = n.split(['.', ':']).next().unwrap_or(n);
    if NOT_BINDINGS.contains(&base) || base.starts_with("x-transition") {
        return None;
    }
    let value = a.value.as_deref().unwrap_or("").trim();
    if value.is_empty() {
        return None;
    }
    let expr = if base == "x-for" {
        value
            .split_once(" in ")
            .or_else(|| value.split_once(" of "))
            .map_or(value, |(_, list)| list)
    } else {
        value
    };
    let names = expression_names(expr)
        .into_iter()
        .filter(|id| !id.starts_with('$') && !vars.contains(id) && known.contains(id))
        .collect();
    Some(Binding {
        line: a.line,
        names,
    })
}

/// The variables an `x-for` value declares: `item in list`, `(item, i) in list`.
fn for_vars(value: &str) -> Vec<String> {
    let Some((head, _)) = value
        .split_once(" in ")
        .or_else(|| value.split_once(" of "))
    else {
        return Vec::new();
    };
    head.trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

fn starts(cs: &[char], i: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(k, c)| cs.get(i + k) == Some(&c))
}

/// Index one past `needle` at or after `from`, counting lines; the end of
/// input when it is absent.
fn skip_past(cs: &[char], from: usize, needle: &str, line: &mut usize) -> usize {
    let mut i = from;
    while i < cs.len() {
        if starts(cs, i, needle) {
            return i + needle.chars().count();
        }
        if cs[i] == '\n' {
            *line += 1;
        }
        i += 1;
    }
    i
}

/// A tag whose name starts at `i`: its lowercase name, its attributes, whether
/// it closes itself, and the index one past its `>`.
fn read_tag(cs: &[char], i: usize, line: &mut usize) -> (String, Vec<Attr>, bool, usize) {
    let (name, mut i) = tag_name(cs, i);
    let mut attrs = Vec::new();
    let mut self_closing = false;
    while i < cs.len() {
        if cs[i].is_whitespace() {
            if cs[i] == '\n' {
                *line += 1;
            }
            i += 1;
            continue;
        }
        if cs[i] == '>' {
            i += 1;
            break;
        }
        if starts(cs, i, "/>") {
            self_closing = true;
            i += 2;
            break;
        }
        let attr_line = *line;
        let (attr, after_name) = attr_name(cs, i);
        let (value, after_value) = attr_value(cs, after_name, line);
        i = after_value;
        if attr.is_empty() {
            i += 1;
        } else {
            attrs.push(Attr {
                name: attr,
                value,
                line: attr_line,
            });
        }
    }
    (name, attrs, self_closing, i)
}

/// The tag name at `i`, made of ASCII letters, digits and `-`, in lowercase,
/// and the index after it.
fn tag_name(cs: &[char], i: usize) -> (String, usize) {
    let end = (i..cs.len())
        .find(|&k| !(cs[k].is_ascii_alphanumeric() || cs[k] == '-'))
        .unwrap_or(cs.len());
    let name = cs[i..end].iter().map(char::to_ascii_lowercase).collect();
    (name, end)
}

/// The attribute name at `i` as written, and the index after it. It ends at
/// white space, `=`, `>` or `/>`.
fn attr_name(cs: &[char], i: usize) -> (String, usize) {
    let end = (i..cs.len())
        .find(|&k| cs[k].is_whitespace() || matches!(cs[k], '=' | '>') || starts(cs, k, "/>"))
        .unwrap_or(cs.len());
    (cs[i..end].iter().collect(), end)
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
    let quote = cs[i];
    let close = (i + 1..cs.len())
        .find(|&k| cs[k] == quote)
        .unwrap_or(cs.len());
    let value: String = cs[i + 1..close].iter().collect();
    *line += value.matches('\n').count();
    (value, close + 1)
}

/// An unquoted value at `i`: it ends at white space or `>`.
fn bare_value(cs: &[char], i: usize) -> (String, usize) {
    let end = (i..cs.len())
        .find(|&k| cs[k].is_whitespace() || cs[k] == '>')
        .unwrap_or(cs.len());
    (cs[i..end].iter().collect(), end)
}
