//! Terminal-width fitting for the queue/active-line live region (#226): measures
//! ambiguous-width glyphs (`▰`/`▱`/emoji) at their East-Asian worst case so the
//! accounting matches what actually renders in a narrow conhost cell, then
//! truncates strings to that budget before they reach `set_message`.

use unicode_width::UnicodeWidthChar;

/// Width of `s` in terminal cells, ANSI stripped, each char measured worst-case
/// (`width_cjk`) — see `## Decisions` in `.ralphy/plan.md` for why this table,
/// not `console::measure_text_width`'s `width()`, is the one that keeps the
/// physical row count at 1.
pub(crate) fn display_width(s: &str) -> usize {
    console::strip_ansi_codes(s)
        .chars()
        .map(|c| c.width_cjk().unwrap_or(0))
        .sum()
}

/// Truncate `s` to fit within `max` display columns, appending `…` when cut.
/// Never panics, never splits a `char`, never indexes by byte.
pub(crate) fn truncate_to_width(s: &str, max: usize) -> String {
    if display_width(s) <= max {
        return s.to_string();
    }
    if max < 2 {
        let mut out = String::new();
        let mut used = 0usize;
        for c in s.chars() {
            let w = c.width_cjk().unwrap_or(0);
            if used + w > max {
                break;
            }
            used += w;
            out.push(c);
        }
        return out;
    }
    let mut out = String::new();
    let mut used = 0usize;
    for c in s.chars() {
        let w = c.width_cjk().unwrap_or(0);
        // Reserve 2 columns for the `…` (its own width_cjk is 2).
        if used + w + 2 > max {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push('…');
    out
}

/// The live-region budget: the terminal's column count, minus one reserved
/// column so a line never sits exactly on the last cell (some terminals,
/// conhost included, auto-wrap there anyway). Floors at 1.
pub(crate) fn terminal_width() -> usize {
    (console::Term::stderr().size().1 as usize)
        .saturating_sub(1)
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_truncate_is_char_safe_on_multibyte_and_zero_width() {
        let empty = truncate_to_width("▰▱▰▱▰▱▰ 0/7 (pending #217)", 0);
        assert_eq!(empty, "", "width 0 truncates to empty, no panic");

        let cut = truncate_to_width("café ☕ ▰▱", 6);
        // Valid by construction: `String` can only ever hold well-formed UTF-8, so
        // this call itself is the char-safety proof, not just the width bound.
        assert!(display_width(&cut) <= 6, "fits budget: {cut:?}");
    }
}
