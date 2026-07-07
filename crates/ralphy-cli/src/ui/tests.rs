/// Render a [`RunEvent`] to a plain, ANSI-free line (local timestamp + outcome
/// glyph + body). The non-TTY / `NO_COLOR` clean-line path; also the public seam
/// the unit tests assert against.
#[cfg(test)]
fn render_plain_line(
    event: &RunEvent,
    ts: &DateTime<Local>,
    duration: Option<Duration>,
) -> Option<String> {
    render_line(
        event,
        ts,
        &LineExtra {
            duration,
            ..Default::default()
        },
        RenderOpts {
            color: false,
            emoji: true,
        },
    )
}

use super::*;
use crate::runstate::{RunEvent, SkipKind};
use chrono::TimeZone;
use tracing::Level;

#[test]
fn render_plain_finished_carries_timestamp_glyph_and_no_ansi() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
        .single()
        .unwrap();
    let event = RunEvent::IssueClosed {
        number: 30,
        tokens: 0,
        usage: UsageLite::default(),
    };
    let line = render_plain_line(&event, &ts, Some(Duration::from_secs(133))).expect("a line");

    assert!(
        line.contains("2026-06-10 14:03:21"),
        "carries the local timestamp: {line}"
    );
    assert!(line.contains('✅'), "carries the outcome glyph: {line}");
    assert!(line.contains("#30"), "carries the issue number: {line}");
    assert!(line.contains("2m13s"), "carries the duration: {line}");
    assert!(
        !line.contains('\u{1b}'),
        "plain line has no ANSI escape byte: {line:?}"
    );
}

#[test]
fn render_done_line_shows_model_effort_duration_and_compact_meter() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
        .single()
        .unwrap();
    // The meter/model/effort/duration ride on `LineExtra` (the presenter computes
    // them in `drive`); the event itself only names the issue.
    let extra = LineExtra {
        duration: Some(Duration::from_secs(776)),
        model: Some("sonnet".into()),
        effort: Some("medium".into()),
        meter: Some(Meter {
            usage: UsageLite {
                input: 41_200,
                cache_read: 902_000,
                cache_creation: 22_000,
                output: 18_400,
                model: None,
            },
            usd: Some(6.10),
            partial: false,
        }),
    };
    let line = render_line(
        &RunEvent::IssueClosed {
            number: 45,
            tokens: 0,
            usage: UsageLite::default(),
        },
        &ts,
        &extra,
        RenderOpts {
            color: false,
            emoji: true,
        },
    )
    .expect("a line");
    assert!(line.contains("#45 done"), "issue + outcome: {line}");
    assert!(line.contains("sonnet / medium"), "model / effort: {line}");
    assert!(line.contains("(12m56s)"), "issue duration: {line}");
    // Compact emoji breakdown + USD, scaled and joined by ` · `.
    assert!(line.contains("↑41.2k"), "input glyph + tokens: {line}");
    assert!(line.contains("⚡902.0k"), "cache-read: {line}");
    assert!(line.contains("❄22.0k"), "cache-write: {line}");
    assert!(line.contains("↓18.4k"), "output: {line}");
    assert!(line.contains("$6.10"), "read-time USD: {line}");
    assert!(!line.contains('\u{1b}'), "no ANSI byte: {line:?}");
}

#[test]
fn render_done_line_omits_meter_when_zero() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
        .single()
        .unwrap();
    let event = RunEvent::IssueClosed {
        number: 9,
        tokens: 0,
        usage: UsageLite::default(),
    };
    let line = render_plain_line(&event, &ts, None).expect("a line");
    assert!(line.contains("#9 done"), "issue + outcome: {line}");
    assert!(!line.contains('↑'), "no meter when usage is zero: {line}");
    assert!(!line.contains('$'), "no cost when usage is zero: {line}");
}

#[test]
fn render_plain_executing_is_none() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
        .single()
        .unwrap();
    assert_eq!(
        render_plain_line(
            &RunEvent::Executing {
                number: 0,
                model: String::new(),
                budget_min: 0,
                effort: None,
            },
            &ts,
            None
        ),
        None
    );
}

