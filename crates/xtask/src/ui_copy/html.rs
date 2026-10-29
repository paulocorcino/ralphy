//! The copy in an HTML page: static attributes, Alpine-bound expressions, text
//! nodes, and the inline scripts (handed to the JavaScript scanner).

use super::{decode_entities, js, squeeze, CopyFns, Found, Kind};

/// Elements with no closing tag.
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

/// Elements that sit inside a sentence: text on either side of one is a
/// single sentence cut in pieces.
const INLINE: &[&str] = &[
    "a", "abbr", "b", "code", "em", "i", "kbd", "small", "span", "strong",
];

/// Elements that mark a region of the page, used to name a row's area.
const LANDMARKS: &[&str] = &["nav", "aside", "main", "dialog"];

/// These mark a region only when they carry an `aria-label`: unlabelled, the
/// workbench uses them for small groups inside a panel.
const LABELLED_LANDMARKS: &[&str] = &["section", "header", "footer"];

/// Class names the workbench gives to a whole panel.
const AREA_CLASS_SUFFIXES: &[&str] = &["-view", "-tab", "-gate"];

struct Attr {
    name: String,
    value: String,
    line: usize,
}

pub(super) fn scan(src: &str, fns: CopyFns, out: &mut Vec<Found>) {
    let cs: Vec<char> = src.chars().collect();
    let mut line = 1;
    let mut i = 0;
    // Open elements: tag name and, for a landmark, the area it names.
    let mut stack: Vec<(String, Option<String>)> = Vec::new();
    let mut text = String::new();
    let mut text_line = 0;
    // Text was shown since the last element that is not inline.
    let mut in_sentence = false;

    while i < cs.len() {
        if !opens_tag(&cs, i) {
            gather_text(cs[i], line, &mut text, &mut text_line);
            if cs[i] == '\n' {
                line += 1;
            }
            i += 1;
            continue;
        }
        let shown = flush_text(&mut text, text_line, &stack, in_sentence, out);

        if let Some(end) = skip_declaration(&cs, i, &mut line) {
            i = end;
        } else if cs[i + 1] == '/' {
            let (name, end) = close_tag(&cs, i, &mut line, &mut stack);
            in_sentence = continues_sentence(&name, &[], in_sentence, shown);
            i = end;
        } else {
            let (end, continues, raw_text) =
                start_tag(&cs, i, &mut line, &mut stack, out, in_sentence || shown);
            in_sentence = continues;
            i = end;
            if let Some((name, attrs)) = raw_text {
                i = raw_text_body(&cs, i, &name, &attrs, &mut line, fns, out);
            }
        }
    }
    flush_text(&mut text, text_line, &stack, in_sentence, out);
}

/// Skip the comment or the declaration (`<!DOCTYPE …>`) at `i`. Returns the
/// index after it, or `None` when `i` starts neither.
fn skip_declaration(cs: &[char], i: usize, line: &mut usize) -> Option<usize> {
    if starts_at(cs, i, "<!--") {
        Some(skip_past(cs, i + 4, "-->", line))
    } else if starts_at(cs, i, "<!") {
        Some(skip_past(cs, i + 2, ">", line))
    } else {
        None
    }
}

/// Read the start tag at `i`, book its attributes and open the element.
/// `carry` is whether text was shown in the sentence the tag may continue.
/// Returns the index after the tag, whether text after it goes on with the
/// sentence, and, for a `<script>` or `<style>`, its name and attributes: its
/// body follows and is not markup.
fn start_tag(
    cs: &[char],
    i: usize,
    line: &mut usize,
    stack: &mut Vec<(String, Option<String>)>,
    out: &mut Vec<Found>,
    carry: bool,
) -> (usize, bool, Option<(String, Vec<Attr>)>) {
    let (name, attrs, self_closing, end) = read_tag(cs, i + 1, line);
    let continues = continues_sentence(&name, &attrs, carry, false);
    let own = book_attributes(&name, &attrs, stack, out);
    if name == "script" || name == "style" {
        return (end, continues, Some((name, attrs)));
    }
    if !self_closing && !VOID.contains(&name.as_str()) {
        stack.push((name, own));
    }
    (end, continues, None)
}

fn starts_at(cs: &[char], i: usize, s: &str) -> bool {
    s.chars()
        .enumerate()
        .all(|(k, sc)| cs.get(i + k) == Some(&sc))
}

/// Does the `<` at `i` open a start tag, an end tag, a comment or a declaration?
fn opens_tag(cs: &[char], i: usize) -> bool {
    cs[i] == '<'
        && cs
            .get(i + 1)
            .is_some_and(|n| n.is_ascii_alphabetic() || *n == '/' || *n == '!')
}

