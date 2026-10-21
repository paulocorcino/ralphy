use super::*;

/// The queue bar with emoji on, the label `bar_label()` used to produce.
fn bar(state: &RunState) -> String {
    queue_bar_label(
        state,
        RenderOpts {
            color: false,
            emoji: true,
        },
        1000,
    )
}

/// Fold an `issue started` for `n` (the fold's supersede edge for the previous one).
fn start_issue(state: &mut RunState, n: u64) {
    state.apply(RunEvent::IssueStarted {
        number: n,
        title: String::new(),
    });
}

#[test]
fn golden_render_queue_bar_labels_over_a_fixed_event_sequence() {
    // The console's bar labels, pinned event by event over a fixed sequence, so the
    // derived bar cannot drift from the reducer it replaced. The ONE intended
    // divergence is the dry-run fix: #1 advances at the supersede, not at N/N.
    let mut s = RunState::new("t", 3);
    let mut seen = Vec::new();
    s.apply(RunEvent::QueueBuilt {
        count: 3,
        order: vec![1, 2, 3],
        stop_before: Some(3),
        issues: serde_json::Value::Null,
        assignee_filter: None,
        scope: None,
    });
    seen.push(bar(&s));
    start_issue(&mut s, 1);
    seen.push(bar(&s));
    s.apply(RunEvent::PlanWritten {
        number: 1,
        open_steps: 3,
        usage: UsageLite::default(),
        steps: vec![],
    });
    seen.push(bar(&s));
    start_issue(&mut s, 2);
    seen.push(bar(&s));
    s.apply(RunEvent::IssueClosed {
        number: 2,
        tokens: 0,
        invocations: 0,
        usage: UsageLite::default(),
    });
    seen.push(bar(&s));
    assert_eq!(
        seen,
        vec![
            "▱▱▱ 0/3 (pending #1 #2 ⛔ stop-before #3)",
            "▱▱▱ 0/3 (pending #1 #2 ⛔ stop-before #3)",
            "▱▱▱ 0/3 (pending #1 #2 ⛔ stop-before #3)",
            "▰▱▱ 1/3 (pending #2 ⛔ stop-before #3)",
            "▰▰▱ 2/3 (pending ⛔ stop-before #3)",
        ]
    );
    s.finished = true;
    assert_eq!(bar(&s), "▰▰▰ 3/3");
}

#[test]
fn golden_render_active_line_is_derived_from_the_fold() {
    // The active line, rebuilt entirely from the folded entry: the same string the
    // presenter's own `ActiveIssue` produced (pinned by `render_active_line_
    // executing_shows_icon_number_title_model_and_budget`).
    let mut s = RunState::new("t", 1);
    s.apply(RunEvent::IssueStarted {
        number: 7,
        title: "t".into(),
    });
    s.apply(RunEvent::Executing {
        number: 0,
        budget_min: 45,
        model: "claude-opus-4".into(),
        effort: None,
    });
    let entry = s.active_issue().expect("an active issue");
    let line = render_active_line(
        active_phase(&entry.status).expect("a non-terminal phase"),
        entry.number,
        &entry.title,
        entry.model.as_deref(),
        entry.effort.as_deref(),
        Duration::from_secs(63),
        entry.budget_min,
        RenderOpts {
            color: false,
            emoji: true,
        },
        1000,
    );
    assert_eq!(line, "⚙️ #7 t · claude-opus-4 · 1:03 / 45:00");
    // Every terminal status ends the active line.
    s.apply(RunEvent::IssueClosed {
        number: 7,
        tokens: 0,
        invocations: 0,
        usage: UsageLite::default(),
    });
    assert_eq!(active_phase(&s.issues[0].status), None);
}

#[test]
fn queue_bar_label_advances_through_all_terminal_outcomes_to_n_over_n() {
    // A queue of five issues, each leaving via a distinct terminal transition:
    // done, non-green, blocked, stop-before, and a superseded dry-run plan.
    let mut s = RunState::new("t", 5);
    s.apply(RunEvent::QueueBuilt {
        count: 5,
        order: vec![10, 11, 12, 13, 14],
        stop_before: None,
        issues: serde_json::Value::Null,
        assignee_filter: None,
        scope: None,
    });
    assert_eq!(bar(&s), "▱▱▱▱▱ 0/5 (pending #10 #11 #12 #13 #14)");

    // done
    start_issue(&mut s, 10);
    s.apply(RunEvent::IssueClosed {
        number: 10,
        tokens: 0,
        invocations: 0,
        usage: UsageLite::default(),
    });
    // non-green (stopping run)
    start_issue(&mut s, 11);
    s.apply(RunEvent::NonGreen {
        number: 11,
        outcome: "Stuck".into(),
    });
    // blocked-by skip
    s.apply(RunEvent::Skipped {
        number: 12,
        kind: SkipKind::BlockedBy,
        label: None,
        blockers: vec![],
    });
    // stop-before skip
    s.apply(RunEvent::Skipped {
        number: 13,
        kind: SkipKind::StopBefore,
        label: None,
        blockers: vec![],
    });
    start_issue(&mut s, 14);
    assert_eq!(bar(&s), "▰▰▰▰▱ 4/5 (pending #14)");

    // #14 is a dry-run plan: no terminal event, terminal only once a following
    // `issue started` supersedes it in the fold.
    start_issue(&mut s, 99);
    assert_eq!(bar(&s), "▰▰▰▰▰ 5/5");

    // The end-of-run flush is a safe no-op when the bar is already complete.
    s.finished = true;
    assert_eq!(bar(&s), "▰▰▰▰▰ 5/5");
}