#[test]
fn render_plain_notice_shows_warn_and_error_glyphs() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
        .single()
        .unwrap();
    let warn_line = render_plain_line(
        &RunEvent::Notice {
            level: Level::WARN,
            message: "could not return to 'main'".to_string(),
        },
        &ts,
        None,
    )
    .expect("warn renders a line");
    assert!(warn_line.contains('⚠'), "warn glyph: {warn_line}");
    assert!(
        warn_line.contains("could not return to 'main'"),
        "warn message: {warn_line}"
    );

    let error_line = render_plain_line(
        &RunEvent::Notice {
            level: Level::ERROR,
            message: "boom".to_string(),
        },
        &ts,
        None,
    )
    .expect("error renders a line");
    assert!(error_line.contains('💥'), "error glyph: {error_line}");
    assert!(error_line.contains("boom"), "error message: {error_line}");
}

#[test]
fn render_plain_sleep_started_ended_deadline_return_some_and_executing_none() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
        .single()
        .unwrap();

    let sleep_start = render_plain_line(
        &RunEvent::SleepStarted {
            reset: "15:30".to_string(),
            target_epoch: 1_000_000,
        },
        &ts,
        None,
    )
    .expect("SleepStarted renders a line");
    assert!(
        sleep_start.contains("usage limit"),
        "SleepStarted body: {sleep_start}"
    );
    assert!(
        sleep_start.contains("15:30"),
        "SleepStarted reset time: {sleep_start}"
    );

    let sleep_end =
        render_plain_line(&RunEvent::SleepEnded, &ts, None).expect("SleepEnded renders a line");
    assert!(
        sleep_end.contains("resuming"),
        "SleepEnded body: {sleep_end}"
    );

    let deadline = render_plain_line(&RunEvent::DeadlinePassed { number: 42 }, &ts, None)
        .expect("DeadlinePassed renders a line");
    assert!(
        deadline.contains("deadline"),
        "DeadlinePassed body: {deadline}"
    );
    assert!(
        deadline.contains("#42"),
        "DeadlinePassed number: {deadline}"
    );

    // Executing is live-region only — no permanent line.
    assert_eq!(
        render_plain_line(
            &RunEvent::Executing {
                number: 0,
                model: String::new(),
                budget_min: 0,
                effort: None,
            },
            &ts,
            None,
        ),
        None
    );
}

#[test]
fn styled_sleep_started_is_live_region_only() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
        .single()
        .unwrap();
    let event = RunEvent::SleepStarted {
        reset: "08:10".to_string(),
        target_epoch: 1_000_000,
    };
    let styled = RenderOpts {
        color: true,
        emoji: true,
    };
    assert_eq!(
        render_line(&event, &ts, &LineExtra::default(), styled),
        None
    );

    let plain = RenderOpts {
        color: false,
        emoji: true,
    };
    assert!(render_line(&event, &ts, &LineExtra::default(), plain)
        .expect("plain sleep line")
        .contains("sleeping until 08:10"));
}

#[test]
fn render_plain_knowledge_consolidation_carries_glyph_and_counts() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 14, 2, 16, 0)
        .single()
        .unwrap();
    let started = render_plain_line(&RunEvent::KnowledgeConsolidating { notes: 4 }, &ts, None)
        .expect("KnowledgeConsolidating renders a line");
    assert!(started.contains('📚'), "knowledge glyph: {started}");
    assert!(started.contains('4'), "note count: {started}");
    assert!(started.contains("KNOWLEDGE.md"), "target file: {started}");

    let done = render_plain_line(&RunEvent::KnowledgeConsolidated { archived: 4 }, &ts, None)
        .expect("KnowledgeConsolidated renders a line");
    assert!(done.contains('📚'), "knowledge glyph: {done}");
    assert!(
        done.contains("4 note(s) archived"),
        "archived count: {done}"
    );
    assert!(!done.contains('\u{1b}'), "no ANSI byte: {done:?}");
}

