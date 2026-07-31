//! The unpriced volume, split by cause — the gap the total could not price,
//! kept first-class so it stays visible enough to get fixed (ADR-0053 D4).

use serde::{Deserialize, Serialize};

use super::format::{fmt_share, fmt_tokens, share_of, volume_label};

/// The tokens the total could not price, split by cause. First-class, not a
/// footnote: the gap stays visible enough to get fixed, and the operator can
/// tell which part of it is worth working on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Unpriced {
    /// Every unpriced token: the sum of [`Self::causes`].
    pub tokens: u64,
    /// The rendered volume (`1.66M`), or an empty string when nothing is unpriced.
    pub label: String,
    /// The share of all this project's tokens that went unpriced, `0.0..=1.0`.
    pub share: f64,
    /// The same share, rendered (`42.9%`).
    pub share_label: String,
    /// The complement: the tokens the total DID price, and their share. Carried
    /// so the coverage of the figure can be shown as a proportion rather than
    /// left to be inferred from its gap.
    pub priced: u64,
    pub priced_label: String,
    pub priced_share: f64,
    pub priced_share_label: String,
    /// The causes with volume in them, in the order the operator can act on
    /// them. A cause with nothing in it is DROPPED: a bucket at zero is not a
    /// finding, and three permanent zeroes teach the operator to stop reading the
    /// element that exists to be read.
    pub causes: Vec<UnpricedCause>,
    /// Interactive sessions whose vendor keeps no token count anywhere (`tokens:
    /// null`, e.g. Cursor — ADR-0042 D11). They carry real spend of unmeasurable
    /// size, so they are counted here rather than passing as zero.
    pub unmetered_sessions: u64,
}

/// One reason a token went unpriced. `key` is the closed vocabulary the surface
/// styles on; everything else is already rendered.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UnpricedCause {
    /// `recoverable` | `no_price` | `lost`.
    pub key: String,
    pub tokens: u64,
    pub label: String,
    /// This cause's share of the UNPRICED volume (not of the project) — the
    /// question a cause row answers is "how much of the gap is this".
    pub share: f64,
    pub share_label: String,
}

/// The running unpriced volume, by cause. Crate-private for the same reason as
/// [`super::meter::Counts`].
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Gap {
    pub(crate) recoverable: u64,
    pub(crate) no_price: u64,
    pub(crate) lost: u64,
    pub(crate) unmetered_sessions: u64,
}

impl Gap {
    /// Ordered by what the operator can do about it: run recovery, add one line
    /// to `pricing.toml`, or nothing at all (ADR-0053 D4).
    const CAUSES: [&'static str; 3] = ["recoverable", "no_price", "lost"];

    fn of(&self, key: &str) -> u64 {
        match key {
            "recoverable" => self.recoverable,
            "no_price" => self.no_price,
            _ => self.lost,
        }
    }

    pub(crate) fn render(self, all_tokens: u64) -> Unpriced {
        let tokens = self.recoverable + self.no_price + self.lost;
        let share = share_of(tokens, all_tokens);
        let priced = all_tokens.saturating_sub(tokens);
        let priced_share = share_of(priced, all_tokens);
        Unpriced {
            tokens,
            label: volume_label(tokens),
            share,
            share_label: fmt_share(share),
            priced,
            priced_label: volume_label(priced),
            priced_share,
            priced_share_label: fmt_share(priced_share),
            causes: Self::CAUSES
                .iter()
                .filter(|key| self.of(key) > 0)
                .map(|key| {
                    let cause = self.of(key);
                    let share = share_of(cause, tokens);
                    UnpricedCause {
                        key: (*key).to_string(),
                        tokens: cause,
                        label: fmt_tokens(cause),
                        share,
                        share_label: fmt_share(share),
                    }
                })
                .collect(),
            unmetered_sessions: self.unmetered_sessions,
        }
    }
}
