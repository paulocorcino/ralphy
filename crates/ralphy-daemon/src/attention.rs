//! The attention list: the requests this daemon's own sources open
//! (docs/adr/0085 D10, D11, D13). Each daemon works out its own list; the
//! daemon the page talks to joins its peers' lists (`routes/api_attention.rs`).
//!
//! A request shows only what is in the agent state: the vendor, the console,
//! the kind, its age (`since`) and the one-line detail (D13). A run's request
//! comes from its snapshot, which [`tick`] reads from each registered repo; a
//! finished run's requests come from the final snapshot in its run folder,
//! and last until the operator dismisses them ([`dismissals`], D12), a newer
//! run works the same issue, or the run folder is removed.

use ralphy_run_snapshot::{IssueBlock, RunSnapshot};

use crate::agent_state::Unreported;
use crate::session::SessionInfo;

pub(crate) mod dismissals;
pub(crate) mod done;
mod finals;
pub(crate) mod telegram;
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
    /// A console that finished the turn the operator marked "tell me when it
    /// is done" for. It lasts until the operator opens the console or
    /// dismisses it.
    ConsoleDone,
    /// A run whose agent (`phase.agent` of its snapshot) is `waiting`.
    RunWaiting,
    /// An issue a finished run ended `blocked`, `hitl` or `needs_split`, or
    /// whose agent the idle watchdog stopped.
    RunFinished,
}

impl Kind {
    /// Whether the operator may dismiss a request of this kind. A live
    /// source cannot be dismissed: an agent that still waits stays listed
    /// (docs/adr/0085 D12).
    pub fn dismissable(self) -> bool {
        match self {
            Kind::ConsoleWaiting | Kind::ConsoleSilent | Kind::RunWaiting => false,
            Kind::ConsoleDone | Kind::RunFinished => true,
        }
    }
}

/// One request on the wire of `GET /api/attention`. `id` is
/// `console:<session id>` for a console, `done:<session id>:<n>` for the
/// n-th finished turn a console was marked for, `run:<runid>` for a run and
/// `issue:<run stamp>:<issue>` for a finished run's issue; a peer's request reaches the page
/// with `id`, `console` and `repo` prefixed `<daemon_id>/`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttentionRequest {
    pub id: String,
    pub kind: Kind,
    pub vendor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console: Option<String>,
    /// The console's name, when it has one; a main daemon's message for a
    /// peer's console names it from here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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

/// The final snapshot of a finished run of one registered repo, and the name
/// of its run folder.
#[derive(Debug, Clone, PartialEq)]
pub struct FinishedRun {
    pub repo: String,
    /// The run folder's name: `<repo>/.ralphy/runs/<stamp>/`.
    pub stamp: String,
    /// When the run ended (RFC 3339): when it wrote its final snapshot.
    pub ended: String,
    pub snapshot: RunSnapshot,
}

/// What the attention tick read of the runs of every registered repo, and
/// the done marks it fired.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunsRead {
    pub live: Vec<RunSource>,
    pub finished: Vec<FinishedRun>,
    pub done: Vec<done::DoneFired>,
}

/// The detail of a request for an issue whose agent the idle watchdog stopped.
pub const IDLE_REAPED_DETAIL: &str = "the idle watchdog stopped the agent";

/// The requests the consoles open, in the order of `sessions`, then the
/// finished turns of marked consoles, then the ones the live runs open, then the ones the finished runs open, in the order of
/// `runs`. `sessions` is the list as `/api/sessions` shows it, already
/// rendered at the time of the read. A request whose id `is_dismissed` is
/// not listed.
pub fn list(
    sessions: &[SessionInfo],
    runs: &RunsRead,
    is_dismissed: &dyn Fn(&str) -> bool,
) -> Vec<AttentionRequest> {
    sessions
        .iter()
        .filter_map(console_request)
        .chain(done::requests(sessions, &runs.done))
        .chain(runs.live.iter().filter_map(run_request))
        .chain(finished_requests(runs))
        .filter(|request| !is_dismissed(&request.id))
        .collect()
}

/// One request per console whose shown agent state is `waiting`, and one per
/// silent console, with the silence reason as its detail. A console with no
/// state source by design is none: it would stay open for its whole life.
fn console_request(info: &SessionInfo) -> Option<AttentionRequest> {
    let state = info.agent_state.as_ref()?;
    let (kind, detail) = match state.source {
        Some(Unreported::Silent) => (Kind::ConsoleSilent, state.reason.clone()),
        Some(Unreported::NoSource) => return None,
        None if state.state.as_deref() == Some("waiting") => {
            (Kind::ConsoleWaiting, state.detail.clone())
        }
        None => return None,
    };
    Some(AttentionRequest {
        id: format!("console:{}", info.id),
        kind,
        vendor: info.agent.clone(),
        console: Some(info.id.to_string()),
        name: info.name.clone(),
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
        name: None,
        repo: Some(source.repo.clone()),
        run: Some(snap.runid.clone()),
        issue: snap.phase.active.map(|n| n.to_string()),
        since: agent.since.clone(),
        detail: agent.detail.clone(),
        dismissable: kind.dismissable(),
    })
}