#[test]
fn render_plain_needs_split_names_the_bundle_and_next_step() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
        .single()
        .unwrap();
    let line = render_plain_line(&RunEvent::NeedsSplit { number: 3 }, &ts, None)
        .expect("NeedsSplit renders a line");
    assert!(line.contains("#3 bundle — needs split"), "{line}");
    assert!(line.contains("/to-issues"), "{line}");
}

#[test]
fn render_plain_skipped_shows_skip_label() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
        .single()
        .unwrap();
    // A dependency skip with no resolved blockers keeps the bare fallback.
    let blocked = render_plain_line(
        &RunEvent::Skipped {
            number: 7,
            kind: SkipKind::BlockedBy,
            label: None,
            blockers: vec![],
        },
        &ts,
        None,
    )
    .expect("Skipped renders a line");
    assert!(blocked.contains("skipped (blocked)"), "{blocked}");

    // A dependency skip that knows its blocker names it (`blocked by #139`).
    let blocked_by = render_plain_line(
        &RunEvent::Skipped {
            number: 140,
            kind: SkipKind::BlockedBy,
            label: None,
            blockers: vec![139],
        },
        &ts,
        None,
    )
    .expect("Skipped renders a line");
    assert!(
        blocked_by.contains("skipped (blocked by #139)"),
        "{blocked_by}"
    );

    let stop_before = render_plain_line(
        &RunEvent::Skipped {
            number: 8,
            kind: SkipKind::StopBefore,
            label: None,
            blockers: vec![],
        },
        &ts,
        None,
    )
    .expect("StopBefore renders a line");
    assert!(
        stop_before.contains("skipped (stop-before)"),
        "{stop_before}"
    );

    let human_return = render_plain_line(
        &RunEvent::Skipped {
            number: 9,
            kind: SkipKind::HumanReturn,
            label: Some("needs-info".to_string()),
            blockers: vec![],
        },
        &ts,
        None,
    )
    .expect("HumanReturn renders a line");
    assert!(
        human_return.contains("skipped (needs-info)"),
        "{human_return}"
    );
}

#[test]
fn render_plain_human_blocked_names_the_blocker() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
        .single()
        .unwrap();
    let hitl = render_plain_line(
        &RunEvent::HumanBlocked {
            number: 16,
            on: vec![30],
        },
        &ts,
        None,
    )
    .expect("HumanBlocked renders a line");
    // Names the issue the operator must clear, not a bare phrase.
    assert!(hitl.contains("#16 waiting on human at #30"), "{hitl}");
    // The human-gate glyph (🙋), not the generic skip glyph, and no "skipped"
    // wording — it asks for a person, not a queue retry.
    assert!(hitl.contains("🙋"), "{hitl}");
    assert!(!hitl.contains("skipped"), "{hitl}");

    // With no resolved blocker, it degrades to the bare phrase.
    let bare = render_plain_line(
        &RunEvent::HumanBlocked {
            number: 7,
            on: vec![],
        },
        &ts,
        None,
    )
    .expect("HumanBlocked renders a line");
    assert!(bare.contains("#7 waiting on human"), "{bare}");
    assert!(!bare.contains(" at "), "{bare}");
}

#[test]
fn totals_panel_shows_waiting_on_human_only_when_nonzero() {
    let opts = RenderOpts {
        color: false,
        emoji: false,
    };
    // Zero → the counts line stays the three-part triad, no hitl line.
    let none = render_totals_panel(&panel_base(), opts);
    assert!(
        !none.iter().any(|l| l.contains("waiting on human")),
        "{none:?}"
    );

    // Non-zero → a dedicated waiting-on-human line appears.
    let data = PanelData {
        hitl: 2,
        ..panel_base()
    };
    let lines = render_totals_panel(&data, opts);
    assert!(
        lines.iter().any(|l| l.contains("2 waiting on human")),
        "{lines:?}"
    );
}

