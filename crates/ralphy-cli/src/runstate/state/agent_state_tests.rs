use super::*;

fn waiting() -> RunEvent {
    RunEvent::AgentState {
        state: "waiting".into(),
        since: "t1".into(),
        detail: Some("which port?".into()),
    }
}

/// ADR-0059: the fold keeps the last reported state verbatim, and drops
/// it when the child it came from is gone — a new issue, a reap.
#[test]
fn agent_state_is_kept_verbatim_and_cleared_when_the_child_is_gone() {
    let mut s = RunState::new("t", 1);
    assert_eq!(s.agent, None);
    s.apply(RunEvent::IssueStarted {
        number: 7,
        title: "x".into(),
    });
    s.apply(waiting());
    assert_eq!(
        s.agent,
        Some(AgentState {
            state: "waiting".into(),
            since: "t1".into(),
            detail: Some("which port?".into()),
        })
    );
    s.apply(RunEvent::AgentState {
        state: "done".into(),
        since: "t2".into(),
        detail: None,
    });
    assert_eq!(s.agent.as_ref().map(|a| a.state.as_str()), Some("done"));
    s.apply(RunEvent::IdleReaped { idle_minutes: 5 });
    assert_eq!(s.agent, None, "a reap clears it");
    s.apply(waiting());
    s.apply(RunEvent::IssueStarted {
        number: 8,
        title: "y".into(),
    });
    assert_eq!(s.agent, None, "a new issue clears it");
    // Unrelated to the phase vocabulary: the issue stays where it was.
    assert_eq!(s.run_phase(), "planning");
}

/// ADR-0059 §1: "a run in `sleeping` has no agent at all" — and neither
/// has a run between issues, after a non-green stop, a human block, a
/// split, the deadline or the operator's stop. Each ends the child; each
/// must drop the state that child reported.
#[test]
fn every_event_that_ends_the_child_clears_the_agent_state() {
    let ending: Vec<(&str, RunEvent)> = vec![
        (
            "IssueClosed",
            RunEvent::IssueClosed {
                number: 7,
                tokens: 0,
                invocations: 0,
                usage: super::super::UsageLite::default(),
            },
        ),
        (
            "NonGreen",
            RunEvent::NonGreen {
                number: 7,
                outcome: "Timeout".into(),
            },
        ),
        (
            "HumanBlocked",
            RunEvent::HumanBlocked {
                number: 7,
                on: vec![30],
            },
        ),
        ("NeedsSplit", RunEvent::NeedsSplit { number: 7 }),
        ("DeadlinePassed", RunEvent::DeadlinePassed { number: 8 }),
        ("RunStopped", RunEvent::RunStopped { number: 7 }),
        (
            "SleepStarted",
            RunEvent::SleepStarted {
                reset: "10pm".into(),
                target_epoch: 1,
            },
        ),
        ("IdleReaped", RunEvent::IdleReaped { idle_minutes: 5 }),
    ];
    for (name, ev) in ending {
        let mut s = RunState::new("t", 1);
        s.apply(RunEvent::IssueStarted {
            number: 7,
            title: "x".into(),
        });
        s.apply(waiting());
        assert!(s.agent.is_some(), "{name}: precondition");
        s.apply(ev);
        assert_eq!(s.agent, None, "{name} must clear the agent state");
    }
    // Whereas the events that do NOT end the child keep it.
    let mut s = RunState::new("t", 1);
    s.apply(RunEvent::IssueStarted {
        number: 7,
        title: "x".into(),
    });
    s.apply(waiting());
    s.apply(RunEvent::ApiDegraded);
    s.apply(RunEvent::PlanWritten {
        number: 7,
        open_steps: 3,
        usage: super::super::UsageLite::default(),
        steps: Vec::new(),
    });
    assert!(
        s.agent.is_some(),
        "a plan written mid-run is not the child's end"
    );
}