/// Add the character `c`, on page line `line`, to the text node being gathered.
/// `text_line` is the line of its first visible character.
fn gather_text(c: char, line: usize, text: &mut String, text_line: &mut usize) {
    if c != '\n' && !c.is_whitespace() && text.trim().is_empty() {
        *text_line = line;
    }
    text.push(c);
}

/// Does text go on with the sentence an element leaves open? Only an inline
/// element does, and `<span x-text="n"></span> rows`: the bound value starts the
/// sentence the next text goes on with.
fn continues_sentence(name: &str, attrs: &[Attr], in_sentence: bool, shown: bool) -> bool {
    INLINE.contains(&name) && (in_sentence || shown || attrs.iter().any(|a| a.name == "x-text"))
}

/// Read the end tag at `i`, close the element it names, and return that name
/// in lowercase with the index after the tag.
fn close_tag(
    cs: &[char],
    i: usize,
    line: &mut usize,
    stack: &mut Vec<(String, Option<String>)>,
) -> (String, usize) {
    let end = skip_past(cs, i + 2, ">", line);
    // `end` is past the `>`, or the end of input when there is none.
    let close = if cs.get(end.wrapping_sub(1)) == Some(&'>') {
        end - 1
    } else {
        end
    };
    let name: String = cs[i + 2..close.max(i + 2)]
        .iter()
        .collect::<String>()
        .trim()
        .to_ascii_lowercase();
    if let Some(at) = stack.iter().rposition(|(n, _)| *n == name) {
        stack.truncate(at);
    }
    (name, end)
}

/// Book the attributes of a start tag. Returns the area the tag itself names,
/// if it is a landmark: a landmark's own attributes belong to that area.
fn book_attributes(
    name: &str,
    attrs: &[Attr],
    stack: &[(String, Option<String>)],
    out: &mut Vec<Found>,
) -> Option<String> {
    let own = landmark(name, attrs);
    let area = own.clone().or_else(|| area_of(stack));
    for attr in attrs {
        attribute(attr, area.as_deref(), out);
    }
    own
}

/// Read the body of a `<script>` or `<style>` that opened at `i`, and return
/// the index after its end tag. Only a script with no `src` is copy.
fn raw_text_body(
    cs: &[char],
    i: usize,
    name: &str,
    attrs: &[Attr],
    line: &mut usize,
    fns: CopyFns,
    out: &mut Vec<Found>,
) -> usize {
    let close = format!("</{name}");
    let body_line = *line;
    let body_end = find(cs, i, &close).unwrap_or(cs.len());
    let body: String = cs[i..body_end].iter().collect();
    if name == "script" && !attrs.iter().any(|a| a.name == "src") {
        js::scan(&body, body_line, fns, out);
    }
    *line += body.matches('\n').count();
    skip_past(cs, body_end, ">", line)
}

/// Book the text gathered so far as one row. Returns whether it was one.
fn flush_text(
    text: &mut String,
    line: usize,
    stack: &[(String, Option<String>)],
    continues: bool,
    out: &mut Vec<Found>,
) -> bool {
    let raw = squeeze(text);
    text.clear();
    let decoded = decode_entities(&raw);
    if !decoded.chars().any(char::is_alphabetic) {
        return false;
    }
    out.push(Found {
        text: decoded.clone(),
        line,
        kind: Kind::Text,
        concatenated: false,
        area: area_of(stack),
        fragments: vec![decoded, raw],
        anchor: Some(">".to_string()),
        continues,
    });
    true
}

fn attribute(attr: &Attr, area: Option<&str>, out: &mut Vec<Found>) {
    let bound = attr
        .name
        .strip_prefix("x-bind:")
        .or_else(|| attr.name.strip_prefix(':'));
    let start = out.len();
    match (attr.name.as_str(), bound) {
        ("title" | "aria-label" | "placeholder", _) => {
            let text = squeeze(&decode_entities(&attr.value));
            if text.chars().any(char::is_alphabetic) {
                let kind = match attr.name.as_str() {
                    "title" => Kind::Title,
                    "aria-label" => Kind::AriaLabel,
                    _ => Kind::Placeholder,
                };
                out.push(Found {
                    fragments: vec![text.clone(), attr.value.clone()],
                    anchor: Some(format!("{}=", attr.name)),
                    continues: false,
                    text,
                    line: attr.line,
                    kind,
                    concatenated: false,
                    area: None,
                });
            }
        }
        (_, Some("title" | "aria-label" | "placeholder")) | ("x-text", _) => {
            js::expression(&decode_entities(&attr.value), attr.line, Kind::Alpine, out);
            for found in &mut out[start..] {
                found.anchor = Some(format!("{}=\"", attr.name));
            }
        }
        _ => {}
    }
    for found in &mut out[start..] {
        found.area = area.map(str::to_string);
    }
}