#[test]
fn totals_panel_undo_line_is_mode_aware() {
    let opts = RenderOpts {
        color: false,
        emoji: false,
    };

    // New + clean run (repo back on orig): undo drops the run branch.
    let new_clean = render_totals_panel(&panel_base(), opts);
    assert!(
        new_clean.iter().any(|l| l.contains(
            "undo (pre-run tag 'ralphy/pre-run-20260610-120000'): \
                 git branch -D afk/run-20260610-120000"
        )),
        "{new_clean:?}"
    );

    // New + stopped (repo parked on the run branch): checkout orig first.
    let stopped = render_totals_panel(
        &PanelData {
            stop: Some(PanelStop::Deadline),
            ..panel_base()
        },
        opts,
    );
    assert!(
        stopped
            .iter()
            .any(|l| l.contains("git checkout main && git branch -D afk/run-20260610-120000")),
        "{stopped:?}"
    );

    // Current: undo rewinds the live branch to the marker.
    let current = render_totals_panel(
        &PanelData {
            branch_mode: PanelBranchMode::Current,
            branch: "main".to_string(),
            ..panel_base()
        },
        opts,
    );
    assert!(
        current
            .iter()
            .any(|l| l.contains("git reset --hard ralphy/pre-run-20260610-120000")),
        "{current:?}"
    );
}

#[test]
fn totals_panel_undo_line_absent_without_tag_or_commits() {
    let opts = RenderOpts {
        color: false,
        emoji: false,
    };
    // No tag (creation failed or the runner dropped it) → no undo line.
    let no_tag = render_totals_panel(
        &PanelData {
            undo_tag: None,
            ..panel_base()
        },
        opts,
    );
    assert!(!no_tag.iter().any(|l| l.contains("undo")), "{no_tag:?}");

    // Zero commits → nothing to undo, even if a tag value leaked through.
    let no_commits = render_totals_panel(
        &PanelData {
            commits: 0,
            ..panel_base()
        },
        opts,
    );
    assert!(
        !no_commits.iter().any(|l| l.contains("undo")),
        "{no_commits:?}"
    );
}

#[test]
fn normalize_remote_url_handles_ssh_https_and_dot_git() {
    // SCP-style SSH → https, `.git` stripped.
    assert_eq!(
        normalize_remote_url("git@github.com:paulocorcino/ocs-inventory-go-server.git"),
        "https://github.com/paulocorcino/ocs-inventory-go-server"
    );
    // ssh:// URL form.
    assert_eq!(
        normalize_remote_url("ssh://git@github.com/owner/repo.git"),
        "https://github.com/owner/repo"
    );
    // Already https, only `.git` removed.
    assert_eq!(
        normalize_remote_url("https://github.com/owner/repo.git"),
        "https://github.com/owner/repo"
    );
    // https without `.git` is left intact.
    assert_eq!(
        normalize_remote_url("https://github.com/owner/repo"),
        "https://github.com/owner/repo"
    );
}

#[test]
fn render_info_line_emoji_plain_and_omits_missing_segments() {
    let emoji = RenderOpts {
        color: false,
        emoji: true,
    };
    let full = render_info_line(
        "ocs-inventory",
        Some("main"),
        Some("https://github.com/owner/repo"),
        emoji,
    );
    assert_eq!(
        full,
        "📦 ocs-inventory · 🌿 main · 🔗 https://github.com/owner/repo"
    );

    // No URL (local-only repo): the 🔗 segment is omitted entirely.
    let no_url = render_info_line("proj", Some("dev"), None, emoji);
    assert_eq!(no_url, "📦 proj · 🌿 dev");

    // Plain path: no emoji, no ANSI byte.
    let plain = render_info_line(
        "proj",
        Some("dev"),
        Some("https://x/y"),
        RenderOpts {
            color: false,
            emoji: false,
        },
    );
    assert_eq!(plain, "proj · dev · https://x/y");
    assert!(!plain.contains('\u{1b}'), "no ANSI byte: {plain:?}");
}

