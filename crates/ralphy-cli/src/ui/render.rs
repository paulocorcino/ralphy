//! Stateless render formatters: pure functions and DTOs that turn a
//! [`RunEvent`](crate::runstate::RunEvent) or panel data into display strings.
//! No state, no `indicatif` — the live-region machine lives in
//! [`presenter`](super::presenter).

use std::time::Duration;

use console::Style;

mod line;
mod meter;
mod panel;

pub(crate) use line::{render_active_line, render_line, sleep_label};
#[cfg(test)]
pub(crate) use meter::{fmt_tokens, fmt_usd_compact};
pub(crate) use meter::{meter_for, LineExtra, Meter};
pub use panel::{render_totals_panel, PanelBranchMode, PanelData, PanelStop};

/// `MM:SS` clock form (minutes may exceed 59), e.g. `12:43`, `45:00`. The
/// active-line/budget form; distinct from [`fmt_duration`]'s `2m13s` finished-line
/// form.
pub(crate) fn fmt_clock(d: Duration) -> String {
    let secs = d.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// How a line is rendered: whether ANSI colour and emoji are available. The
/// non-TTY / `NO_COLOR` path sets both `false`, guaranteeing no ANSI ever reaches
/// a redirected file (ADR-0006 D3).
#[derive(Debug, Clone, Copy)]
pub struct RenderOpts {
    pub color: bool,
    pub emoji: bool,
}

/// Normalize a git remote URL to an `https` web URL for the header link: strip a
/// trailing `.git`, and rewrite the `git@host:owner/repo` / `ssh://git@host/owner/repo`
/// SSH forms to `https://host/owner/repo`. An already-`http(s)` URL is left as-is
/// (minus `.git`). Pure over its input.
pub fn normalize_remote_url(raw: &str) -> String {
    let s = raw.trim();
    let s = s.strip_suffix(".git").unwrap_or(s);
    if let Some(rest) = s.strip_prefix("ssh://git@") {
        return format!("https://{rest}");
    }
    if let Some(rest) = s.strip_prefix("git@") {
        // `host:owner/repo` → `host/owner/repo` (only the first colon is the sep).
        return format!("https://{}", rest.replacen(':', "/", 1));
    }
    s.to_string()
}

/// Render the start-up info line shown under the branding header: the project name,
/// the current branch, and the repo web URL, joined by ` · `. Each present segment
/// gets an emoji prefix only when `opts.emoji`; a missing branch or URL is simply
/// omitted. The non-colour path emits no ANSI byte.
pub fn render_info_line(
    project: &str,
    branch: Option<&str>,
    url: Option<&str>,
    opts: RenderOpts,
) -> String {
    let seg = |emoji: &str, value: &str| -> String {
        if opts.emoji {
            format!("{emoji} {value}")
        } else {
            value.to_string()
        }
    };
    let mut parts: Vec<String> = vec![seg("📦", project)];
    if let Some(b) = branch {
        parts.push(seg("🌿", b));
    }
    if let Some(u) = url {
        parts.push(seg("🔗", u));
    }
    let line = parts.join(" · ");
    if opts.color {
        Style::new().dim().apply_to(&line).to_string()
    } else {
        line
    }
}

/// Choose the emoji or its ASCII fallback.
pub(crate) fn pick(emoji: &'static str, ascii: &'static str, use_emoji: bool) -> &'static str {
    if use_emoji {
        emoji
    } else {
        ascii
    }
}

/// `13s` or `2m05s`.
pub(crate) fn fmt_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}
