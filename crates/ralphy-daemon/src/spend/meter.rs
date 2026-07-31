//! The `token_meter` split: the running accumulator and the rendered view the
//! document carries.

use ralphy_pricing::TokenCounts;
use serde::{Deserialize, Serialize};

use super::format::{fmt_share, fmt_tokens, share_of};

/// The canonical `token_meter` split the operator already reads in the terminal
/// (`↑` input, `⚡` cache-read, `❄` cache-write, `↓` output), carrying the raw
/// counts, the one-line meter, and the same split as four addressable parts. The
/// `k`/`M` abbreviation and every percentage are formatted **here** so neither is
/// reimplemented in JavaScript (PRD #355) — the client renders, it does not
/// compute.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TokenMeter {
    pub input: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
    pub output: u64,
    pub total: u64,
    /// The abbreviated total (`1.8M`).
    pub label: String,
    /// `↑12.4k ⚡184k ❄8.1k ↓3.2k`. The CLI's `fmt_meter`
    /// (`ralphy-cli/src/ui/render.rs`) is the source of this vocabulary; the two
    /// are mirrored deliberately rather than shared, because the daemon must not
    /// depend on the CLI. `meter_is_the_cli_vocabulary` pins the glyphs.
    pub meter: String,
    /// The same four counts, always all four and always in the canonical order,
    /// so the surface can lay the split out as rows without knowing the
    /// vocabulary. A zero part is KEPT — an absent `⚡` would read as "no cache
    /// column exists" rather than "nothing was reused".
    pub parts: Vec<MeterPart>,
}

/// One kind of token in the meter, carrying its own glyph, name and share so a
/// renderer needs no table of its own.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MeterPart {
    /// `input` | `cache_read` | `cache_creation` | `output`.
    pub key: String,
    /// `↑` | `⚡` | `❄` | `↓`.
    pub glyph: String,
    /// `input` | `cache read` | `cache write` | `output`.
    pub name: String,
    pub tokens: u64,
    /// The abbreviated count (`184k`).
    pub label: String,
    /// This kind's share of the meter's total, `0.0..=1.0` — for a bar's width.
    pub share: f64,
    /// The same share, rendered (`38.2%`).
    pub share_label: String,
}

/// The four running token counts. Crate-private: the document's [`TokenMeter`]
/// is a RENDERED view of this, and keeping the accumulator separate is what
/// stops a half-formatted struct from ever existing.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Counts {
    pub(crate) input: u64,
    pub(crate) cache_read: u64,
    pub(crate) cache_creation: u64,
    pub(crate) output: u64,
    pub(crate) total: u64,
}

impl Counts {
    /// The canonical order and vocabulary, in one place: `↑` input, `⚡`
    /// cache-read (hot reuse), `❄` cache-write (cold store), `↓` output.
    const KINDS: [(&'static str, &'static str, &'static str); 4] = [
        ("input", "↑", "input"),
        ("cache_read", "⚡", "cache read"),
        ("cache_creation", "❄", "cache write"),
        ("output", "↓", "output"),
    ];

    fn of(&self, key: &str) -> u64 {
        match key {
            "input" => self.input,
            "cache_read" => self.cache_read,
            "cache_creation" => self.cache_creation,
            _ => self.output,
        }
    }

    pub(crate) fn add(&mut self, tokens: &TokenCounts) {
        self.input += tokens.input;
        self.cache_read += tokens.cache_read;
        self.cache_creation += tokens.cache_creation;
        self.output += tokens.output;
        self.total += tokens.total();
    }

    pub(crate) fn render(self) -> TokenMeter {
        let parts = Self::KINDS
            .iter()
            .map(|(key, glyph, name)| {
                let tokens = self.of(key);
                let share = share_of(tokens, self.total);
                MeterPart {
                    key: (*key).to_string(),
                    glyph: (*glyph).to_string(),
                    name: (*name).to_string(),
                    tokens,
                    label: fmt_tokens(tokens),
                    share,
                    share_label: fmt_share(share),
                }
            })
            .collect::<Vec<_>>();
        TokenMeter {
            input: self.input,
            cache_read: self.cache_read,
            cache_creation: self.cache_creation,
            output: self.output,
            total: self.total,
            label: fmt_tokens(self.total),
            meter: parts
                .iter()
                .map(|p| format!("{}{}", p.glyph, p.label))
                .collect::<Vec<_>>()
                .join(" "),
            parts,
        }
    }
}
