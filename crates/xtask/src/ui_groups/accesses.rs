//! Reads which members of `shell()` each member touches, and whether it writes.

use std::collections::BTreeMap;

use super::lex::{lex, Piece, Tok, Token};
use super::{close_of, ident_at, tok_is, Access, Member, TokenRange, MUTATORS};

/// The accesses of each member to the other members, from the tokens in its
/// range.
pub(super) fn accesses(toks: &[Token], members: &[Member], ranges: &[TokenRange]) -> Vec<Access> {
    let known: BTreeMap<&str, usize> = members
        .iter()
        .enumerate()
        .map(|(n, m)| (m.name.as_str(), n))
        .collect();
    let mut accesses = Vec::new();
    for (within, &(first, last)) in ranges.iter().enumerate() {
        let mut found = Vec::new();
        scan_accesses(&toks[first..=last], &known, &mut found);
        for (name, write) in found {
            if name != members[within].name {
                accesses.push(Access {
                    name,
                    within,
                    write,
                });
            }
        }
    }
    accesses
}

/// Every `this.<member>` / `self.<member>` in `toks`, with whether it writes.
pub(super) fn scan_accesses(
    toks: &[Token],
    known: &BTreeMap<&str, usize>,
    out: &mut Vec<(String, bool)>,
) {
    for (j, t) in toks.iter().enumerate() {
        if let Tok::Tpl(pieces) = &t.tok {
            scan_holes(pieces, t.line, known, out);
            continue;
        }
        if let Some(name) = member_name(toks, j).filter(|n| known.contains_key(n)) {
            out.push((name.to_string(), writes(toks, j, j + 2)));
        }
    }
}

/// The accesses in the `${…}` holes of a template literal that starts on `line`.
fn scan_holes(
    pieces: &[Piece],
    line: usize,
    known: &BTreeMap<&str, usize>,
    out: &mut Vec<(String, bool)>,
) {
    for piece in pieces {
        if let Piece::Expr(code) = piece {
            scan_accesses(&lex(code, line), known, out);
        }
    }
}

/// The name in `this.<name>` or `self.<name>` when the receiver is at `j`.
fn member_name(toks: &[Token], j: usize) -> Option<&str> {
    let receiver = matches!(ident_at(toks, j), Some("this" | "self"));
    if !receiver || !tok_is(toks, j + 1, ".") {
        return None;
    }
    ident_at(toks, j + 2)
}

/// Is the access whose receiver is at `recv` and whose name is at `name` a write?
fn writes(toks: &[Token], recv: usize, name: usize) -> bool {
    let next = name + 1;
    if assigns(toks, next) || bumps(toks, next) || (recv > 0 && bumps(toks, recv - 1)) {
        return true;
    }
    mutating_call(toks, next) || writes_inside(toks, recv, next)
}

/// Is there a `++` or `--` at `i`?
fn bumps(toks: &[Token], i: usize) -> bool {
    tok_is(toks, i, "++") || tok_is(toks, i, "--")
}

/// Is there a call of a [`MUTATORS`] method at `i`: `.push(`, `?.set(`, …?
fn mutating_call(toks: &[Token], i: usize) -> bool {
    (tok_is(toks, i, ".") || tok_is(toks, i, "?."))
        && ident_at(toks, i + 1).is_some_and(|m| MUTATORS.contains(&m))
        && tok_is(toks, i + 2, "(")
}

/// The index after the `.name`, `?.name` or `[…]` step at `k`, if there is one.
fn step_inside(toks: &[Token], k: usize) -> Option<usize> {
    if (tok_is(toks, k, ".") || tok_is(toks, k, "?.")) && ident_at(toks, k + 1).is_some() {
        Some(k + 2)
    } else if tok_is(toks, k, "[") {
        Some(close_of(toks, k) + 1)
    } else {
        None
    }
}

/// A write inside the member whose steps start at `next`: `this.x.y =`,
/// `this.x[k] =`, `this.x.y++`, `this.x.y.push(…)`, `delete this.x[k]`.
fn writes_inside(toks: &[Token], recv: usize, next: usize) -> bool {
    let mut k = next;
    let mut inside = false;
    while let Some(after) = step_inside(toks, k) {
        k = after;
        inside = true;
        if assigns(toks, k) || bumps(toks, k) || mutating_call(toks, k) {
            return true;
        }
    }
    inside && recv > 0 && toks[recv - 1].ident() == Some("delete")
}

/// Does an assignment operator start at `i`? The lexer splits `||=` into
/// `||` and `=`, so two touching tokens count as one.
fn assigns(toks: &[Token], i: usize) -> bool {
    let Some(t) = toks.get(i) else { return false };
    match t.tok {
        Tok::Punct("=" | "+=" | "-=" | "*=" | "/=" | "**=") => true,
        Tok::Punct("||" | "&&" | "??" | "%" | "&" | "|" | "^") => toks
            .get(i + 1)
            .is_some_and(|n| n.is("=") && n.start == t.end),
        Tok::Ident(_) | Tok::Punct(_) | Tok::Str(_) | Tok::Tpl(_) | Tok::Other => false,
    }
}
