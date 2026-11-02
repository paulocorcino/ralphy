use super::*;

/// A map for a vendor that sends an interrupt event (D7.4).
fn interrupting(line: &StatusLine) -> Option<Signal> {
    match line.event.as_str() {
        "Prompt" => Some(Signal::TurnStart),
        "Ask" => Some(Signal::Wait {
            kind: WaitKind::Permission,
            tool_use_id: Some("u1".into()),
            detail: "permission: Bash".into(),
        }),
        "Return" => Some(Signal::WaitEnd {
            tool_use_id: Some("u1".into()),
        }),
        "Interrupt" => Some(Signal::Interrupt),
        _ => None,
    }
}

/// D7.4: an interrupt is `done`, with its own cause, and it ends the wait.
#[test]
fn an_interrupt_is_done_and_ends_the_wait() {
    let mut fold = Fold::new();
    let line = |event: &str| StatusLine {
        event: event.into(),
        ts: event.into(),
        ..StatusLine::default()
    };
    let mut seen = Vec::new();
    for event in ["Prompt", "Ask", "Interrupt", "Return"] {
        let got = fold.apply(&line(event), interrupting);
        seen.push((got.map(|o| (o.state, o.cause)), fold.waiting()));
    }
    let permission = Some(WaitKind::Permission);
    assert_eq!(
        seen,
        [
            (Some((State::Working, None)), None),
            (Some((State::Waiting, None)), permission),
            (Some((State::Done, Some(Cause::Interrupt))), None),
            (Some((State::Working, None)), None),
        ]
    );
}
