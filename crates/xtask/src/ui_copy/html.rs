//! The copy in an HTML page: static attributes, Alpine-bound expressions, text
//! nodes, and the inline scripts (handed to the JavaScript scanner).

use super::{decode_entities, js, squeeze, Found, Kind};

/// Elements with no closing tag.
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
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

pub(super) fn scan(src: &str, out: &mut Vec<Found>) {
    let cs: Vec<char> = src.chars().collect();
    let mut line = 1;
    let mut i = 0;
    // Open elements: tag name and, for a landmark, the area it names.
    let mut stack: Vec<(String, Option<String>)> = Vec::new();
    let mut text = String::new();
    let mut text_line = 0;

    while i < cs.len() {
        let c = cs[i];
        let starts = |s: &str| {
            s.chars()
                .enumerate()
                .all(|(k, sc)| cs.get(i + k) == Some(&sc))
        };
        let tag_start = c == '<'
            && cs
                .get(i + 1)
                .is_some_and(|n| n.is_ascii_alphabetic() || *n == '/' || *n == '!');
        if !tag_start {
            if c == '\n' {
                line += 1;
            } else if !c.is_whitespace() && text.trim().is_empty() {
                text_line = line;
            }
            text.push(c);
            i += 1;
            continue;
        }
        flush_text(&mut text, text_line, &stack, out);

        if starts("<!--") {
            i = skip_past(&cs, i + 4, "-->", &mut line);
        } else if starts("<!") {
            i = skip_past(&cs, i + 2, ">", &mut line);
        } else if cs[i + 1] == '/' {
            let end = skip_past(&cs, i + 2, ">", &mut line);
            let name: String = cs[i + 2..end.saturating_sub(1)]
                .iter()
                .collect::<String>()
                .trim()
                .to_ascii_lowercase();
            if let Some(at) = stack.iter().rposition(|(n, _)| *n == name) {
                stack.truncate(at);
            }
            i = end;
        } else {
            let (name, attrs, self_closing, end) = read_tag(&cs, i + 1, &mut line);
            i = end;
            // A landmark's own attributes belong to the area it names.
            let own = landmark(&name, &attrs);
            let area = own.clone().or_else(|| area_of(&stack));
            for attr in &attrs {
                attribute(attr, area.as_deref(), out);
            }
            if name == "script" || name == "style" {
                let close = format!("</{name}");
                let body_line = line;
                let body_end = find(&cs, i, &close).unwrap_or(cs.len());
                let body: String = cs[i..body_end].iter().collect();
                if name == "script" && !attrs.iter().any(|a| a.name == "src") {
                    js::scan(&body, body_line, out);
                }
                line += body.matches('\n').count();
                i = skip_past(&cs, body_end, ">", &mut line);
            } else if !self_closing && !VOID.contains(&name.as_str()) {
                stack.push((name, own));
            }
        }
    }
    flush_text(&mut text, text_line, &stack, out);
}

fn flush_text(
    text: &mut String,
    line: usize,
    stack: &[(String, Option<String>)],
    out: &mut Vec<Found>,
) {
    let raw = squeeze(text);
    text.clear();
    let decoded = decode_entities(&raw);
    if !decoded.chars().any(char::is_alphabetic) {
        return;
    }
    out.push(Found {
        text: decoded.clone(),
        line,
        kind: Kind::Text,
        concatenated: false,
        area: area_of(stack),
        fragments: vec![decoded, raw],
        anchor: Some(">".to_string()),
    });
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
fn read_tag(cs: &[char], mut i: usize, line: &mut usize) -> (String, Vec<Attr>, bool, usize) {
    let mut name = String::new();
    while i < cs.len() && !cs[i].is_whitespace() && cs[i] != '>' && cs[i] != '/' {
        name.push(cs[i].to_ascii_lowercase());
        i += 1;
    }
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
        let mut attr = String::new();
        while i < cs.len() && !cs[i].is_whitespace() && !matches!(cs[i], '=' | '>') {
            attr.push(cs[i]);
            i += 1;
        }
        let mut value = String::new();
        let mut value_line = *line;
        if cs.get(i) == Some(&'=') {
            i += 1;
            match cs.get(i) {
                Some(&q @ ('"' | '\'')) => {
                    i += 1;
                    value_line = *line;
                    while i < cs.len() && cs[i] != q {
                        if cs[i] == '\n' {
                            *line += 1;
                        }
                        value.push(cs[i]);
                        i += 1;
                    }
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
        attrs.push(Attr {
            name: attr.to_ascii_lowercase(),
            value,
            line: value_line,
        });
    }
    (name, attrs, self_closing, i)
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
