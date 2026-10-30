//! The bindings of `index.html`: the Alpine attributes in the scope of
//! `x-data="shell"`, and the members each one names.

use std::collections::BTreeSet;

use super::lex::{lex, Tok, Token};
use super::{close_of, tok_is, Binding};

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

/// The free identifiers an Alpine expression reads, and the flag named by the
/// first argument of `scrim('flag', …)`.
fn expression_names(expr: &str) -> Vec<String> {
    let toks = lex(expr, 1);
    let declared = declared_names(&toks);
    free_names(&toks, &declared)
}

/// The names the expression declares itself: arrow parameters, and the name
/// after `let`, `const` or `var`.
fn declared_names(toks: &[Token]) -> BTreeSet<String> {
    let mut declared = BTreeSet::new();
    for (i, t) in toks.iter().enumerate() {
        if t.is("=>") {
            declared.extend(arrow_params(toks, i));
        }
        if matches!(t.ident(), Some("let" | "const" | "var")) {
            declared.extend(toks.get(i + 1).and_then(Token::ident).map(str::to_string));
        }
    }
    declared
}

/// The parameter names of the arrow whose `=>` is at `i`: one bare name, or the
/// identifiers between the parentheses before it.
fn arrow_params(toks: &[Token], i: usize) -> Vec<String> {
    if let Some(p) = i.checked_sub(1).and_then(|p| toks[p].ident()) {
        return vec![p.to_string()];
    }
    if i > 0 && tok_is(toks, i - 1, ")") {
        let open = (0..i - 1).rev().find(|&k| close_of(toks, k) == i - 1);
        return open
            .map_or(&toks[0..0], |o| &toks[o..i])
            .iter()
            .filter_map(|p| p.ident().map(str::to_string))
            .collect();
    }
    Vec::new()
}

/// The identifiers of `toks` that are free reads (see [`is_free_read`]), plus
/// the flag of `scrim('flag', …)`.
fn free_names(toks: &[Token], declared: &BTreeSet<String>) -> Vec<String> {
    let mut names = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        names.extend(scrim_flag(toks, i));
        if let Some(id) = t.ident() {
            if is_free_read(toks, i, declared) {
                names.push(id.to_string());
            }
        }
    }
    names
}

/// The first segment of the flag named by the first argument of `scrim(…)`,
/// when the token at `i` is that `scrim`.
fn scrim_flag(toks: &[Token], i: usize) -> Option<String> {
    if toks[i].ident() != Some("scrim") || !tok_is(toks, i + 1, "(") {
        return None;
    }
    match toks.get(i + 2).map(|t| &t.tok) {
        Some(Tok::Str(flag)) => Some(flag.split('.').next().unwrap_or("").to_string()),
        _ => None,
    }
}

/// Is the identifier at `i` a read of a member: not `declared`, not a property
/// after `.` or `?.`, and not an object key.
fn is_free_read(toks: &[Token], i: usize, declared: &BTreeSet<String>) -> bool {
    let at = |i: usize, p: &str| tok_is(toks, i, p);
    let after_dot = i > 0 && (at(i - 1, ".") || at(i - 1, "?."));
    let key = at(i + 1, ":") && (i == 0 || at(i - 1, "{") || at(i - 1, ","));
    let declared_here = toks[i].ident().is_some_and(|id| declared.contains(id));
    !after_dot && !key && !declared_here
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
