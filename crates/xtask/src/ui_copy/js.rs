//! Where a JavaScript string literal reaches the screen, and how an expression
//! around it becomes one row of copy.

use super::lex::{lex, Piece, Tok, Token};
use super::{decode_entities, html, squeeze, CopyFns, Found, Kind};

/// Object keys whose value is shown: a toast's `text`, a dialog's `title`, a
/// table row's `label`, a setting's `help` and its section's `blurb`
/// (`wb-settings.js`, rendered by `x-text="it.help"` / `"sec.blurb"`).
const SHOWN_KEYS: &[&str] = &[
    "title",
    "message",
    "text",
    "label",
    "confirmLabel",
    "cancelLabel",
    "placeholder",
    "hint",
    "caption",
    "tooltip",
    "ariaLabel",
    "help",
    "blurb",
    "note",
];

/// A function whose name ends in one of these returns copy.
const HELPER_SUFFIXES: &[&str] = &[
    "Title", "Label", "Text", "Hint", "Tooltip", "Message", "Caption",
];

/// Scan one JavaScript source; `first_line` is the line of its first char.
/// `fns` names functions that return copy whatever their suffix, and
/// functions that take copy as an argument (`docs/ui-copy-rules.json`).
pub(super) fn scan(src: &str, first_line: usize, fns: CopyFns, out: &mut Vec<Found>) {
    let cs: Vec<char> = src.chars().collect();
    let toks = lex(src, first_line);
    let sc = Scan {
        cs: &cs,
        toks: &toks,
        fns,
    };
    // One entry per open bracket: the call that owns it (for `(`, and for a
    // `{` passed straight to a call) and whether it is a helper's body.
    let mut stack: Vec<(&str, Option<&str>, bool)> = Vec::new();
    let mut helper_body_at: Option<usize> = None;

    for (i, t) in toks.iter().enumerate() {
        match &t.tok {
            Tok::Punct(p @ ("(" | "[" | "{")) => {
                let owner = match *p {
                    "(" => sc.ident_at(i.wrapping_sub(1)),
                    "{" if sc.is_at(i.wrapping_sub(1), "(") => stack.last().and_then(|s| s.1),
                    _ => None,
                };
                let helper = helper_body_at == Some(i);
                stack.push((p, owner, helper));
            }
            Tok::Punct(")" | "]" | "}") => {
                stack.pop();
            }
            _ => {}
        }
        let Some(name) = t.ident() else { continue };
        let prev_dot = sc.is_at(i.wrapping_sub(1), ".");
        let after_fn = sc.ident_at(i.wrapping_sub(1)) == Some("function");
        let calls = sc.is_at(i + 1, "(");

        let helper_name = HELPER_SUFFIXES.iter().any(|s| name.ends_with(s))
            || fns.helpers.iter().any(|h| h == name);
        if helper_name && calls && !prev_dot {
            let close = sc.close_of(i + 1);
            if sc.is_at(close + 1, "{") {
                helper_body_at = Some(close + 1);
            }
        }

        match name {
            key if SHOWN_KEYS.contains(&key)
                && sc.is_at(i + 1, ":")
                && (sc.is_at(i.wrapping_sub(1), "{") || sc.is_at(i.wrapping_sub(1), ","))
                && stack.last().is_some_and(|s| s.0 == "{") =>
            {
                let kind = match stack.last().and_then(|s| s.1) {
                    Some("toast") => Kind::Toast,
                    Some("askConfirm" | "askNotice" | "askPrompt") => Kind::Confirm,
                    _ => Kind::Property,
                };
                let end = sc.expr_end(i + 2, true);
                let from = out.len();
                sc.emit(i + 2, end, kind, false, false, out);
                anchor(&mut out[from..], format!("{key}: "));
            }
            "confirm" | "prompt" | "alert"
                if calls
                    && !after_fn
                    && (!prev_dot || sc.ident_at(i.wrapping_sub(2)) == Some("window")) =>
            {
                if let Some(&(a, b)) = sc.args(i + 1).first() {
                    sc.emit(a, b, Kind::Confirm, false, false, out);
                }
            }
            "textContent" | "innerText" | "innerHTML" | "title" | "placeholder" | "ariaLabel"
                if prev_dot && (sc.is_at(i + 1, "=") || sc.is_at(i + 1, "+=")) =>
            {
                let kind = match name {
                    "title" | "placeholder" | "ariaLabel" => Kind::Attribute,
                    _ => Kind::TextContent,
                };
                let end = sc.expr_end(i + 2, false);
                let from = out.len();
                sc.emit(i + 2, end, kind, name == "innerHTML", false, out);
                anchor(&mut out[from..], format!("{name} = "));
            }
            "setAttribute" if calls => {
                let args = sc.args(i + 1);
                if let [(a0, b0), (a1, b1), ..] = args[..] {
                    let named = b0 == a0 + 1
                        && matches!(&toks[a0].tok,
                            Tok::Str(s) if matches!(s.as_str(), "title" | "aria-label" | "placeholder"));
                    if let (true, Tok::Str(attr)) = (named, &toks[a0].tok) {
                        let from = out.len();
                        sc.emit(a1, b1, Kind::Attribute, false, false, out);
                        anchor(&mut out[from..], format!("\"{attr}\", "));
                    }
                }
            }
            "write" | "writeln"
                if calls && prev_dot && sc.ident_at(i.wrapping_sub(2)) == Some("term") =>
            {
                if let Some(&(a, b)) = sc.args(i + 1).first() {
                    sc.emit(a, b, Kind::TermNotice, false, false, out);
                }
            }
            "key" if calls && !prev_dot && !after_fn => {
                let args = sc.args(i + 1);
                if let [(a0, b0), (a1, b1), (a2, b2), ..] = args[..] {
                    if b0 == a0 + 1 && matches!(toks[a0].tok, Tok::Str(_)) {
                        sc.emit(a1, b1, Kind::KeyBar, true, false, out);
                        sc.emit(a2, b2, Kind::KeyBar, false, false, out);
                    }
                }
            }
            "this"
                if sc.is_at(i + 1, ".") && sc.ident_at(i + 2).is_some() && sc.is_at(i + 3, "=") =>
            {
                let end = sc.expr_end(i + 4, false);
                sc.emit(i + 4, end, Kind::State, false, true, out);
            }
            "const" if sc.ident_at(i + 1).is_some_and(screaming) && sc.is_at(i + 2, "=") => {
                let end = sc.expr_end(i + 3, false);
                let from = out.len();
                sc.emit(i + 3, end, Kind::Const, false, true, out);
                // A media query or a selector (`"(max-width: 560px)"`) has
                // spaces and is still not copy.
                let emitted = out.split_off(from);
                out.extend(
                    emitted
                        .into_iter()
                        .filter(|f| !f.text.starts_with(['(', '['])),
                );
                let named = sc.ident_at(i + 1).unwrap_or_default();
                anchor(&mut out[from..], format!("{named} = "));
            }
            call if calls
                && !after_fn
                && fns
                    .calls
                    .iter()
                    .any(|c| c.matches(call, owner(&sc, i, prev_dot))) =>
            {
                let args = sc.args(i + 1);
                let on = owner(&sc, i, prev_dot);
                for c in fns.calls.iter().filter(|c| c.matches(call, on)) {
                    if let Some(&(a, b)) = args.get(c.arg) {
                        sc.emit(a, b, Kind::Call, false, false, out);
                    }
                }
            }
            "return" if stack.iter().any(|s| s.2) => {
                let end = sc.expr_end(i + 1, false);
                sc.emit(i + 1, end, Kind::Helper, false, false, out);
            }
            _ => {}
        }
    }
}