#[test]
fn queue_bar_label_finish_flushes_trailing_issue_to_n_over_n() {
    // A trailing dry-run issue with no following `issue started`: only the
    // end-of-run flush (`finished`) takes the bar to N/N.
    let mut s = RunState::new("t", 3);
    s.apply(RunEvent::QueueBuilt {
        count: 3,
        order: vec![1, 2, 3],
        stop_before: None,
        issues: serde_json::Value::Null,
        assignee_filter: None,
        scope: None,
    });
    for n in [1, 2] {
        start_issue(&mut s, n);
        s.apply(RunEvent::IssueClosed {
            number: n,
            tokens: 0,
            invocations: 0,
            usage: UsageLite::default(),
        });
    }
    assert_eq!(bar(&s), "▰▰▱ 2/3 (pending #3)");
    s.finished = true;
    assert_eq!(bar(&s), "▰▰▰ 3/3");
}

#[test]
fn queue_bar_label_marks_the_stop_before_cut_in_the_pending_list() {
    // The bioledger order: the run works #21..#10, then halts before #15.
    let mut s = RunState::new("t", 13);
    s.apply(RunEvent::QueueBuilt {
        count: 13,
        order: vec![21, 20, 14, 7, 8, 9, 10, 15, 16, 17, 18, 19, 5],
        stop_before: Some(15),
        issues: serde_json::Value::Null,
        assignee_filter: None,
        scope: None,
    });
    let emoji = RenderOpts {
        color: false,
        emoji: true,
    };
    assert_eq!(
        queue_bar_label(&s, emoji, 1000),
        "▱▱▱▱▱▱▱▱▱▱▱▱▱ 0/13 \
             (pending #21 #20 #14 #7 #8 #9 #10 ⛔ stop-before #15 #16 #17 #18 #19 #5)"
    );
    // ASCII fallback for a no-emoji terminal: same cut, glyph-free marker.
    let ascii = RenderOpts {
        color: false,
        emoji: false,
    };
    assert!(
        queue_bar_label(&s, ascii, 1000).contains("#10 |stop-before #15| #16"),
        "ascii marker brackets the boundary issue: {}",
        queue_bar_label(&s, ascii, 1000)
    );
    // No `stop_before` -> no marker, unchanged rendering.
    let mut plain = RunState::new("t", 2);
    plain.apply(RunEvent::QueueBuilt {
        count: 2,
        order: vec![1, 2],
        stop_before: None,
        issues: serde_json::Value::Null,
        assignee_filter: None,
        scope: None,
    });
    assert_eq!(bar(&plain), "▱▱ 0/2 (pending #1 #2)");
}

#[test]
fn render_active_line_executing_shows_icon_number_title_model_and_budget() {
    let opts = RenderOpts {
        color: false,
        emoji: true,
    };
    let line = render_active_line(
        Phase::Executing,
        31,
        "Console UI",
        Some("sonnet"),
        Some("medium"),
        Duration::from_secs(12 * 60 + 43),
        Some(45),
        opts,
        1000,
    );
    assert!(line.contains('⚙'), "executing phase icon: {line}");
    assert!(line.contains("#31"), "issue number: {line}");
    assert!(line.contains("Console UI"), "title: {line}");
    assert!(line.contains("sonnet / medium"), "model / effort: {line}");
    assert!(line.contains("12:43 / 45:00"), "elapsed / budget: {line}");
    assert!(!line.contains('\u{1b}'), "no ANSI byte: {line:?}");
}

#[test]
fn render_active_line_executing_zero_budget_shows_elapsed_only() {
    // A disabled per-issue cap (`0` = unbounded, explicit opt-out) renders just the
    // elapsed clock — never a misleading `/ 0:00` ceiling.
    let opts = RenderOpts {
        color: false,
        emoji: true,
    };
    // No model/effort segment (its own ` / ` separator would mask the clock's).
    let line = render_active_line(
        Phase::Executing,
        31,
        "Console UI",
        None,
        None,
        Duration::from_secs(12 * 60 + 43),
        Some(0),
        opts,
        1000,
    );
    assert!(line.contains("12:43"), "elapsed clock: {line}");
    assert!(
        !line.contains('/'),
        "no budget slash when the cap is disabled: {line}"
    );
    assert!(!line.contains("0:00"), "no zero ceiling: {line}");
}

