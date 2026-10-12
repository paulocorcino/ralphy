//! `/ws/session` and `/api/sessions*`: launch, reattach, watch and close the
//! daemon-owned sessions, locally or relayed to the peer that owns them.

use std::path::PathBuf;

mod bridge;
mod gemini_root;
mod join;
mod peer_console;
mod refuse;
mod relay;
mod traffic;
mod upgrade;

pub(crate) use bridge::*;
pub(crate) use relay::*;
pub(crate) use upgrade::session_ws_upgrade;

use crate::device::DeviceId;
use crate::identity;
use traffic::{EffectiveType, Link, Tab};

#[cfg(test)]
mod tests;

/// Query for `/ws/session`. A NEW agent launch carries `repo` + `agent`; a NEW
/// free-console launch (issue #167) carries `console=1` and an optional `repo`
/// (home dir when absent); a REATTACH carries `id` (and optional `takeover=1`,
/// or `watch=1` for a read-only attach, issue #334). All optional so one struct
/// serves every shape; the handler dispatches on `id` first, then `console`.
#[derive(Default, serde::Deserialize)]
pub(crate) struct SessionQuery {
    pub(crate) repo: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) id: Option<u64>,
    pub(crate) takeover: Option<u32>,
    pub(crate) watch: Option<u32>,
    pub(crate) console: Option<u32>,
    /// A worktree NAME beside `repo`+`agent` on a NEW agent launch (ADR-0063
    /// §3); ignored on a reattach — the record owns it — and on `console=1`.
    pub(crate) checkout: Option<String>,
    /// The startup command of a `console=1` launch: the shell runs it and the
    /// session ends with it. Ignored on every other path. Whitespace-only is
    /// the same as absent.
    pub(crate) command: Option<String>,
    /// The browser tab's holder id, on a launch or a writer reattach: a
    /// reattach naming the holder that claimed the slot reclaims it without
    /// `takeover` (ADR-0051 §9 amendment 2026-09-22). Ignored on `watch=1`.
    pub(crate) holder: Option<String>,
    /// The console name on a NEW agent launch. Claude takes its folded form as
    /// `--name` when the repo opts in; a name that folds to nothing keeps the
    /// hex name. Ignored on every other path. Empty is
    /// the same as absent; longer than 40 characters is cut to 40.
    pub(crate) name: Option<String>,
    /// The window record a NEW launch is for (ADR-0050 amendment 2026-10-04):
    /// while a live session serves it, the launch attaches to that session
    /// instead of starting another. Ignored on a reattach by `id`.
    pub(crate) record: Option<String>,
    /// `1`: the page can decode a gzip replay, so the daemon that hosts the
    /// session may send the replay frame gzipped (ADR-0079 D9). Live frames
    /// are never compressed.
    pub(crate) gzip: Option<u32>,
    /// The tab's attempt number for this claim, beside `holder`: it only grows
    /// per holder, so a lower one than the slot's writer is a late socket the
    /// tab already replaced (ADR-0079 D7). Ignored without a holder.
    pub(crate) attempt: Option<u64>,
    /// On a reattach by `id`: the count of the session's output bytes the
    /// tab's terminal already shows. The daemon then sends only the bytes
    /// after it, when it still has them all, and says so in `session-open`
    /// (ADR-0079 D8).
    pub(crate) since: Option<u64>,
    /// The network the browser reported (`navigator.connection`): its
    /// `effectiveType`, its `rtt` in ms, and its `downlink` in Mbit/s. Only
    /// the traffic summary reads them. Strings, so that a bad value is absent
    /// and does not refuse the socket.
    pub(crate) net: Option<String>,
    pub(crate) net_rtt: Option<String>,
    pub(crate) net_down: Option<String>,
}

/// The shape of a `holder` and of a `record`: 1–64 ASCII letters, digits, `-`
/// or `_`. Anything else is treated as absent.
fn well_formed_key(key: &str) -> bool {
    (1..=64).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl SessionQuery {
    /// The holder, when it is a well-formed one: 1–64 ASCII letters, digits,
    /// `-` or `_`. Anything else is treated as absent — it can only lose the
    /// reclaim, never gain one.
    pub(crate) fn holder(&self) -> Option<&str> {
        self.holder.as_deref().filter(|h| well_formed_key(h))
    }

    /// The window record, when it is a well-formed one (the `holder` shape).
    pub(crate) fn record(&self) -> Option<&str> {
        self.record.as_deref().filter(|r| well_formed_key(r))
    }

    /// The browser tab that opens the socket, as this query names it.
    /// `device` is the device of the request that carries the query.
    pub(crate) fn tab(&self, device: Option<DeviceId>) -> Tab {
        Tab {
            holder: self.holder().map(str::to_owned),
            device,
            gzip: self.gzip == Some(1),
            link: self.link(),
        }
    }

    /// The network the browser reported. A value that does not parse, or
    /// that no browser gives, is absent: it is never guessed.
    pub(crate) fn link(&self) -> Link {
        Link {
            effective_type: self.net.as_deref().and_then(EffectiveType::parse),
            rtt_ms: self.net_rtt.as_deref().and_then(|v| v.parse().ok()),
            downlink_mbps: self
                .net_down
                .as_deref()
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|d| d.is_finite() && *d >= 0.0),
        }
    }

    /// The console name, cut to the desk's limit on a char boundary so the
    /// launch and the desk agree on what the name is.
    pub(crate) fn name(&self) -> Option<&str> {
        let name = self.name.as_deref().filter(|n| !n.is_empty())?;
        Some(
            match name.char_indices().nth(crate::desk::CONSOLE_NAME_MAX) {
                Some((cut, _)) => &name[..cut],
                None => name,
            },
        )
    }
}

/// The two labels a `session-open` frame carries beside the identity: the
/// vendor-side name (Claude only) and the worktree the console lives in. They
/// travel together on every path — launch, reattach, watch.
#[derive(Default)]
pub(crate) struct SessionLabels {
    pub(crate) name: Option<String>,
    pub(crate) checkout: Option<String>,
    /// The socket was attached read-only: a launch whose record another page
    /// already drives (ADR-0050 amendment 2026-10-04).
    pub(crate) watching: bool,
}

#[derive(Clone)]
pub(crate) struct SessionHost {
    pub(crate) peers_dir: PathBuf,
    pub(crate) identity: Option<identity::Identity>,
    pub(crate) environment: String,
    /// The port this daemon bound, so a peer probe can refuse to dial itself.
    pub(crate) bound_port: u16,
}