/// One request per issue of a finished run that needs the operator, unless a
/// newer run works the same issue.
fn finished_requests(runs: &RunsRead) -> impl Iterator<Item = AttentionRequest> + '_ {
    runs.finished.iter().flat_map(move |run| {
        run.snapshot
            .issues
            .iter()
            .filter(move |issue| !worked_again(runs, run, issue.number))
            .filter_map(move |issue| finished_request(run, issue))
    })
}

/// What a finished issue asks of the operator, if anything: an idle reap
/// first, else the status the run ended it with. `done`, `skipped`,
/// `non_green` and the other statuses ask nothing.
fn finished_detail(issue: &IssueBlock) -> Option<String> {
    if issue.idle_reaped {
        return Some(IDLE_REAPED_DETAIL.to_string());
    }
    matches!(issue.status.as_str(), "blocked" | "hitl" | "needs_split")
        .then(|| issue.status.clone())
}

fn finished_request(run: &FinishedRun, issue: &IssueBlock) -> Option<AttentionRequest> {
    let detail = finished_detail(issue)?;
    let kind = Kind::RunFinished;
    Some(AttentionRequest {
        id: format!("issue:{}:{}", run.stamp, issue.number),
        kind,
        vendor: run.snapshot.exec_agent.clone(),
        console: None,
        name: None,
        repo: Some(run.repo.clone()),
        run: Some(run.snapshot.runid.clone()),
        issue: Some(issue.number.to_string()),
        since: run.ended.clone(),
        detail: Some(detail),
        dismissable: kind.dismissable(),
    })
}

