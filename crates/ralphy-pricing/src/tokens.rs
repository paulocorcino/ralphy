//! The compact token count that the terminal and the web both show.

/// Format a token count compactly: `1.2M`, `8.4k`, or a bare `912` under a
/// thousand. One decimal place for the scaled forms.
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "a figure shown with one decimal; a loss above 2^53 is far below it"
)]
pub fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