/// `NEEDS_REPO`: the name the UI gives a constant that holds copy.
/// The identifier before the dot of a method call at `i`: `None` for a bare
/// call, `Some("")` when the dot follows something that is not a name.
fn owner<'s>(sc: &'s Scan<'_>, i: usize, prev_dot: bool) -> Option<&'s str> {
    prev_dot.then(|| sc.ident_at(i.wrapping_sub(2)).unwrap_or(""))
}

fn screaming(name: &str) -> bool {
    name.chars().any(|c| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn anchor(found: &mut [Found], before: String) {
    for f in found {
        f.anchor = Some(before.clone());
    }
}

/// The rows of one JavaScript expression that stands alone, such as an Alpine
/// attribute value.
pub(super) fn expression(src: &str, first_line: usize, kind: Kind, out: &mut Vec<Found>) {
    let cs: Vec<char> = src.chars().collect();
    let toks = lex(src, first_line);
    let sc = Scan {
        cs: &cs,
        toks: &toks,
        fns: CopyFns::default(),
    };
    sc.emit(0, toks.len(), kind, false, false, out);
}

struct Scan<'a> {
    cs: &'a [char],
    toks: &'a [Token],
    fns: CopyFns<'a>,
}

impl Scan<'_> {
    fn is_at(&self, i: usize, p: &str) -> bool {
        self.toks.get(i).is_some_and(|t| t.is(p))
    }

    fn ident_at(&self, i: usize) -> Option<&str> {
        self.toks.get(i).and_then(Token::ident)
    }

    fn opens(t: &Token) -> bool {
        t.is("(") || t.is("[") || t.is("{")
    }

    fn closes(t: &Token) -> bool {
        t.is(")") || t.is("]") || t.is("}")
    }

    /// Index of the bracket that closes the one at `open`.
    fn close_of(&self, open: usize) -> usize {
        let mut depth = 0i32;
        for (k, t) in self.toks.iter().enumerate().skip(open) {
            if Self::opens(t) {
                depth += 1;
            } else if Self::closes(t) {
                depth -= 1;
                if depth == 0 {
                    return k;
                }
            }
        }
        self.toks.len()
    }

    /// Token ranges of the arguments of the call whose `(` is at `open`.
    fn args(&self, open: usize) -> Vec<(usize, usize)> {
        let close = self.close_of(open);
        let mut out = Vec::new();
        let mut start = open + 1;
        let mut depth = 0i32;
        for k in open + 1..close.min(self.toks.len()) {
            let t = &self.toks[k];
            if Self::opens(t) {
                depth += 1;
            } else if Self::closes(t) {
                depth -= 1;
            } else if depth == 0 && t.is(",") {
                out.push((start, k));
                start = k + 1;
            }
        }
        if start < close {
            out.push((start, close));
        }
        out
    }

    /// End of the expression that starts at `start`: a `;`, or a closer that
    /// belongs to an enclosing bracket, or (for an object value) a `,`.
    fn expr_end(&self, start: usize, stop_on_comma: bool) -> usize {
        let mut depth = 0i32;
        for (k, t) in self.toks.iter().enumerate().skip(start) {
            if Self::opens(t) {
                depth += 1;
            } else if Self::closes(t) {
                if depth == 0 {
                    return k;
                }
                depth -= 1;
            } else if depth == 0 && (t.is(";") || (stop_on_comma && t.is(","))) {
                return k;
            }
        }
        self.toks.len()
    }

    fn source(&self, a: usize, b: usize) -> String {
        let (Some(first), Some(last)) = (self.toks.get(a), self.toks.get(b.wrapping_sub(1))) else {
            return String::new();
        };
        squeeze(&self.cs[first.start..last.end].iter().collect::<String>())
    }

    /// Top-level positions in `a..b` where `pred` holds.
    fn top_level(&self, a: usize, b: usize, pred: impl Fn(&Token) -> bool) -> Vec<usize> {
        let mut depth = 0i32;
        let mut at = Vec::new();
        for k in a..b {
            let t = &self.toks[k];
            if Self::opens(t) {
                depth += 1;
            } else if Self::closes(t) {
                depth -= 1;
            } else if depth == 0 && pred(t) {
                at.push(k);
            }
        }
        at
    }

    /// The alternatives an expression can show. The condition of a ternary is
    /// not shown, and neither is the left side of `&&`.
    fn alternatives(&self, a: usize, b: usize, out: &mut Vec<(usize, usize)>) {
        if a >= b {
            return;
        }
        // `( … )` around the whole expression.
        if self.toks[a].is("(") && self.close_of(a) == b - 1 {
            return self.alternatives(a + 1, b - 1, out);
        }
        if let Some(&q) = self.top_level(a, b, |t| t.is("?")).first() {
            let mut nested = 0;
            let colon = self
                .top_level(q + 1, b, |t| t.is("?") || t.is(":"))
                .into_iter()
                .find(|&k| {
                    if self.toks[k].is("?") {
                        nested += 1;
                        false
                    } else if nested > 0 {
                        nested -= 1;
                        false
                    } else {
                        true
                    }
                });
            if let Some(colon) = colon {
                self.alternatives(q + 1, colon, out);
                self.alternatives(colon + 1, b, out);
                return;
            }
        }
        let ors = self.top_level(a, b, |t| t.is("||") || t.is("??"));
        if !ors.is_empty() {
            let mut start = a;
            for k in ors.into_iter().chain(std::iter::once(b)) {
                self.alternatives(start, k, out);
                start = k + 1;
            }
            return;
        }
        if let Some(&and) = self.top_level(a, b, |t| t.is("&&")).last() {
            return self.alternatives(and + 1, b, out);
        }
        out.push((a, b));
    }

    /// Render one alternative as copy: literals as written, anything else as
    /// `{expr}`. `None` when no literal in it says anything.
    fn render(&self, a: usize, b: usize, strip_html: bool) -> Option<(String, bool, Vec<String>)> {
        let mut text = String::new();
        let mut concatenated = false;
        let mut fragments = Vec::new();
        let mut lit = |s: &str, text: &mut String| {
            let s = if strip_html {
                strip_tags(s)
            } else {
                s.to_string()
            };
            text.push_str(&s);
            fragments.push(s);
        };
        let mut start = a;
        for k in self
            .top_level(a, b, |t| t.is("+"))
            .into_iter()
            .chain(std::iter::once(b))
        {
            match self.toks.get(start..k) {
                Some([one]) => match &one.tok {
                    Tok::Str(s) => lit(s, &mut text),
                    Tok::Tpl(pieces) => {
                        for piece in pieces {
                            match piece {
                                Piece::Lit(s) => lit(s, &mut text),
                                Piece::Expr(e) => {
                                    concatenated = true;
                                    text.push_str(&format!("{{{}}}", squeeze(e)));
                                }
                            }
                        }
                    }
                    _ => {
                        concatenated = true;
                        text.push_str(&format!("{{{}}}", self.source(start, k)));
                    }
                },
                _ => {
                    concatenated = true;
                    text.push_str(&format!("{{{}}}", self.source(start, k)));
                }
            }
            start = k + 1;
        }
        let says_something = fragments.iter().any(|f| f.chars().any(char::is_alphabetic));
        let text = squeeze(&text);
        (says_something && (concatenated || !looks_like_code(&text))).then_some((
            text,
            concatenated,
            fragments,
        ))
    }

    /// An alternative written as HTML with elements in it: the literals as
    /// written, each hole as `{expr}`. `None` when it holds no element, so
    /// plain text keeps the one-row path.
    fn markup(&self, a: usize, b: usize) -> Option<String> {
        let mut src = String::new();
        let hole = |e: &str, src: &mut String| {
            // A hole that builds markup of its own is not read as markup here.
            let shown = if e.contains(['<', '>']) {
                "markup".to_string()
            } else {
                squeeze(e)
            };
            src.push_str(&format!("{{{shown}}}"));
            // Keep the line count of what follows.
            src.push_str(&"\n".repeat(e.matches('\n').count()));
        };
        let mut start = a;
        for k in self
            .top_level(a, b, |t| t.is("+"))
            .into_iter()
            .chain(std::iter::once(b))
        {
            match self.toks.get(start..k) {
                Some([one]) => match &one.tok {
                    Tok::Str(s) => src.push_str(s),
                    Tok::Tpl(pieces) => {
                        for piece in pieces {
                            match piece {
                                Piece::Lit(s) => src.push_str(s),
                                Piece::Expr(e) => hole(e, &mut src),
                            }
                        }
                    }
                    _ => hole(&self.source(start, k), &mut src),
                },
                _ => hole(&self.source(start, k), &mut src),
            }
            start = k + 1;
        }
        let cs: Vec<char> = src.chars().collect();
        let has_element = cs
            .windows(2)
            .any(|w| w[0] == '<' && w[1].is_ascii_alphabetic());
        has_element.then_some(src)
    }

    /// One row per element of an `innerHTML` value: each button's `title`,
    /// `aria-label` and label is its own text, not one merged row.
    fn split_markup(&self, markup: &str, line: usize, kind: Kind, out: &mut Vec<Found>) {
        let mut found = Vec::new();
        html::scan(markup, self.fns, &mut found);
        for mut f in found {
            if !without_holes(&f.text).chars().any(char::is_alphabetic) {
                continue;
            }
            f.line += line - 1;
            f.concatenated = f.text.contains('{');
            // A bare text node keeps the sink's kind; an attribute keeps its own.
            if f.kind == Kind::Text {
                f.kind = kind;
            }
            out.push(f);
        }
    }

    fn emit(
        &self,
        a: usize,
        b: usize,
        kind: Kind,
        strip_html: bool,
        needs_space: bool,
        out: &mut Vec<Found>,
    ) {
        let mut alts = Vec::new();
        self.alternatives(a, b.min(self.toks.len()), &mut alts);
        for (x, y) in alts {
            if strip_html {
                if let Some(markup) = self.markup(x, y) {
                    self.split_markup(&markup, self.toks[x].line, kind, out);
                    continue;
                }
            }
            let Some((text, concatenated, fragments)) = self.render(x, y, strip_html) else {
                continue;
            };
            if needs_space && !text.contains(' ') {
                continue;
            }
            out.push(Found {
                text,
                line: self.toks[x].line,
                kind,
                concatenated,
                area: None,
                fragments,
                anchor: None,
                continues: false,
            });
        }
    }
}

/// The text with every `{expr}` hole removed.
fn without_holes(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '{' => depth += 1,
            '}' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// A literal that is an identifier, a class name, a path or a selector rather
/// than words: `bi-check`, `index.html`, `#stage`, or a list of Bootstrap
/// icon classes such as `bi bi-sticky`.
fn looks_like_code(text: &str) -> bool {
    let mut words = text.split_whitespace().peekable();
    if words.peek().is_some() && words.all(|w| w == "bi" || w.starts_with("bi-")) {
        return true;
    }
    if text.contains(char::is_whitespace) {
        return false;
    }
    let kebab = text.contains('-')
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    let dotted = text.contains('.') && !text.ends_with('.') && !text.contains('…');
    kebab
        || dotted
        || text.contains([
            '_', '/', '#', '=', '<', '>', '(', ')', '[', ']', '{', '}', '\\',
        ])
}

/// The text of an HTML fragment: tags dropped, entities decoded.
fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    decode_entities(&out)
}
