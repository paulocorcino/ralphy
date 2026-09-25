//! Where a JavaScript string literal reaches the screen, and how an expression
//! around it becomes one row of copy.

mod scan;

use super::lex::{lex, Tok};
use super::{CopyFns, Found, Kind};
use scan::Scan;

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
        // A member access, plain or optional (`WBFail?.failed(`).
        let prev_dot = sc.is_at(i.wrapping_sub(1), ".") || sc.is_at(i.wrapping_sub(1), "?.");
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

/// The identifier before the dot of a method call at `i`: `None` for a bare
/// call, `Some("")` when the dot follows something that is not a name.
fn owner<'s>(sc: &'s Scan<'_>, i: usize, prev_dot: bool) -> Option<&'s str> {
    prev_dot.then(|| sc.ident_at(i.wrapping_sub(2)).unwrap_or(""))
}

/// `NEEDS_REPO`: the name the UI gives a constant that holds copy.
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
