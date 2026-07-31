//! The money and volume vocabulary, spelled once. Every figure this crate hands
//! a client is rendered HERE, so neither the abbreviation (`1.2M`) nor the
//! percentage (`38.2%`) is ever reimplemented in JavaScript (PRD #355).

/// `part / whole` as `0.0..=1.0`, and `0.0` for an empty whole — a share of
/// nothing is not a share, and `NaN` would reach a bar's width.
pub(crate) fn share_of(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}

/// A share as a percentage, rendered here rather than in the client: one decimal,
/// because the figure this surface exists to shame (44%) and the figure it should
/// reach (0.2%) must BOTH read, and an integer percent collapses the second.
pub(crate) fn fmt_share(share: f64) -> String {
    format!("{:.1}%", share * 100.0)
}

/// The project total, rendered: `~$?` when nothing priced (ADR-0034 D3 — `$0`
/// would be a lie that hides spend), else `$2,350.59`, with a trailing `+` when
/// the figure omits volume it could not price.
pub(crate) fn fmt_total(usd: Option<f64>, floor: bool) -> String {
    match usd {
        None => "~$?".to_string(),
        Some(value) => format!(
            "${}{}",
            group_thousands(value),
            if floor { "+" } else { "" }
        ),
    }
}

/// `2350.586` → `2,350.59`. Two decimals with `,` every three integer digits —
/// a project total reaches four and five figures, where an ungrouped run of
/// digits stops being readable at a glance.
pub(crate) fn group_thousands(value: f64) -> String {
    let fixed = format!("{value:.2}");
    let (whole, fraction) = fixed.split_once('.').unwrap_or((fixed.as_str(), "00"));
    let (sign, digits) = match whole.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", whole),
    };
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{sign}{grouped}.{fraction}")
}

/// A token volume for the unpriced markers: the abbreviated count, or an empty
/// string for zero so a bucket with nothing in it renders no marker at all.
pub(crate) fn volume_label(tokens: u64) -> String {
    if tokens == 0 {
        String::new()
    } else {
        fmt_tokens(tokens)
    }
}

/// Format a token count compactly: `1.2M`, `8.4k`, or a bare `912` under a
/// thousand. Mirrors the CLI footer's `fmt_tokens` so the web and the terminal
/// abbreviate identically.
pub(crate) fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A four-figure total is where an ungrouped run of digits stops being
    /// readable — the separator is the reason this renders server-side.
    #[test]
    fn a_large_floor_total_reads_as_grouped_digits() {
        assert_eq!(fmt_total(Some(2_350.586), true), "$2,350.59+");
        assert_eq!(fmt_total(Some(1_234_567.8), false), "$1,234,567.80");
        assert_eq!(fmt_total(Some(0.5), false), "$0.50");
        assert_eq!(fmt_total(None, true), "~$?");
    }
}