#[test]
fn fmt_duration_formats_minutes_and_seconds() {
    assert_eq!(fmt_duration(Duration::from_secs(13)), "13s");
    assert_eq!(fmt_duration(Duration::from_secs(133)), "2m13s");
    assert_eq!(fmt_duration(Duration::from_secs(120)), "2m00s");
}

#[test]
fn queue_state_advances_through_all_terminal_outcomes_to_n_over_n() {
    // A queue of five issues, each leaving via a distinct terminal transition:
    // done, non-green, blocked, stop-before, and a superseded infeasible plan.
    let mut q = QueueState::built(5, vec![10, 11, 12, 13, 14], None);
    assert_eq!(q.bar_label(), "▱▱▱▱▱ 0/5 (pending #10 #11 #12 #13 #14)");

    // done
    q.advance(10);
    // non-green (stopping run)
    q.advance(11);
    // blocked-by skip
    q.advance(12);
    // stop-before skip
    q.advance(13);
    assert_eq!(q.bar_label(), "▰▰▰▰▱ 4/5 (pending #14)");

    // #14 is an infeasible/dry-run plan: no terminal event, completed only when
    // a following `issue started` supersedes it.
    q.supersede(14);
    assert_eq!(q.completed, 5);
    assert_eq!(q.bar_label(), "▰▰▰▰▰ 5/5");

    // Idempotent: a stray repeat never over-counts past N/N.
    q.advance(14);
    assert_eq!(q.completed, 5);

    // `finish` is a safe flush even when already complete.
    q.finish();
    assert_eq!(q.bar_label(), "▰▰▰▰▰ 5/5");
}

#[test]
fn queue_state_finish_flushes_trailing_issue_to_n_over_n() {
    // A trailing infeasible issue with no following `issue started`: only the
    // end-of-run `finish` flushes the bar to N/N.
    let mut q = QueueState::built(3, vec![1, 2, 3], None);
    q.advance(1);
    q.advance(2);
    assert_eq!(q.bar_label(), "▰▰▱ 2/3 (pending #3)");
    q.finish();
    assert_eq!(q.bar_label(), "▰▰▰ 3/3");
}

#[test]
fn queue_state_marks_the_stop_before_cut_in_the_pending_list() {
    // The bioledger order: the run works #21..#10, then halts before #15.
    let q = QueueState::built(
        13,
        vec![21, 20, 14, 7, 8, 9, 10, 15, 16, 17, 18, 19, 5],
        Some(15),
    );
    let emoji = RenderOpts {
        color: false,
        emoji: true,
    };
    assert_eq!(
        q.bar_label_opts(emoji),
        "▱▱▱▱▱▱▱▱▱▱▱▱▱ 0/13 \
             (pending #21 #20 #14 #7 #8 #9 #10 ⛔ stop-before #15 #16 #17 #18 #19 #5)"
    );
    // ASCII fallback for a no-emoji terminal: same cut, glyph-free marker.
    let ascii = RenderOpts {
        color: false,
        emoji: false,
    };
    assert!(
        q.bar_label_opts(ascii)
            .contains("#10 |stop-before #15| #16"),
        "ascii marker brackets the boundary issue: {}",
        q.bar_label_opts(ascii)
    );
    // No `stop_before` → no marker, unchanged rendering.
    let plain = QueueState::built(2, vec![1, 2], None);
    assert_eq!(plain.bar_label(), "▱▱ 0/2 (pending #1 #2)");
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
    // A disabled per-issue cap (`0` = unbounded, the default) renders just the
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
    );
    assert!(line.contains("[exec]"), "ascii phase fallback: {line}");
    assert!(!line.contains('\u{1b}'), "no ANSI byte: {line:?}");
}

