//! The attention list: the requests this daemon's own sources open
//! (docs/adr/0085 D10, D11, D13). Each daemon works out its own list; the
//! daemon the page talks to joins its peers' lists (`routes/api_attention.rs`).
//!
//! A request shows only what is in the agent state: the vendor, the console,
//! the kind, its age (`since`) and the one-line detail (D13). A run's request
//! comes from its snapshot, which [`tick`] reads from each registered repo.

use ralphy_run_snapshot::RunSnapshot;

use crate::session::SessionInfo;

pub(crate) mod tick;

/// The kind of a request. Matched exhaustively, so a new kind has to say
/// where it comes from and whether it may be dismissed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// A console whose agent state is `waiting`.
    ConsoleWaiting,
    /// A console whose hooks sent nothing: Ralphy cannot see its agent.
    ConsoleSilent,
    /// A run whose agent (`phase.agent` of its snapshot) is `waiting`.
    RunWaiting,
}

impl Kind {
    /// Whether the operator may dismiss a request of this kind. A live
    /// source cannot be dismissed: an agent that still waits stays listed
    /// (docs/adr/0085 D12).
    pub fn dismissable(self) -> bool {
        match self {
            Kind::ConsoleWaiting | Kind::ConsoleSilent | Kind::RunWaiting => false,
        }
    }
}

/// One request on the wire of `GET /api/attention`. `id` is
/// `console:<session id>` for a console and `run:<runid>` for a run; a peer's request reaches the page
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

/// The live run snapshots of one registered repo, under its registry key.
#[derive(Debug, Clone, PartialEq)]
pub struct RunSource {
    pub repo: String,
    pub snapshot: RunSnapshot,
}

/// The requests the consoles open, in the order of `sessions`, then the ones
/// the runs open, in the order of `runs`. `sessions` is the list as
/// `/api/sessions` shows it, already rendered at the time of the read.
pub fn list(sessions: &[SessionInfo], runs: &[RunSource]) -> Vec<AttentionRequest> {
    sessions
        .iter()
        .filter_map(console_request)
        .chain(runs.iter().filter_map(run_request))
        .collect()
}

/// One request per console whose shown agent state is `waiting`, and one per
/// silent console, with the silence reason as its detail.
fn console_request(info: &SessionInfo) -> Option<AttentionRequest> {
    let state = info.agent_state.as_ref()?;
    let (kind, detail) = if state.source.as_deref() == Some("silent") {
        (Kind::ConsoleSilent, state.reason.clone())
    } else if state.state.as_deref() == Some("waiting") {
        (Kind::ConsoleWaiting, state.detail.clone())
    } else {
        return None;
    };
    Some(AttentionRequest {
        id: format!("console:{}", info.id),
        kind,
        vendor: info.agent.clone(),
        console: Some(info.id.to_string()),
        repo: Some(info.repo.clone()),
        run: None,
        issue: None,
        since: state.since.clone(),
        detail,
        dismissable: kind.dismissable(),
    })
}

/// One request per run whose agent is `waiting`: the vendor of the phase the
/// run is in, and the issue it works on.
fn run_request(source: &RunSource) -> Option<AttentionRequest> {
    let snap = &source.snapshot;
    let agent = snap.phase.agent.as_ref()?;
    if agent.state != "waiting" {
        return None;
    }
    let kind = Kind::RunWaiting;
    let vendor = if snap.phase.state == "planning" {
        &snap.plan_agent
    } else {
        &snap.exec_agent
    };
    Some(AttentionRequest {
        id: format!("run:{}", snap.runid),
        kind,
        vendor: vendor.clone(),
        console: None,
        repo: Some(source.repo.clone()),
        run: Some(snap.runid.clone()),
        issue: snap.phase.active.map(|n| n.to_string()),
        since: agent.since.clone(),
        detail: agent.detail.clone(),
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
        let got = list(
            &[console(7, Some("waiting"), Some("permission: Bash"))],
            &[],
        );
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
            let got = list(&[console(1, state, None)], &[]);
            assert!(got.is_empty(), "{why}: {got:?}");
        }
    }

    fn run(runid: &str, phase: &str, agent: Option<(&str, Option<&str>)>) -> RunSource {
        let mut snapshot = RunSnapshot {
            runid: runid.to_string(),
            plan_agent: "claude".to_string(),
            exec_agent: "codex".to_string(),
            ..RunSnapshot::default()
        };
        snapshot.phase.state = phase.to_string();
        snapshot.phase.active = Some(71);
        snapshot.phase.agent = agent.map(|(state, detail)| ralphy_run_snapshot::AgentBlock {
            state: state.to_string(),
            since: "2026-10-28T11:00:00-03:00".to_string(),
            detail: detail.map(str::to_string),
        });
        RunSource {
            repo: "owner/r".to_string(),
            snapshot,
        }
    }

    #[test]
    fn a_waiting_run_is_one_request() {
        let got = list(
            &[],
            &[run(
                "01RUN",
                "executing",
                Some(("waiting", Some("permission: Bash"))),
            )],
        );
        assert_eq!(got.len(), 1, "{got:?}");
        let request = &got[0];
        assert_eq!(request.kind, Kind::RunWaiting);
        assert_eq!(request.id, "run:01RUN");
        assert_eq!(request.run.as_deref(), Some("01RUN"));
        assert_eq!(request.issue.as_deref(), Some("71"));
        assert_eq!(request.repo.as_deref(), Some("owner/r"));
        assert_eq!(
            request.vendor, "codex",
            "the executor runs the executing phase"
        );
        assert_eq!(request.since, "2026-10-28T11:00:00-03:00");
        assert_eq!(request.detail.as_deref(), Some("permission: Bash"));
        assert!(!request.dismissable);
        let wire = serde_json::to_value(request).expect("a request serializes");
        assert_eq!(wire["kind"], "run-waiting", "{wire}");
        // The planner runs the planning phase.
        let planning = list(&[], &[run("01P", "planning", Some(("waiting", None)))]);
        assert_eq!(planning[0].vendor, "claude", "{planning:?}");
        // A run whose agent does not wait, or has no agent state, opens none.
        for (agent, why) in [(Some(("working", None)), "working"), (None, "no state")] {
            let got = list(&[], &[run("01Q", "executing", agent)]);
            assert!(got.is_empty(), "{why}: {got:?}");
        }
    }

    #[test]
    fn a_silent_console_is_one_request() {
        let reason = "No hook event reached Ralphy.";
        let mut info = console(3, None, None);
        info.agent_state = Some(AgentState::silent(
            reason.to_string(),
            std::time::SystemTime::UNIX_EPOCH,
        ));
        let got = list(&[info], &[]);
        assert_eq!(got.len(), 1, "{got:?}");
        let request = &got[0];
        assert_eq!(request.kind, Kind::ConsoleSilent);
        assert_eq!(request.id, "console:3");
        assert_eq!(request.console.as_deref(), Some("3"));
        assert_eq!(request.detail.as_deref(), Some(reason));
        assert_eq!(request.since, "1970-01-01T00:00:00Z");
        assert!(!request.dismissable);
        let wire = serde_json::to_value(request).expect("a request serializes");
        assert_eq!(wire["kind"], "console-silent", "{wire}");
    }
}
