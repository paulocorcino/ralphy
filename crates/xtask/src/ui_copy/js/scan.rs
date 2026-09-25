//! The expression reader for `js::scan`: bracket matching, argument and
//! expression bounds, and how an expression becomes the text of one row.

use super::super::lex::{Piece, Tok, Token};
use super::super::{decode_entities, html, squeeze, CopyFns, Found, Kind};

pub(super) struct Scan<'a> {
    pub(super) cs: &'a [char],
    pub(super) toks: &'a [Token],
    pub(super) fns: CopyFns<'a>,
}

impl Scan<'_> {
    pub(super) fn is_at(&self, i: usize, p: &str) -> bool {
        self.toks.get(i).is_some_and(|t| t.is(p))
    }

    pub(super) fn ident_at(&self, i: usize) -> Option<&str> {
        self.toks.get(i).and_then(Token::ident)
    }

    fn opens(t: &Token) -> bool {
        t.is("(") || t.is("[") || t.is("{")
    }

    fn closes(t: &Token) -> bool {
        t.is(")") || t.is("]") || t.is("}")
    }

    /// Index of the bracket that closes the one at `open`.
    pub(super) fn close_of(&self, open: usize) -> usize {
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
    pub(super) fn args(&self, open: usize) -> Vec<(usize, usize)> {
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
    pub(super) fn expr_end(&self, start: usize, stop_on_comma: bool) -> usize {
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

    pub(super) fn emit(
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
