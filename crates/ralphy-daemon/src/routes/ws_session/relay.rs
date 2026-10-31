//! The peer relay of a console: the open the owning daemon receives on a
//! channel of its page socket, and the dial of that channel.

use std::ops::ControlFlow;
use std::time::Instant;

use axum::http::StatusCode;
use serde_json::{json, Map, Value};

use super::traffic::Tab;
// The console channel of a peer's session logs the same traffic line.
pub(crate) use super::traffic::Traffic;
use super::upgrade::Opened;
use super::{PeerRelay, SessionQuery};
use crate::mux::{Out, Permit, WriterClosed};
use crate::peer;
use crate::routes::audit_layer::{ConsoleRecord, SocketAudit};

/// The `end` of a relayed console the peer closed: why it closed is in the
/// peer's own log line.
pub(crate) const PEER_CLOSED: &str = "peer-closed";

/// Whether the relay goes on. `Break(true)`: the other side must be told.
type Flow = ControlFlow<bool>;

/// The fields of the `console` channel open a peer gets for `query`: the
/// same fields as `query`, under the same rules, with `slug` as the repo.
pub(crate) fn peer_session_open(query: &SessionQuery, slug: &str) -> Value {
    let mut fields = Map::new();
    if let Some(id) = query.id {
        reattach_fields(&mut fields, query, id, slug);
    } else if query.console == Some(1) {
        // A free console on a peer with no distro (ADR-0067 §7): the peer
        // opens it in its own checkout of `slug`.
        console_fields(&mut fields, query, slug);
    } else {
        launch_fields(&mut fields, query, slug);
    }
    Value::Object(fields)
}

/// The fields of a reattach to session `id`.
fn reattach_fields(fields: &mut Map<String, Value>, query: &SessionQuery, id: u64, slug: &str) {
    fields.insert("id".into(), json!(id));
    fields.insert("repo".into(), json!(slug));
    if query.takeover == Some(1) {
        fields.insert("takeover".into(), json!(1));
    }
    if query.watch == Some(1) {
        fields.insert("watch".into(), json!(1));
    }
    push_holder(fields, query);
    push_gzip(fields, query);
    push_since(fields, query);
    push_link(fields, query);
}

/// The fields of a free console, with its startup command.
fn console_fields(fields: &mut Map<String, Value>, query: &SessionQuery, slug: &str) {
    fields.insert("console".into(), json!(1));
    fields.insert("repo".into(), json!(slug));
    if let Some(command) = query.command.as_deref().map(str::trim) {
        if !command.is_empty() {
            fields.insert("command".into(), json!(command));
        }
    }
    push_record(fields, query);
    push_holder(fields, query);
    push_gzip(fields, query);
    push_link(fields, query);
}

/// The fields of a new agent launch.
fn launch_fields(fields: &mut Map<String, Value>, query: &SessionQuery, slug: &str) {
    fields.insert("repo".into(), json!(slug));
    let agent = query.agent.as_deref().unwrap_or_default();
    fields.insert("agent".into(), json!(agent));
    // The owning daemon resolves the name against ITS registry.
    if let Some(checkout) = query.checkout.as_deref() {
        fields.insert("checkout".into(), json!(checkout));
    }
    // The owning daemon folds it (ADR-0066 §6).
    if let Some(name) = query.name() {
        fields.insert("name".into(), json!(name));
    }
    push_record(fields, query);
    push_holder(fields, query);
    push_gzip(fields, query);
    push_link(fields, query);
}

/// The daemon that hosts the session is the one that compresses the replay,
/// so it must know the page can decode it.
fn push_gzip(fields: &mut Map<String, Value>, query: &SessionQuery) {
    if query.gzip == Some(1) {
        fields.insert("gzip".into(), json!(1));
    }
}

/// The owning daemon keeps the session's output, so it is the one that can
/// resume a reattach at the tab's byte count (ADR-0079 D8).
fn push_since(fields: &mut Map<String, Value>, query: &SessionQuery) {
    if let Some(since) = query.since {
        fields.insert("since".into(), json!(since));
    }
}

/// The network the browser reported, so the peer's own traffic summary has
/// it too. Only the parsed values go on, as the strings the peer reads.
fn push_link(fields: &mut Map<String, Value>, query: &SessionQuery) {
    let link = query.link();
    if let Some(kind) = link.effective_type {
        fields.insert("net".into(), json!(kind.as_str()));
    }
    if let Some(rtt) = link.rtt_ms {
        fields.insert("net_rtt".into(), json!(rtt.to_string()));
    }
    if let Some(down) = link.downlink_mbps {
        fields.insert("net_down".into(), json!(down.to_string()));
    }
}

/// The owning daemon keeps one session per window record (ADR-0050 amendment
/// 2026-10-04), so it must know the record of a launch.
fn push_record(fields: &mut Map<String, Value>, query: &SessionQuery) {
    if let Some(record) = query.record() {
        fields.insert("record".into(), json!(record));
    }
}

