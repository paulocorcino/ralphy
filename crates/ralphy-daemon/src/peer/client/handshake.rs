//! The version handshake: dial a peer's `/api/peer/hello` and read what came
//! back as a [`PeerStatus`].

use super::{classify_address, classify_self_dial, diagnose_failed_dial, get, PeerStatus, SelfRef};
use crate::peer::{PeerDescriptor, PEER_PROTOCOL_VERSION};

pub(super) async fn handshake(
    d: &PeerDescriptor,
    me: SelfRef<'_>,
    environment: &mut Option<String>,
) -> PeerStatus {
    if let Some(refused) = classify_address(&d.address) {
        return refused;
    }
    if let Some(refused) = classify_self_dial(&d.address, d.port, me) {
        return refused;
    }
    let (status, body) = match get(d, "/api/peer/hello").await {
        Ok(pair) => pair,
        Err(e) => return diagnose_failed_dial(d, format!("{e:#}")).await,
    };
    // Any HTTP answer came through the tunnel, so the tunnel works.
    if d.tunnel.is_some() {
        crate::peer::tunnel::tunnels().answered(&d.daemon_id);
    }
    if let Some(verdict) = http_verdict(status, d.tunnel.is_some()) {
        return verdict;
    }
    let hello = serde_json::from_slice::<serde_json::Value>(&body).ok();
    *environment = hello_environment(hello.as_ref());
    let answered_id = hello
        .as_ref()
        .and_then(|v| v.get("daemon_id").and_then(|id| id.as_str()));
    if let Some(refused) = identity_verdict(d, me, answered_id) {
        return refused;
    }
    protocol_verdict(hello.as_ref())
}

/// The status a non-200 answer to the handshake decides, or `None` for a 200.
fn http_verdict(status: u16, tunnel: bool) -> Option<PeerStatus> {
    if status == 401 || status == 403 {
        return Some(PeerStatus::Unauthorized { tunnel });
    }
    if status != 200 {
        return Some(PeerStatus::Unreachable {
            why: format!("peer answered HTTP {status} to the handshake"),
        });
    }
    None
}

/// The environment label the peer gave, when it gave a non-empty one.
fn hello_environment(hello: Option<&serde_json::Value>) -> Option<String> {
    hello
        .and_then(|v| v.get("environment").and_then(|e| e.as_str()))
        .filter(|e| !e.is_empty())
        .map(str::to_string)
}

/// A refusal when the daemon that answered is not the one `d` names: this
/// daemon itself, or another daemon on the same port.
fn identity_verdict(
    d: &PeerDescriptor,
    me: SelfRef<'_>,
    answered: Option<&str>,
) -> Option<PeerStatus> {
    // Belt and braces for the gate above: a handshake that answers with THIS
    // daemon's own id came back to us by some route the port check did not
    // model, and treating it as a peer would federate this daemon with itself.
    // Skipped when un-baptized, where the empty id is not an identity.
    if !me.daemon_id.is_empty() && answered == Some(me.daemon_id) {
        return Some(PeerStatus::Refused {
            why: format!(
                "the handshake at {}:{} answered with this daemon's own identity — \
                 the connection loops back here rather than reaching another environment",
                d.address, d.port
            ),
        });
    }
    // ADR-0067 amendment M5: several daemons can share one host, so a port can
    // answer with another daemon than the one this descriptor names. Skipped
    // when either side has no id.
    let answered = answered?;
    if d.daemon_id.is_empty() || answered == d.daemon_id {
        return None;
    }
    // Only `host add` writes a tunnel descriptor, and running it again
    // reads the identity and the socket anew.
    let remedy = if d.tunnel.is_some() {
        ". Add the host again"
    } else {
        ""
    };
    Some(PeerStatus::Refused {
        why: format!(
            "another daemon answers at {}:{}: it is {answered}, not {}{remedy}",
            d.address, d.port, d.daemon_id
        ),
    })
}

/// Whether the peer speaks this daemon's protocol version.
fn protocol_verdict(hello: Option<&serde_json::Value>) -> PeerStatus {
    // A value past u32::MAX is no protocol version at all, so it counts as absent.
    let theirs = hello
        .and_then(|v| v.get("protocol_version"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u32::try_from(v).ok());
    match theirs {
        Some(v) if v == PEER_PROTOCOL_VERSION => PeerStatus::Reachable,
        Some(v) => PeerStatus::VersionMismatch {
            theirs: v,
            ours: PEER_PROTOCOL_VERSION,
        },
        None => PeerStatus::Unreachable {
            why: "the handshake answered without a protocol_version".to_string(),
        },
    }
}

#[cfg(test)]
mod tests;
