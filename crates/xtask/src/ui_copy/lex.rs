//! A JavaScript lexer small enough to trust for one job: find the string
//! literals and the punctuation around them, with line numbers.
//!
//! It is not a parser. It knows strings, template literals, comments and regex
//! literals, because each of those can hide a quote that would otherwise
//! derail every token after it. Everything else is an identifier, a punctuator
//! or `Other`.

/// One piece of a template literal: text as written, or a `${…}` hole.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Piece {
    Lit(String),
    Expr(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Tok {
    Ident(String),
    Punct(&'static str),
    Str(String),
    Tpl(Vec<Piece>),
    /// A number or a regex literal: never copy, only kept so the token stream
    /// stays in step with the source.
    Other,
}

#[derive(Debug, Clone)]
pub(super) struct Token {
    pub(super) tok: Tok,
    pub(super) line: usize,
    /// Char offsets into the lexed source, `start..end`.
    pub(super) start: usize,
    pub(super) end: usize,
}

impl Token {
    pub(super) fn is(&self, p: &str) -> bool {
        matches!(self.tok, Tok::Punct(q) if q == p)
    }

    pub(super) fn ident(&self) -> Option<&str> {
        match &self.tok {
            Tok::Ident(name) => Some(name),
            _ => None,
        }
    }
}

/// Longest first: `===` must win over `==` and `=`.
const PUNCTS: &[&str] = &[
    "===", "!==", "...", "**=", "?.", "??", "||", "&&", "==", "!=", "=>", "<=", ">=", "+=", "-=",
    "*=", "/=", "++", "--", "(", ")", "[", "]", "{", "}", ",", ";", ":", "?", "=", "+", "-", "*",
    "/", "%", "!", "<", ">", ".", "&", "|", "^", "~", "@", "#",
];

/// After these keywords a `/` opens a regex, not a division.
const REGEX_AFTER: &[&str] = &[
    "return", "typeof", "case", "in", "of", "new", "delete", "void", "throw", "else", "do",
];

/// Lex `src`; `first_line` is the line number of its first character.
pub(super) fn lex(src: &str, first_line: usize) -> Vec<Token> {
    let cs: Vec<char> = src.chars().collect();
    let mut out: Vec<Token> = Vec::new();
    let mut line = first_line;
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        let start_line = line;
        let next = cs.get(i + 1).copied();
        let tok = if c == '/' && next == Some('/') {
            while i < cs.len() && cs[i] != '\n' {
                i += 1;
            }
            continue;
        } else if c == '/' && next == Some('*') {
            i += 2;
            while i < cs.len() && !(cs[i] == '*' && cs.get(i + 1) == Some(&'/')) {
                if cs[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i += 2;
            continue;
        } else if c == '/' && regex_allowed(out.last()) {
            i = skip_regex(&cs, i);
            Tok::Other
        } else if c == '"' || c == '\'' {
            let (text, end) = read_string(&cs, i, &mut line);
            i = end;
            Tok::Str(text)
        } else if c == '`' {
            let (pieces, end) = read_template(&cs, i, &mut line);
            i = end;
            Tok::Tpl(pieces)
        } else if c.is_alphabetic() || c == '_' || c == '$' {
            while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_' || cs[i] == '$') {
                i += 1;
            }
            Tok::Ident(cs[start..i].iter().collect())
        } else if c.is_ascii_digit() {
            while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '.' || cs[i] == '_') {
                i += 1;
            }
            Tok::Other
        } else if let Some(p) = PUNCTS.iter().find(|p| {
            p.chars()
                .enumerate()
                .all(|(k, pc)| cs.get(i + k) == Some(&pc))
        }) {
            i += p.chars().count();
            Tok::Punct(p)
        } else {
            i += 1;
            Tok::Other
        };
        out.push(Token {
            tok,
            line: start_line,
            start,
            end: i,
        });
    }
    out
}

fn regex_allowed(prev: Option<&Token>) -> bool {
    match prev.map(|t| &t.tok) {
        None => true,
        Some(Tok::Ident(word)) => REGEX_AFTER.contains(&word.as_str()),
        Some(Tok::Punct(p)) => !matches!(*p, ")" | "]" | "}"),
        Some(_) => false,
    }
}

/// Index one past a regex literal that opens at `i`, flags included. A `/`
/// inside a character class does not close it.
fn skip_regex(cs: &[char], mut i: usize) -> usize {
    i += 1;
    let mut in_class = false;
    while i < cs.len() {
        match cs[i] {
            '\\' => {
                i += 2;
                continue;
            }
            '[' => in_class = true,
            ']' => in_class = false,
            '/' if !in_class => {
                i += 1;
                break;
            }
            '\n' => break,
            _ => {}
        }
        i += 1;
    }
    while i < cs.len() && cs[i].is_alphanumeric() {
        i += 1;
    }
    i
}

/// Decode one escape whose backslash is at `i - 1`; returns the char and the
/// index after the escape.
fn escape(cs: &[char], i: usize) -> (Option<char>, usize) {
    let hex = |from: usize, to: usize| -> Option<char> {
        let digits: String = cs.get(from..to)?.iter().collect();
        u32::from_str_radix(&digits, 16)
            .ok()
            .and_then(char::from_u32)
    };
    match cs.get(i) {
        Some('n') => (Some('\n'), i + 1),
        Some('r') => (Some('\r'), i + 1),
        Some('t') => (Some('\t'), i + 1),
        Some('\n') => (None, i + 1),
        Some('x') => (hex(i + 1, i + 3), i + 3),
        Some('u') if cs.get(i + 1) == Some(&'{') => {
            let close = (i + 2..cs.len())
                .find(|&k| cs[k] == '}')
                .unwrap_or(cs.len());
            (hex(i + 2, close), close + 1)
        }
        Some('u') => (hex(i + 1, i + 5), i + 5),
        Some(&other) => (Some(other), i + 1),
        None => (None, i),
    }
}

fn read_string(cs: &[char], open: usize, line: &mut usize) -> (String, usize) {
    let quote = cs[open];
    let mut text = String::new();
    let mut i = open + 1;
    while i < cs.len() {
        let c = cs[i];
        if c == '\\' {
            if cs.get(i + 1) == Some(&'\n') {
                *line += 1;
            }
            let (decoded, next) = escape(cs, i + 1);
            text.extend(decoded);
            i = next;
            continue;
        }
        i += 1;
        if c == quote {
            break;
        }
        // An unterminated string ends at the line; it must not swallow the
        // rest of the file.
        if c == '\n' {
            *line += 1;
            break;
        }
        text.push(c);
    }
    (text, i)
}

fn read_template(cs: &[char], open: usize, line: &mut usize) -> (Vec<Piece>, usize) {
    let mut pieces = Vec::new();
    let mut lit = String::new();
    let mut i = open + 1;
    while i < cs.len() {
        let c = cs[i];
        match c {
            '\\' => {
                let (decoded, next) = escape(cs, i + 1);
                lit.extend(decoded);
                i = next;
            }
            '`' => {
                i += 1;
                break;
            }
            '$' if cs.get(i + 1) == Some(&'{') => {
                if !lit.is_empty() {
                    pieces.push(Piece::Lit(std::mem::take(&mut lit)));
                }
                let end = hole_end(cs, i + 2, line);
                let expr: String = cs[i + 2..end.saturating_sub(1).max(i + 2)].iter().collect();
                pieces.push(Piece::Expr(expr.trim().to_string()));
                i = end;
            }
            _ => {
                if c == '\n' {
                    *line += 1;
                }
                lit.push(c);
                i += 1;
            }
        }
    }
    if !lit.is_empty() {
        pieces.push(Piece::Lit(lit));
    }
    (pieces, i)
}

/// Index one past the `}` that closes a `${` hole whose body starts at `i`.
/// Strings and nested templates inside the hole are skipped whole.
fn hole_end(cs: &[char], mut i: usize, line: &mut usize) -> usize {
    let mut depth = 1;
    while i < cs.len() {
        match cs[i] {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            '"' | '\'' => {
                i = read_string(cs, i, line).1;
                continue;
            }
            '`' => {
                i = read_template(cs, i, line).1;
                continue;
            }
            '\n' => *line += 1,
            _ => {}
        }
        i += 1;
    }
    i
}