/// Whether a run of the same repo that started after `run`, live or
/// finished, has `number` in its issues. The order is the snapshots' own
/// `started_at`; a run whose start cannot be read is never "after".
fn worked_again(runs: &RunsRead, run: &FinishedRun, number: u64) -> bool {
    let started = |snap: &RunSnapshot| chrono::DateTime::parse_from_rfc3339(&snap.started_at).ok();
    let Some(this) = started(&run.snapshot) else {
        return false;
    };
    let live = runs.live.iter().map(|r| (&r.repo, &r.snapshot));
    let finished = runs.finished.iter().map(|f| (&f.repo, &f.snapshot));
    live.chain(finished).any(|(repo, snap)| {
        *repo == run.repo
            && snap.issues.iter().any(|i| i.number == number)
            && started(snap).is_some_and(|at| at > this)
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
            &RunsRead::default(),
            &|_| false,
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
            let got = list(&[console(1, state, None)], &RunsRead::default(), &|_| false);
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

    fn live(source: RunSource) -> RunsRead {
        RunsRead {
            live: vec![source],
            ..RunsRead::default()
        }
    }

    #[test]
    fn a_waiting_run_is_one_request() {
        let got = list(
            &[],
            &live(run(
                "01RUN",
                "executing",
                Some(("waiting", Some("permission: Bash"))),
            )),
            &|_| false,
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
        let planning = list(
            &[],
            &live(run("01P", "planning", Some(("waiting", None)))),
            &|_| false,
        );
        assert_eq!(planning[0].vendor, "claude", "{planning:?}");
        // A run whose agent does not wait, or has no agent state, opens none.
        for (agent, why) in [(Some(("working", None)), "working"), (None, "no state")] {
            let got = list(&[], &live(run("01Q", "executing", agent)), &|_| false);
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
        let got = list(&[info], &RunsRead::default(), &|_| false);
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

    /// A console launched with no state source on purpose shows its reason,
    /// but is not a request; a console that went silent next to it still is.
    #[test]
    fn a_console_with_no_state_source_by_design_is_no_request() {
        let at = std::time::SystemTime::UNIX_EPOCH;
        let mut by_design = console(4, None, None);
        by_design.agent_state = Some(AgentState::unreported(
            Unreported::NoSource,
            "This project has its own Codex hooks.".to_string(),
            at,
        ));
        let mut went_silent = console(5, None, None);
        went_silent.agent_state = Some(AgentState::silent("No hook event.".to_string(), at));
        let got = list(&[by_design, went_silent], &RunsRead::default(), &|_| false);
        let ids: Vec<&str> = got.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["console:5"], "{got:?}");
    }

    /// A finished run of `repo` in the folder `stamp`, started at `started`,
    /// whose issues are `(number, status)`.
    fn finished(repo: &str, stamp: &str, started: &str, issues: &[(u64, &str)]) -> FinishedRun {
        FinishedRun {
            repo: repo.to_string(),
            stamp: stamp.to_string(),
            ended: "2026-10-28T12:00:00Z".to_string(),
            snapshot: RunSnapshot {
                runid: format!("01{stamp}"),
                exec_agent: "codex".to_string(),
                started_at: started.to_string(),
                issues: issues
                    .iter()
                    .map(|&(number, status)| IssueBlock {
                        number,
                        status: status.to_string(),
                        ..IssueBlock::default()
                    })
                    .collect(),
                ..RunSnapshot::default()
            },
        }
    }

    fn finished_only(runs: Vec<FinishedRun>) -> RunsRead {
        RunsRead {
            finished: runs,
            ..RunsRead::default()
        }
    }

    const STAMP: &str = "20261028-100000";
    const STARTED: &str = "2026-10-28T10:00:00-03:00";

    #[test]
    fn finished_issue_statuses_open_requests_by_the_table() {
        // (status as the snapshot writes it, requests it opens)
        let rows: [(&str, usize); 6] = [
            ("blocked", 1),
            ("hitl", 1),
            ("needs_split", 1),
            ("done", 0),
            ("skipped", 0),
            ("non_green", 0),
        ];
        for (status, count) in rows {
            let runs = finished_only(vec![finished("owner/r", STAMP, STARTED, &[(71, status)])]);
            let got = list(&[], &runs, &|_| false);
            assert_eq!(got.len(), count, "{status}: {got:?}");
            let Some(request) = got.first() else { continue };
            assert_eq!(request.kind, Kind::RunFinished, "{status}");
            assert_eq!(request.id, format!("issue:{STAMP}:71"), "{status}");
            assert_eq!(request.detail.as_deref(), Some(status), "{status}");
            assert_eq!(request.issue.as_deref(), Some("71"), "{status}");
            assert_eq!(
                request.run.as_deref(),
                Some("0120261028-100000"),
                "{status}"
            );
            assert_eq!(request.repo.as_deref(), Some("owner/r"), "{status}");
            assert_eq!(request.vendor, "codex", "{status}");
            assert_eq!(request.since, "2026-10-28T12:00:00Z", "{status}");
            assert!(request.dismissable, "{status}");
            let wire = serde_json::to_value(request).expect("a request serializes");
            assert_eq!(wire["kind"], "run-finished", "{wire}");
        }
    }

    #[test]
    fn an_idle_reaped_agent_is_one_request() {
        let mut run = finished(
            "owner/r",
            STAMP,
            STARTED,
            &[(71, "non_green"), (72, "done")],
        );
        run.snapshot.issues[0].idle_reaped = true;
        let got = list(&[], &finished_only(vec![run]), &|_| false);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].kind, Kind::RunFinished);
        assert_eq!(got[0].id, format!("issue:{STAMP}:71"));
        assert_eq!(got[0].detail.as_deref(), Some(IDLE_REAPED_DETAIL));
        assert!(got[0].dismissable);
    }

    #[test]
    fn a_new_run_of_the_same_issue_ends_the_request() {
        let old = || finished("owner/r", STAMP, STARTED, &[(71, "blocked")]);
        let later = "2026-10-28T11:00:00-03:00";
        // A later finished run of the same issue in the same repo ends it.
        let runs = finished_only(vec![
            old(),
            finished("owner/r", "20261028-110000", later, &[(71, "done")]),
        ]);
        assert!(list(&[], &runs, &|_| false).is_empty());
        // So does a live run that works it.
        let mut working = run("01LIVE", "executing", None);
        working.snapshot.started_at = later.to_string();
        working.snapshot.issues = vec![IssueBlock {
            number: 71,
            status: "executing".to_string(),
            ..IssueBlock::default()
        }];
        let runs = RunsRead {
            live: vec![working.clone()],
            finished: vec![old()],
            ..RunsRead::default()
        };
        assert!(list(&[], &runs, &|_| false).is_empty());
        // The same number in another repo does not.
        working.repo = "owner/other".to_string();
        let runs = RunsRead {
            live: vec![working],
            finished: vec![
                old(),
                finished("owner/other", "20261028-110000", later, &[(71, "done")]),
            ],
            ..RunsRead::default()
        };
        let got = list(&[], &runs, &|_| false);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].id, format!("issue:{STAMP}:71"));
        // An earlier run of the same issue does not end it either.
        let runs = finished_only(vec![
            old(),
            finished(
                "owner/r",
                "20261028-090000",
                "2026-10-28T09:00:00-03:00",
                &[(71, "done")],
            ),
        ]);
        assert_eq!(list(&[], &runs, &|_| false).len(), 1);
    }
}