#[test]
fn render_active_line_planning_shows_brain_icon_and_no_budget() {
    let opts = RenderOpts {
        color: false,
        emoji: true,
    };
    let line = render_active_line(
        Phase::Planning,
        31,
        "Console UI",
        None,
        None,
        Duration::from_secs(12),
        None,
        opts,
        1000,
    );
    assert!(line.contains('🧠'), "planning phase icon: {line}");
    assert!(line.contains("0:12"), "elapsed clock: {line}");
    assert!(
        !line.contains('/'),
        "no budget slash while planning: {line}"
    );
    assert!(!line.contains('\u{1b}'), "no ANSI byte: {line:?}");
}

#[test]
fn render_active_line_no_colour_emits_no_ansi() {
    let opts = RenderOpts {
        color: false,
        emoji: false,
    };
    let line = render_active_line(
        Phase::Executing,
        31,
        "title",
        Some("opus"),
        Some("high"),
        Duration::from_secs(63),
        Some(45),
        opts,
        1000,
    );
    assert!(line.contains("[exec]"), "ascii phase fallback: {line}");
    assert!(!line.contains('\u{1b}'), "no ANSI byte: {line:?}");
    // Width independence (Step 10, #226): the plain path must stay ANSI-free at
    // any width, including one so small the title is cut to nothing.
    let narrow = render_active_line(
        Phase::Executing,
        31,
        "title",
        Some("opus"),
        Some("high"),
        Duration::from_secs(63),
        Some(45),
        opts,
        10,
    );
    assert!(
        !narrow.contains('\u{1b}'),
        "no ANSI byte at width 10: {narrow:?}"
    );
}

/// A pending-heavy queue at a width that holds the bar and counter but not the
/// whole pending list: the list is cut, and the `N/M` counter survives (#226).
#[test]
fn queue_bar_label_fits_the_terminal_width() {
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
    // The whole label is 56 columns (`▰`/`▱` measure width_cjk=1 on this
    // unicode-width table), so 30 columns cuts the pending list; `…` is 2.
    let label = queue_bar_label(&s, opts, 30);
    assert_eq!(label, "▱▱▱▱▱▱▱ 0/7 (pending #217 #2…");
    assert_eq!(fit::display_width(&label), 30, "fills the given width");
}

/// A 300-char title at width 60: the title is cut, but the tail (model/effort +
/// clock) is never touched (#226).
#[test]
fn render_active_line_truncates_the_title_and_keeps_the_clock() {
    let opts = RenderOpts {
        color: false,
        emoji: true,
    };
    let title = "x".repeat(300);
    let line = render_active_line(
        Phase::Executing,
        31,
        &title,
        Some("claude-opus-4"),
        None,
        Duration::from_secs(65),
        Some(45),
        opts,
        60,
    );
    assert!(
        fit::display_width(&line) <= 60,
        "fits the given width: {line:?}"
    );
    assert!(line.contains('…'), "the title is truncated: {line}");
    assert!(
        line.ends_with("1:05 / 45:00"),
        "the clock tail survives whole: {line}"
    );
}

/// The degenerate case: a ten-column terminal must not panic and must still
/// carry the `N/M` counter (#226).
#[test]
fn queue_bar_label_survives_a_ten_column_terminal() {
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
    let label = queue_bar_label(&s, opts, 10);
    assert!(!label.is_empty(), "never empty, even at width 10");
    assert!(label.contains("0/7"), "counter survives: {label}");
    assert!(
        fit::display_width(&label) <= 10,
        "fits the given width: {label:?}"
    );
}

/// The degenerate case for the active line: a ten-column terminal must not
/// panic. The title gives up every column; the tail is never cut, so the line
/// is wider than the terminal (#226).
#[test]
fn render_active_line_survives_a_ten_column_terminal() {
    let opts = RenderOpts {
        color: false,
        emoji: true,
    };
    let title = "x".repeat(300);
    let line = render_active_line(
        Phase::Executing,
        31,
        &title,
        Some("claude-opus-4"),
        None,
        Duration::from_secs(65),
        Some(45),
        opts,
        10,
    );
    assert_eq!(line, "⚙\u{fe0f} #31  · claude-opus-4 · 1:05 / 45:00");
}

#[test]
fn sleep_label_replaces_queue_context_with_limit_message() {
    let opts = RenderOpts {
        color: false,
        emoji: true,
    };
    let label = sleep_label("08:10", opts);
    assert_eq!(label, "🌙 usage limit — sleeping until 08:10");
    assert!(
        !label.contains("pending"),
        "sleep hides pending list: {label}"
    );
    assert!(!label.contains('\u{1b}'), "no ANSI byte: {label:?}");
}

#[test]
fn fmt_clock_formats_mm_ss() {
    assert_eq!(fmt_clock(Duration::from_secs(12 * 60 + 43)), "12:43");
    assert_eq!(fmt_clock(Duration::from_secs(45 * 60)), "45:00");
    assert_eq!(fmt_clock(Duration::from_secs(5)), "0:05");
    assert_eq!(fmt_clock(Duration::from_secs(72 * 60 + 5)), "72:05");
}
