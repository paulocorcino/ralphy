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
    const VOID: &[&str] = &[
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source",
        "track", "wbr",
    ];
    let cs: Vec<char> = html.chars().collect();
    let mut out = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut line = 1;
    let mut i = 0;
    while i < cs.len() {
        if starts(&cs, i, "<!--") {
            i = skip_past(&cs, i, "-->", &mut line);
        } else if starts(&cs, i, "<!") {
            i = skip_past(&cs, i, ">", &mut line);
        } else if starts(&cs, i, "</") {
            let (tag, _, _, end) = read_tag(&cs, i + 2, &mut line);
            if let Some(k) = stack.iter().rposition(|o| o.tag == tag) {
                stack.truncate(k);
            }
            i = end;
        } else if cs[i] == '<' && cs.get(i + 1).is_some_and(char::is_ascii_alphabetic) {
            let (tag, attrs, self_closing, end) = read_tag(&cs, i + 1, &mut line);
            i = end;
            let parent = stack.last();
            let x_data = attrs.iter().find(|a| a.name == "x-data");
            let in_shell = match x_data {
                Some(a) => a.value.as_deref().map(str::trim) == Some("shell"),
                None => parent.is_some_and(|p| p.in_shell),
            };
            let mut vars = parent.map(|p| p.vars.clone()).unwrap_or_default();
            if let Some(f) = attrs.iter().find(|a| a.name == "x-for") {
                vars.extend(for_vars(f.value.as_deref().unwrap_or("")));
            }
            if in_shell {
                for a in &attrs {
                    if let Some(b) = binding(a, &vars, known) {
                        out.push(b);
                    }
                }
            }
            if tag == "script" || tag == "style" {
                i = skip_past(&cs, i, &format!("</{tag}"), &mut line);
                i = skip_past(&cs, i, ">", &mut line);
            } else if !self_closing && !VOID.contains(&tag.as_str()) {
                stack.push(Open {
                    tag,
                    in_shell,
                    vars,
                });
            }
        } else {
            if cs[i] == '\n' {
                line += 1;
            }
            i += 1;
        }
    }
    out
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
fn read_tag(cs: &[char], mut i: usize, line: &mut usize) -> (String, Vec<Attr>, bool, usize) {
    let mut name = String::new();
    while i < cs.len() && (cs[i].is_ascii_alphanumeric() || cs[i] == '-') {
        name.push(cs[i].to_ascii_lowercase());
        i += 1;
    }
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
        let mut attr = String::new();
        while i < cs.len()
            && !cs[i].is_whitespace()
            && !matches!(cs[i], '=' | '>')
            && !starts(cs, i, "/>")
        {
            attr.push(cs[i]);
            i += 1;
        }
        let mut value = None;
        if cs.get(i) == Some(&'=') {
            i += 1;
            let quote = cs.get(i).copied();
            let mut v = String::new();
            if matches!(quote, Some('"' | '\'')) {
                i += 1;
                while i < cs.len() && Some(cs[i]) != quote {
                    if cs[i] == '\n' {
                        *line += 1;
                    }
                    v.push(cs[i]);
                    i += 1;
                }
                i += 1;
            } else {
                while i < cs.len() && !cs[i].is_whitespace() && cs[i] != '>' {
                    v.push(cs[i]);
                    i += 1;
                }
            }
            value = Some(v);
        }
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
