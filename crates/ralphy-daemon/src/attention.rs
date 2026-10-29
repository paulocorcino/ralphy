//! The attention list: the requests this daemon's own sources open
//! (docs/adr/0085 D10, D11, D13). Each daemon works out its own list; the
//! daemon the page talks to joins its peers' lists (`routes/api_attention.rs`).
//!
//! A request shows only what is in the agent state: the vendor, the console,
//! the kind, its age (`since`) and the one-line detail (D13).

use crate::session::SessionInfo;

/// The kind of a request. Matched exhaustively, so a new kind has to say
/// where it comes from and whether it may be dismissed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// A console whose agent state is `waiting`.
    ConsoleWaiting,
}

impl Kind {
    /// Whether the operator may dismiss a request of this kind. A live
    /// source cannot be dismissed: an agent that still waits stays listed
    /// (docs/adr/0085 D12).
    pub fn dismissable(self) -> bool {
        match self {
            Kind::ConsoleWaiting => false,
        }
    }
}

/// One request on the wire of `GET /api/attention`. `id` is
/// `console:<session id>` for a console; a peer's request reaches the page
/// with `id`, `console` and `repo` prefixed `<daemon_id>/`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttentionRequest {
    pub id: String,
    pub kind: Kind,
    pub vendor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    /// When the request opened: the agent state's own `since`.
    pub since: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub dismissable: bool,
}

/// The requests the consoles open, in the order of `sessions`: one per
/// console whose shown agent state is `waiting`. `sessions` is the list as
/// `/api/sessions` shows it, already rendered at the time of the read.
pub fn list(sessions: &[SessionInfo]) -> Vec<AttentionRequest> {
    sessions.iter().filter_map(console_request).collect()
}

fn console_request(info: &SessionInfo) -> Option<AttentionRequest> {
    let state = info.agent_state.as_ref()?;
    if state.state.as_deref() != Some("waiting") {
        return None;
    }
    let kind = Kind::ConsoleWaiting;
    Some(AttentionRequest {
        id: format!("console:{}", info.id),
        kind,
        vendor: info.agent.clone(),
        console: Some(info.id.to_string()),
        repo: Some(info.repo.clone()),
        run: None,
        issue: None,
        since: state.since.clone(),
        detail: state.detail.clone(),
        dismissable: kind.dismissable(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_state::AgentState;

    fn console(id: u64, state: Option<&str>, detail: Option<&str>) -> SessionInfo {
        SessionInfo {
            id,
            repo: "owner/r".to_string(),
            agent: "claude".to_string(),
            kind: "agent".to_string(),
            started_at: 1,
            environment: None,
            name: None,
            checkout: None,
            record: None,
            agent_state: state.map(|s| AgentState {
                state: Some(s.to_string()),
                since: "2026-10-28T10:00:00-03:00".to_string(),
                detail: detail.map(str::to_string),
                source: None,
                reason: None,
            }),
        }
    }

    #[test]
    fn a_waiting_console_is_one_request() {
        let got = list(&[console(7, Some("waiting"), Some("permission: Bash"))]);
        assert_eq!(got.len(), 1, "{got:?}");
        let request = &got[0];
        assert_eq!(request.kind, Kind::ConsoleWaiting);
        assert_eq!(request.id, "console:7");
        assert_eq!(request.detail.as_deref(), Some("permission: Bash"));
        assert!(!request.dismissable);
        let wire = serde_json::to_value(request).expect("a request serializes");
        assert_eq!(wire["kind"], "console-waiting", "{wire}");
    }

    #[test]
    fn only_a_waiting_console_opens_a_request() {
        for (state, why) in [
            (Some("working"), "working"),
            (Some("done"), "done"),
            (Some("unknown"), "unknown"),
            (None, "no state"),
        ] {
            let got = list(&[console(1, state, None)]);
            assert!(got.is_empty(), "{why}: {got:?}");
        }
    }
}