/// The area a landmark element names: its `aria-label`, else `tag.class`
/// with the last (most specific) class.
fn landmark(name: &str, attrs: &[Attr]) -> Option<String> {
    let get = |n: &str| attrs.iter().find(|a| a.name == n).map(|a| a.value.as_str());
    let class = get("class").unwrap_or("");
    let label = get("aria-label");
    let region = class
        .split_whitespace()
        .any(|c| AREA_CLASS_SUFFIXES.iter().any(|s| c.ends_with(s)));
    let dialog = matches!(get("role"), Some("dialog" | "alertdialog"));
    let labelled = LABELLED_LANDMARKS.contains(&name) && label.is_some();
    if !(LANDMARKS.contains(&name) || labelled || dialog || region) {
        return None;
    }
    if let Some(label) = label {
        return Some(label.to_string());
    }
    Some(match class.split_whitespace().last() {
        Some(last) => format!("{name}.{last}"),
        None => name.to_string(),
    })
}

fn area_of(stack: &[(String, Option<String>)]) -> Option<String> {
    stack.iter().rev().find_map(|(_, area)| area.clone())
}

/// Read a start tag whose name begins at `i`. Returns the lowercase name, its
/// attributes, whether it ends in `/>`, and the index after the `>`.
fn read_tag(cs: &[char], i: usize, line: &mut usize) -> (String, Vec<Attr>, bool, usize) {
    let (name, mut i) = read_tag_name(cs, i);
    let mut attrs = Vec::new();
    let mut self_closing = false;
    while i < cs.len() {
        let c = cs[i];
        if c == '\n' {
            *line += 1;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '>' {
            return (name, attrs, self_closing, i + 1);
        }
        if c == '/' {
            self_closing = true;
            i += 1;
            continue;
        }
        self_closing = false;
        let (attr, next) = read_attr(cs, i, line);
        attrs.push(attr);
        i = next;
    }
    (name, attrs, self_closing, i)
}

/// The lowercase tag name at `i`, and the index after it.
fn read_tag_name(cs: &[char], mut i: usize) -> (String, usize) {
    let mut name = String::new();
    while i < cs.len() && !cs[i].is_whitespace() && cs[i] != '>' && cs[i] != '/' {
        name.push(cs[i].to_ascii_lowercase());
        i += 1;
    }
    (name, i)
}

/// One attribute that starts at `i`, and the index after it.
fn read_attr(cs: &[char], i: usize, line: &mut usize) -> (Attr, usize) {
    let (name, i) = read_attr_name(cs, i);
    let (value, value_line, i) = read_attr_value(cs, i, line);
    let attr = Attr {
        name: name.to_ascii_lowercase(),
        value,
        line: value_line,
    };
    (attr, i)
}

fn read_attr_name(cs: &[char], mut i: usize) -> (String, usize) {
    let mut name = String::new();
    while i < cs.len() && !cs[i].is_whitespace() && !matches!(cs[i], '=' | '>') {
        name.push(cs[i]);
        i += 1;
    }
    (name, i)
}

/// The value after an attribute name, quoted or not, with the line it starts
/// on and the index after it. An attribute with no `=` has an empty value.
fn read_attr_value(cs: &[char], mut i: usize, line: &mut usize) -> (String, usize, usize) {
    let mut value = String::new();
    let mut value_line = *line;
    if cs.get(i) == Some(&'=') {
        i += 1;
        match cs.get(i) {
            Some(&q @ ('"' | '\'')) => {
                i += 1;
                value_line = *line;
                (value, i) = read_quoted(cs, i, q, line);
                i += 1;
            }
            _ => {
                while i < cs.len() && !cs[i].is_whitespace() && cs[i] != '>' {
                    value.push(cs[i]);
                    i += 1;
                }
            }
        }
    }
    (value, value_line, i)
}

/// The text from `i` up to the quote `q`, and the index of that quote (or the
/// end of input).
fn read_quoted(cs: &[char], mut i: usize, q: char, line: &mut usize) -> (String, usize) {
    let mut value = String::new();
    while i < cs.len() && cs[i] != q {
        if cs[i] == '\n' {
            *line += 1;
        }
        value.push(cs[i]);
        i += 1;
    }
    (value, i)
}

fn find(cs: &[char], from: usize, needle: &str) -> Option<usize> {
    let n: Vec<char> = needle.chars().collect();
    (from..cs.len()).find(|&k| {
        cs.get(k..k + n.len())
            .is_some_and(|w| w.iter().zip(&n).all(|(a, b)| a.eq_ignore_ascii_case(b)))
    })
}

/// Index after the next `needle` at or after `from`, counting newlines passed.
fn skip_past(cs: &[char], from: usize, needle: &str, line: &mut usize) -> usize {
    let end = find(cs, from, needle).map_or(cs.len(), |k| k + needle.chars().count());
    *line += cs[from.min(end)..end]
        .iter()
        .filter(|&&c| c == '\n')
        .count();
    end
}
