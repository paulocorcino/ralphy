//! Where a JavaScript string literal reaches the screen, and how an expression
//! around it becomes one row of copy.

mod scan;
mod walk;

use super::lex::lex;
use super::{CopyFns, Found, Kind};
use scan::Scan;
use walk::Walk;

/// Object keys whose value is shown: a toast's `text`, a dialog's `title`, a
/// table row's `label`, a setting's `help` and its section's `blurb`
/// (`wb-settings.ts`, rendered by `x-text="it.help"` / `"sec.blurb"`).
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
    Walk::new(&sc, out).run();
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

#[cfg(test)]
mod tests;
