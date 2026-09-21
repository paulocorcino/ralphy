//! The token/cost meter: pricing a usage and formatting it compactly.

use std::time::Duration;

use crate::pricing::PriceTable;
use crate::ui::UsageLite;

/// A priced token meter for one scroll-up line: the combined breakdown to show
/// (`↑ ⚡ ❄ ↓`) plus the read-time USD (D8). `usd` is `None` when nothing in the
/// meter could be priced (rendered `$?`, never `$0`); `partial` flags a model that
/// was unpriced so the figure can carry a `+?` residue.
pub(crate) struct Meter {
    pub(crate) usage: UsageLite,
    pub(crate) usd: Option<f64>,
    pub(crate) partial: bool,
}

/// Per-line render context the presenter computes in `drive` (it owns the clock,
/// the active issue's display model/effort, and the price table) and hands to
/// `render_line`. All fields are absent for events that carry no meter/duration.
#[derive(Default)]
pub(crate) struct LineExtra {
    pub(crate) duration: Option<Duration>,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
    pub(crate) meter: Option<Meter>,
}

/// Price one phase's [`UsageLite`] at read time, or `None` when its model is absent
/// or unpriced. `UsageLite` aliases the core `Usage`, which `crate::pricing::counts`
/// narrows to the [`TokenCounts`](crate::pricing::TokenCounts) the table prices on.
fn price_lite(pt: &PriceTable, u: &UsageLite) -> Option<f64> {
    let model = u.model.as_deref().filter(|m| !m.is_empty())?;
    pt.cost_usd(model, &crate::pricing::counts(u))
}

/// Build a [`Meter`] for an issue line from its planning usage (stashed, may be
/// absent on the `plan written` line) and a final phase's usage. The display
/// breakdown sums both phases; the USD prices each phase's model separately (plan
/// and execute often differ) and sums the priced portion, mirroring
/// `cost_usd_by_model`'s `$?`/`+?` semantics (ADR-0008 D8).
pub(crate) fn meter_for(pt: &PriceTable, plan: Option<&UsageLite>, last: &UsageLite) -> Meter {
    let mut combined = last.clone();
    combined.model = None; // the sum spans models; the label's model comes from the active issue
    if let Some(p) = plan {
        combined.input += p.input;
        combined.cache_read += p.cache_read;
        combined.cache_creation += p.cache_creation;
        combined.output += p.output;
    }
    let mut usd = 0.0;
    let mut any_priced = false;
    let mut any_unpriced = false;
    for u in plan.into_iter().chain(std::iter::once(last)) {
        if u.total() == 0 {
            continue;
        }
        match price_lite(pt, u) {
            Some(c) => {
                usd += c;
                any_priced = true;
            }
            None => any_unpriced = true,
        }
    }
    Meter {
        usage: combined,
        usd: any_priced.then_some(usd),
        partial: any_unpriced,
    }
}

/// The compact emoji token meter: `↑12.4k ⚡184k ❄8.1k ↓3.2k · $1.84`. `↑` input,
/// `⚡` cache-read (hot reuse), `❄` cache-write (cold store), `↓` output. The ASCII
/// path drops the emoji glyphs for `in/cr/cw/out` labels.
pub(super) fn fmt_meter(m: &Meter, emoji: bool) -> String {
    let u = &m.usage;
    let (i, cr, cw, o) = if emoji {
        ("↑", "⚡", "❄", "↓")
    } else {
        ("in ", "cr ", "cw ", "out ")
    };
    format!(
        "{i}{} {cr}{} {cw}{} {o}{} · {}",
        fmt_tokens(u.input),
        fmt_tokens(u.cache_read),
        fmt_tokens(u.cache_creation),
        fmt_tokens(u.output),
        fmt_usd_compact(m.usd, m.partial),
    )
}

/// Compact read-time USD for an inline meter: `$1.84`, `$1.84+?` when some model was
/// unpriced, or a bare `$?` when nothing could be priced — never `$0.00`, which would
/// hide spend (ADR-0008 D8).
pub(crate) fn fmt_usd_compact(usd: Option<f64>, partial: bool) -> String {
    match usd {
        None => "$?".to_string(),
        Some(v) => format!("${v:.2}{}", if partial { "+?" } else { "" }),
    }
}

/// Format a token count compactly for the footer: `1.2M`, `8.4k`, or a bare
/// `912` under a thousand. One decimal place for the scaled forms.
pub(crate) fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
