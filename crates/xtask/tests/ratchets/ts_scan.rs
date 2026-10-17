//! The scans of the workbench modules that read code through the `ui-copy`
//! lexer: casts and `new WebSocket(` sites. Split out of `ratchets.rs`, which
//! holds the baselines and the tests.

use super::lex;

/// The `as unknown as` casts in the code of `src`, template holes included.
pub(crate) fn double_casts(src: &str) -> usize {
    let toks = lex::lex(src, 1);
    let mut count = 0;
    for (i, t) in toks.iter().enumerate() {
        let words: Vec<_> = toks[i..].iter().take(3).map(|t| t.ident()).collect();
        if words == [Some("as"), Some("unknown"), Some("as")] {
            count += 1;
        }
        if let lex::Tok::Tpl(pieces) = &t.tok {
            for piece in pieces {
                if let lex::Piece::Expr(code) = piece {
                    count += double_casts(code);
                }
            }
        }
    }
    count
}

/// The casts in `src` written over another cast: `as` a type, then `as`
/// again. The type is a dotted name with optional type arguments and `[]`
/// suffixes, which is every cast type the workbench writes. Comments and
/// strings are not read; the code inside a template hole is.
pub(crate) fn chained_casts(src: &str) -> usize {
    let toks = lex::lex(src, 1);
    let mut count = 0;
    for (i, t) in toks.iter().enumerate() {
        if t.ident() == Some("as") {
            let end = cast_type_end(&toks, i + 1);
            if end > i + 1 && toks.get(end).and_then(|t| t.ident()) == Some("as") {
                count += 1;
            }
        }
        if let lex::Tok::Tpl(pieces) = &t.tok {
            for piece in pieces {
                if let lex::Piece::Expr(code) = piece {
                    count += chained_casts(code);
                }
            }
        }
    }
    count
}

/// The index after the cast type that starts at `start`, or `start` when no
/// name starts there.
fn cast_type_end(toks: &[lex::Token], start: usize) -> usize {
    let name = |k: usize| toks.get(k).and_then(|t| t.ident()).is_some();
    let punct = |k: usize, p: &str| toks.get(k).is_some_and(|t| t.is(p));
    if !name(start) {
        return start;
    }
    let mut j = start + 1;
    loop {
        let member = punct(j, ".") && name(j + 1);
        let array = punct(j, "[") && punct(j + 1, "]");
        if member || array {
            j += 2;
        } else if punct(j, "<") {
            let mut depth = 0usize;
            while let Some(t) = toks.get(j) {
                if t.is("<") {
                    depth += 1;
                } else if t.is(">") {
                    depth = depth.saturating_sub(1);
                }
                j += 1;
                if depth == 0 {
                    break;
                }
            }
        } else {
            return j;
        }
    }
}

/// The `new WebSocket(` sites in the code of `src`. A comment or a string that
/// says so is not counted.
pub(crate) fn websocket_sites(src: &str) -> usize {
    let toks = lex::lex(src, 1);
    toks.windows(3)
        .filter(|w| {
            w[0].ident() == Some("new") && w[1].ident() == Some("WebSocket") && w[2].is("(")
        })
        .count()
}
