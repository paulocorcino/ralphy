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

fn peer_request() -> AttentionRequest {
    AttentionRequest {
        id: "console:3".to_string(),
        kind: Kind::ConsoleWaiting,
        vendor: "claude".to_string(),
        console: Some("owner/r#3".to_string()),
        name: None,
        repo: Some("owner/r".to_string()),
        run: None,
        issue: None,
        since: "2026-10-28T10:00:00-03:00".to_string(),
        detail: None,
        dismissable: false,
    }
}

#[test]
fn a_peer_request_for_a_person_keeps_the_repo_slug() {
    let got = from_peer("daemon1", peer_request(), PeerReader::Person);
    assert_eq!(got.id, "daemon1/console:3");
    assert_eq!(got.console.as_deref(), Some("daemon1/owner/r#3"));
    assert_eq!(got.repo.as_deref(), Some("owner/r"));
}

#[test]
fn a_peer_request_for_the_page_prefixes_the_repo() {
    let got = from_peer("daemon1", peer_request(), PeerReader::Page);
    assert_eq!(got.id, "daemon1/console:3");
    assert_eq!(got.console.as_deref(), Some("daemon1/owner/r#3"));
    assert_eq!(got.repo.as_deref(), Some("daemon1/owner/r"));
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
