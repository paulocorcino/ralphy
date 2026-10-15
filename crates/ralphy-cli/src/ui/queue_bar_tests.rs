use super::*;
use crate::runstate::RunEvent;

/// A width under the bare ` N/M` counter: the bar gives up every glyph and the
/// counter comes back whole, wider than the terminal, never empty.
#[test]
fn queue_bar_label_keeps_the_bare_counter_when_even_it_does_not_fit() {
    let mut s = RunState::new("t", 7);
    s.apply(RunEvent::QueueBuilt {
        count: 7,
        order: vec![217, 219, 220, 221, 222, 223, 224],
        stop_before: None,
        issues: serde_json::Value::Null,
        assignee_filter: None,
        scope: None,
    });
    let opts = RenderOpts {
        color: false,
        emoji: true,
    };
    assert_eq!(queue_bar_label(&s, opts, 2), " 0/7");
}