#[test]
fn bar_label_no_colour_emits_no_ansi() {
    let mut q = QueueState::built(6, vec![1, 2, 3, 4, 5, 6], None);
    q.advance(1);
    q.advance(2);
    q.advance(3);
    let label = q.bar_label();
    assert_eq!(label, "▰▰▰▱▱▱ 3/6 (pending #4 #5 #6)");
    assert!(!label.contains('\u{1b}'), "no ANSI byte: {label:?}");
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

fn panel_base() -> PanelData {
    PanelData {
        branch: "afk/run-20260610-120000".to_string(),
        orig_branch: "main".to_string(),
        done: 3,
        blocked: 1,
        skipped: 2,
        hitl: 0,
        commits: 5,
        stop: None,
        branch_mode: PanelBranchMode::New,
        dry_run: false,
        undo_tag: Some("ralphy/pre-run-20260610-120000".to_string()),
        run_breakdown: UsageLite {
            input: 8_400_000,
            ..Default::default()
        },
        project_breakdown: UsageLite {
            input: 142_000_000,
            ..Default::default()
        },
        project_id: "owner/repo".to_string(),
        run_usd: Some(2.10),
        project_usd: Some(35.6),
        run_usd_partial: false,
        project_usd_partial: false,
    }
}

#[test]
fn fmt_tokens_scales_millions_thousands_and_bare() {
    assert_eq!(fmt_tokens(1_200_000), "1.2M");
    assert_eq!(fmt_tokens(8_400), "8.4k");
    assert_eq!(fmt_tokens(912), "912");
    assert_eq!(fmt_tokens(0), "0");
}

#[test]
fn render_totals_panel_footer_shows_run_and_project_tokens() {
    let opts = RenderOpts {
        color: false,
        emoji: true,
    };
    let lines = render_totals_panel(&panel_base(), opts);
    let footer = lines
        .iter()
        .find(|l| l.contains("run:") && l.contains("project:"))
        .expect("a token footer line");
    // Carries the formatted run breakdown, the project id, and the project total.
    assert!(footer.contains("↑8.4M"), "run input total: {footer}");
    assert!(footer.contains("owner/repo"), "project id: {footer}");
    assert!(footer.contains("↑142.0M"), "project input total: {footer}");
    // Read-time USD estimates (ADR-0008 D8), compact `$` form.
    assert!(footer.contains("$2.10"), "run usd: {footer}");
    assert!(footer.contains("$35.60"), "project usd: {footer}");
    assert!(!footer.contains('\u{1b}'), "no ANSI byte: {footer:?}");
}

#[test]
fn render_totals_panel_footer_shows_unknown_usd_never_zero() {
    let opts = RenderOpts {
        color: false,
        emoji: true,
    };
    // A fully-unpriced run shows `~$?`, never `~$0.00`.
    let data = PanelData {
        run_usd: None,
        project_usd: None,
        run_usd_partial: true,
        project_usd_partial: true,
        ..panel_base()
    };
    let lines = render_totals_panel(&data, opts);
    let footer = lines
        .iter()
        .find(|l| l.contains("run:") && l.contains("project:"))
        .expect("a token footer line");
    assert!(footer.contains("$?"), "unknown usd shows $?: {footer}");
    assert!(
        !footer.contains("$0.00"),
        "never reports $0 for unknown spend: {footer}"
    );
}

#[test]
fn render_totals_panel_run_and_project_partial_are_independent() {
    let opts = RenderOpts {
        color: false,
        emoji: true,
    };
    // A fully-priced run (no unpriced model) alongside a project whose
    // cumulative ledger DOES hold an unpriced model: the run figure must stay
    // clean while only the project carries `+?`. Guards the regression where a
    // single shared flag leaked the project's residue onto the run total.
    let data = PanelData {
        run_usd: Some(1.81),
        project_usd: Some(15.98),
        run_usd_partial: false,
        project_usd_partial: true,
        ..panel_base()
    };
    let lines = render_totals_panel(&data, opts);
    let footer = lines
        .iter()
        .find(|l| l.contains("run:") && l.contains("project:"))
        .expect("a token footer line");
    let (run_part, project_part) = footer.split_once("project:").expect("two segments");
    assert!(
        run_part.contains("$1.81") && !run_part.contains("+?"),
        "the fully-priced run must NOT carry +?: {run_part}"
    );
    assert!(
        project_part.contains("$15.98+?"),
        "the project's unpriced residue is flagged with +?: {project_part}"
    );
}

#[test]
fn fmt_usd_compact_partial_suffix_and_unknown() {
    assert_eq!(fmt_usd_compact(Some(2.10), false), "$2.10");
    assert_eq!(fmt_usd_compact(Some(2.10), true), "$2.10+?");
    assert_eq!(fmt_usd_compact(None, false), "$?");
    assert_eq!(fmt_usd_compact(None, true), "$?");
}

#[test]
fn render_totals_panel_counts_line_and_no_per_issue_relisting() {
    let opts = RenderOpts {
        color: false,
        emoji: true,
    };
    let lines = render_totals_panel(&panel_base(), opts);
    let all = lines.join("\n");

    // Counts line has the correct triad and numbers.
    assert!(lines[0].contains("✅ 3 done"), "done count: {}", lines[0]);
    assert!(
        lines[0].contains("⛔ 1 blocked"),
        "blocked count: {}",
        lines[0]
    );
    assert!(
        lines[0].contains("⏭️ 2 skipped"),
        "skipped count: {}",
        lines[0]
    );
    // No per-issue `#N:` re-listing — the old format was `  #N: Done`.
    assert!(!all.contains(": Done"), "no per-issue Done line: {all}");
    assert!(
        !all.contains(": Blocked"),
        "no per-issue Blocked line: {all}"
    );
    assert!(
        !all.contains(": Timeout"),
        "no per-issue Timeout line: {all}"
    );
}

#[test]
fn render_totals_panel_git_merge_line_presence_rules() {
    let opts = RenderOpts {
        color: false,
        emoji: true,
    };

    // New + commits > 0: merge line present.
    let lines = render_totals_panel(&panel_base(), opts);
    let all = lines.join("\n");
    assert!(
        all.contains("git merge afk/run-20260610-120000"),
        "merge line present for New+commits: {all}"
    );

    // New + dry_run + 0 commits: no merge line.
    let dry_zero = PanelData {
        dry_run: true,
        commits: 0,
        ..panel_base()
    };
    let all2 = render_totals_panel(&dry_zero, opts).join("\n");
    assert!(
        !all2.contains("git merge"),
        "no merge line for New+dry_run+0-commits: {all2}"
    );

    // Current mode: no merge line regardless of commits.
    let current = PanelData {
        branch_mode: PanelBranchMode::Current,
        ..panel_base()
    };
    let all3 = render_totals_panel(&current, opts).join("\n");
    assert!(
        !all3.contains("git merge"),
        "no merge line for Current mode: {all3}"
    );
}

#[test]
fn render_totals_panel_plain_no_ansi_and_stop_reason_present() {
    let opts = RenderOpts {
        color: false,
        emoji: true,
    };
    let data = PanelData {
        stop: Some(PanelStop::NonGreen {
            number: 42,
            outcome: "Blocked(\"reason\")".to_string(),
        }),
        ..panel_base()
    };
    let lines = render_totals_panel(&data, opts);
    let all = lines.join("\n");

    // No ANSI escape bytes on the plain path.
    assert!(!all.contains('\u{1b}'), "no ANSI in plain render: {all:?}");

    // Stop-reason line is present and references the issue.
    assert!(all.contains("Stopped:"), "stop-reason line present: {all}");
    assert!(all.contains("#42"), "issue number in stop line: {all}");

    // Done/blocked/skipped counts from the supplied PanelData match.
    assert!(all.contains("3 done"), "done count preserved: {all}");
    assert!(all.contains("1 blocked"), "blocked count preserved: {all}");
    assert!(all.contains("2 skipped"), "skipped count preserved: {all}");
}

/// `UsageLite` is a bare alias of `ralphy_core::Usage`, not a mirror struct: a
/// `core::Usage` binds into a `UsageLite` slot with no conversion. Fails to
/// compile (type mismatch) if the mirror struct is ever reintroduced.
#[test]
fn usage_lite_is_alias_of_core_usage() {
    let u: UsageLite = ralphy_core::Usage::default();
    assert_eq!(u.total(), 0);
}