/// The owning daemon keeps the writer slot, so it is the one that must know
/// the holder (ADR-0051 §9 amendment 2026-09-22) and the attempt of its claim
/// (ADR-0079 D7).
fn push_holder(fields: &mut Map<String, Value>, query: &SessionQuery) {
    if let Some(holder) = query.holder() {
        fields.insert("holder".into(), json!(holder));
        if let Some(attempt) = query.attempt {
            fields.insert("attempt".into(), json!(attempt));
        }
    }
}

/// The tab that opens a relayed console, and the audit line the opening
/// writes once the peer accepts it: a launch or a take-over, or `None` for a
/// plain reattach or a watch.
pub(crate) struct Opening {
    pub(crate) tab: Tab,
    pub(crate) who: SocketAudit,
    pub(crate) record: Option<ConsoleRecord>,
}

/// The audit line of a relayed open, written once the peer accepts the
/// open, on its first frame. A refused open writes none.
pub(crate) struct PendingRecord {
    who: SocketAudit,
    record: ConsoleRecord,
}

impl PendingRecord {
    pub(crate) fn write(self) {
        self.who.console(&self.record);
    }
}

/// The peer to dial, the open to send it, and who this daemon says it is.
pub(crate) struct PeerDial<'a> {
    pub(crate) peer: &'a peer::PeerDescriptor,
    pub(crate) fields: Value,
    pub(crate) me: peer::client::SelfRef<'a>,
}

/// Open a `console` channel with `dial.fields` on the peer that owns the
/// session, for the page's channel to relay to. A refused dial is refused
/// with the peer's diagnosis (`502` on a reattach); an HTTP refusal from the
/// peer keeps its own status and body. The peer refuses the open itself
/// later, on the channel.
pub(crate) async fn dial_peer(dial: PeerDial<'_>, opening: Opening) -> Opened {
    let PeerDial { peer, fields, me } = dial;
    match peer::client::open_channel(peer, me, "console", fields).await {
        Ok(channel) => {
            let Opening { tab, who, record } = opening;
            let traffic = Traffic::peer(
                peer.daemon_id.clone(),
                peer.environment.clone(),
                tab,
                Instant::now(),
            );
            Opened::Relay(Box::new(PeerRelay {
                peer: channel,
                traffic,
                record: record.map(|record| PendingRecord { who, record }),
            }))
        }
        Err(peer::client::SocketError::Peer(status)) => {
            Opened::refused(StatusCode::BAD_GATEWAY, status.diagnosis(&peer.environment))
        }
        Err(peer::client::SocketError::Http { status, body }) => Opened::refused(
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            body,
        ),
    }
}

/// Hand the held message to the writer. A writer that has ended means the
/// page is gone: the relay ends, with nobody to tell.
pub(crate) fn hand_on(permit: Result<Permit, WriterClosed>, held: &mut Option<Out>) -> Flow {
    match permit {
        Ok(permit) => {
            if let Some(out) = held.take() {
                permit.put(out);
            }
            ControlFlow::Continue(())
        }
        Err(WriterClosed) => ControlFlow::Break(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The network the browser reported goes to the peer on every shape, so
    /// the peer's traffic summary has it too. A value no browser gives is
    /// absent, and does not refuse the open.
    #[test]
    fn peer_session_open_forwards_the_link_the_browser_reported() {
        let reattach = |net: &str, rtt: &str, down: &str| SessionQuery {
            repo: Some("x".into()),
            id: Some(7),
            net: Some(net.into()),
            net_rtt: Some(rtt.into()),
            net_down: Some(down.into()),
            ..SessionQuery::default()
        };
        assert_eq!(
            peer_session_open(&reattach("4g", "100", "1.75"), "owner/repo"),
            json!({
                "id": 7, "repo": "owner/repo",
                "net": "4g", "net_rtt": "100", "net_down": "1.75"
            })
        );
        let console = SessionQuery {
            id: None,
            console: Some(1),
            ..reattach("slow-2g", "2000", "0")
        };
        assert_eq!(
            peer_session_open(&console, "owner/repo"),
            json!({
                "console": 1, "repo": "owner/repo",
                "net": "slow-2g", "net_rtt": "2000", "net_down": "0"
            })
        );
        let launch = SessionQuery {
            id: None,
            agent: Some("claude".into()),
            ..reattach("2g", "50", "10")
        };
        assert_eq!(
            peer_session_open(&launch, "owner/repo"),
            json!({
                "repo": "owner/repo", "agent": "claude",
                "net": "2g", "net_rtt": "50", "net_down": "10"
            })
        );
        assert_eq!(
            peer_session_open(&reattach("3g", "x", "0.5"), "owner/repo"),
            json!({"id": 7, "repo": "owner/repo", "net": "3g", "net_down": "0.5"}),
            "each value is read alone"
        );
        for (net, rtt, down) in [("5g", "-1", "NaN"), ("wifi", "x", "-2"), ("", "1e9", "inf")] {
            assert_eq!(
                peer_session_open(&reattach(net, rtt, down), "owner/repo"),
                json!({"id": 7, "repo": "owner/repo"}),
                "{net} {rtt} {down}"
            );
        }
    }
}
