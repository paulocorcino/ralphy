use super::*;
use crate::runstate::{RunEvent, UsageLite};

#[test]
fn render_card_small_queue_one_line_per_issue() {
    let mut state = RunState::new("Repo · 2 issues", 2);
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "first".into(),
    });
    state.apply(RunEvent::IssueClosed {
        number: 1,
        tokens: 0,
        invocations: 0,
        usage: UsageLite::default(),
    });
    state.apply(RunEvent::IssueStarted {
        number: 2,
        title: "second".into(),
    });
    let card = render_card(&state, 0);
    assert!(card.contains("✅ #1 first"), "card: {card}");
    assert!(card.contains("🧠 #2 second"), "card: {card}");
    assert!(card.len() <= TELEGRAM_LIMIT);
}

#[test]
fn render_card_names_blocker_on_dependency_skip() {
    // A blocked-by skip carrying its open blocker(s) names them on the issue line
    // (`⏭️ #140 … (blocked by #139)`) so the operator knows which issue held it;
    // the counters are untouched.
    let mut state = RunState::new("repo · 1 issues", 1);
    state.apply(RunEvent::Skipped {
        number: 140,
        kind: crate::runstate::SkipKind::BlockedBy,
        label: None,
        blockers: vec![139],
    });
    let card = render_card(&state, 0);
    assert!(card.contains("⏭️ #140  (blocked by #139)"), "card: {card}");
    // A skip with no resolved blocker adds no suffix.
    let mut bare = RunState::new("repo · 1 issues", 1);
    bare.apply(RunEvent::Skipped {
        number: 141,
        kind: crate::runstate::SkipKind::BlockedBy,
        label: None,
        blockers: vec![],
    });
    let bare_card = render_card(&bare, 0);
    assert!(!bare_card.contains("blocked by"), "card: {bare_card}");
}

#[test]
fn render_card_and_footer_surface_needs_split() {
    let mut state = RunState::new("repo · 1 issues", 1);
    state.apply(RunEvent::IssueStarted {
        number: 3,
        title: "W1 bundle".into(),
    });
    state.apply(RunEvent::PlanWritten {
        number: 3,
        open_steps: 0,
        usage: UsageLite::default(),
        steps: vec![],
    });
    state.apply(RunEvent::NeedsSplit { number: 3 });
    let card = render_card(&state, 0);
    assert!(card.contains("🧩 #3 W1 bundle"), "issue line: {card}");
    assert!(card.contains("· 🧩 1"), "counter: {card}");
    state.finished = true;
    let footer = render_final_push(&state);
    assert!(footer.contains("🧩 1 awaiting split"), "footer: {footer}");
    // Without a bundle, neither the counter nor the footer mention it.
    let clean = RunState::new("repo · 1 issues", 1);
    assert!(!render_card(&clean, 0).contains("🧩"));
    assert!(!render_final_push(&clean).contains("🧩"));
}

#[test]
fn render_card_and_footer_surface_a_dry_run_plan_only_pass() {
    // The dry-run fix reaching the card: the superseded issue renders 📝, the
    // counter line grows a `· 📝 N` segment AFTER the 🧩 one, and the pass counts
    // as processed so the footer is the normal 🏁, never "stopped before any
    // issue was processed".
    let mut state = RunState::new("repo · 2 issues", 2);
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "one".into(),
    });
    state.apply(RunEvent::PlanWritten {
        number: 1,
        open_steps: 3,
        usage: UsageLite::default(),
        steps: vec![],
    });
    state.apply(RunEvent::IssueStarted {
        number: 2,
        title: "two".into(),
    });
    let card = render_card(&state, 0);
    assert!(card.contains("📝 #1 one"), "issue line: {card}");
    assert!(card.contains("· 📝 1"), "counter: {card}");
    assert!(
        card.contains("▶️ 2 · ✅ 0 · ⏭️ 0 · ⛔ 0 · 🤷 0 · ❌ 0 · 📝 1"),
        "the 📝 segment is appended, leaving the base counters byte-stable: {card}"
    );
    state.finished = true;
    let footer = render_final_push(&state);
    assert!(
        footer.contains("🏁"),
        "a dry run did process work: {footer}"
    );
    assert!(
        !footer.contains("stopped before any issue was processed"),
        "footer: {footer}"
    );
    // A run with no plan-only pass keeps the card byte-identical to before.
    let clean = RunState::new("repo · 2 issues", 2);
    assert!(!render_card(&clean, 0).contains("📝"));
}

#[test]
fn footer_marks_a_run_that_processed_nothing_as_stopped() {
    // A run whose card reaches its terminal edge with zero issues finished,
    // skipped, or parked was interrupted (killed/superseded/bailed at startup) —
    // the footer must say so, not `🏁 … ✅ 0 done` which reads as a clean finish
    // (FinCal, 2026-07-13: an aborted run's finished card sat above the next run's
    // start card, reading "finished → started").
    let mut state = RunState::new("repo · 12 issues", 12);
    state.finished = true;
    let footer = render_final_push(&state);
    assert!(footer.contains("🛑"), "stopped marker: {footer}");
    assert!(
        footer.contains("stopped before any issue was processed"),
        "stopped wording: {footer}"
    );
    assert!(!footer.contains("🏁"), "no finish flag: {footer}");
    assert!(
        !footer.contains("✅ 0 done"),
        "no zero-done claim: {footer}"
    );

    // One issue done flips it back to the normal `🏁` completion footer.
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "first".into(),
    });
    state.apply(RunEvent::IssueClosed {
        number: 1,
        tokens: 0,
        invocations: 0,
        usage: UsageLite::default(),
    });
    let done_footer = render_final_push(&state);
    assert!(done_footer.contains("🏁"), "finish flag: {done_footer}");
    assert!(
        done_footer.contains("✅ 1 done"),
        "done count: {done_footer}"
    );
}

#[test]
fn render_card_has_header_counters_and_blank_line_grouping() {
    let mut state = RunState::new("ocs-inventory · 2 issues [AFK]", 2);
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "first".into(),
    });
    let card = render_card(&state, 0);
    // Branding header with the binary version.
    assert!(card.contains("Ralphy - v"), "header missing: {card}");
    assert!(
        card.contains(env!("CARGO_PKG_VERSION")),
        "version missing: {card}"
    );
    // The counter line leads with `▶️ N`, the queue total (not `N issues`).
    assert!(card.contains("▶️ 2 · ✅ 0"), "counters: {card}");
    assert!(!card.contains("2 issues ·"), "old counter form: {card}");
    // Groups are separated by a blank line.
    assert!(card.contains("\n\n"), "blank-line grouping: {card}");
    // No footer mid-run — the issue list is the last group.
    assert!(!card.contains("🏁"), "footer must not show mid-run: {card}");
}

#[test]
fn render_card_shows_live_consolidation_line_then_footer_segment() {
    let mut state = RunState::new("repo · 1 issues", 1);
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "a".into(),
    });
    state.apply(RunEvent::IssueClosed {
        number: 1,
        tokens: 0,
        invocations: 0,
        usage: UsageLite::default(),
    });
    // Mid-consolidation: the live 📚 line shows, no footer yet.
    state.apply(RunEvent::KnowledgeConsolidating { notes: 4 });
    let live = render_card(&state, 0);
    assert!(
        live.contains("📚 consolidating 4 knowledge note(s)"),
        "live consolidation line: {live}"
    );
    assert!(!live.contains("🏁"), "no footer mid-run: {live}");

    // Completion + terminal: the live line is gone, the footer carries the count.
    state.apply(RunEvent::KnowledgeConsolidated { archived: 4 });
    state.finished = true;
    let card = render_card(&state, 0);
    assert!(
        !card.contains("consolidating 4"),
        "live line hidden once finished: {card}"
    );
    assert!(card.contains("📚 4 consolidated"), "footer segment: {card}");
}

#[test]
fn render_card_hides_stale_consolidating_line_on_finished_card() {
    // A failed session never emits `KnowledgeConsolidated`, so `consolidating`
    // stays set — the terminal card must still drop the stale 📚 line.
    let mut state = RunState::new("repo · 1 issues", 1);
    state.apply(RunEvent::KnowledgeConsolidating { notes: 2 });
    state.finished = true;
    let card = render_card(&state, 0);
    assert!(
        !card.contains("consolidating"),
        "no stale live line: {card}"
    );
    assert!(
        !card.contains("📚"),
        "no consolidated footer segment: {card}"
    );
}

#[test]
fn render_card_shows_footer_only_when_finished() {
    let mut state = RunState::new("repo · 1 issues", 1);
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "a".into(),
    });
    state.apply(RunEvent::IssueClosed {
        number: 1,
        tokens: 0,
        invocations: 0,
        usage: UsageLite::default(),
    });
    // During the run: no footer.
    assert!(!render_card(&state, 0).contains("🏁"), "no footer mid-run");
    // Finished: the footer appears with the done/skipped tally.
    state.finished = true;
    let card = render_card(&state, 0);
    assert!(card.contains("🏁"), "footer missing when finished: {card}");
    assert!(card.contains("run finished"), "footer head: {card}");
    assert!(card.contains("✅ 1 done"), "footer tally: {card}");
}

#[test]
fn header_face_is_stable_per_title_but_varies_across_titles() {
    // Same title → same face on every edit (so the card never re-edits just to
    // animate the face).
    assert_eq!(
        header_line(&RunState::new("ocs-inventory · 10 issues", 10)),
        header_line(&RunState::new("ocs-inventory · 10 issues", 10))
    );
    // The face is drawn from the curated pool.
    let face = crate::runstate::header_face("ocs-inventory · 10 issues");
    assert!(
        crate::runstate::HEADER_FACES.contains(&face),
        "face off-pool: {face}"
    );
}

#[test]
fn render_card_collapses_large_queue_within_limit() {
    let mut state = RunState::new("Big run", 200);
    for n in 1..=200u64 {
        state.apply(RunEvent::IssueStarted {
            number: n,
            title: format!("issue {n} with a moderately long descriptive title to pad bytes"),
        });
        if n < 200 {
            state.apply(RunEvent::IssueClosed {
                number: n,
                tokens: 0,
                invocations: 0,
                usage: UsageLite::default(),
            });
        }
    }
    let card = render_card(&state, 0);
    assert!(card.len() <= TELEGRAM_LIMIT, "len {}", card.len());
    assert!(card.contains("▶️ 200"), "card: {card}");
    // Collapsed: active issue #200 and a last-finished line are shown.
    assert!(card.contains("#200"), "card: {card}");
}

#[test]
fn render_card_shows_sleep_line_with_live_countdown() {
    use crate::runstate::SleepState;
    let mut state = RunState::new("Repo", 1);
    state.sleep = Some(SleepState {
        reset: "14:30".into(),
        // 2h13m ahead of `now`.
        target_epoch: 1_700_000_000 + 2 * 3600 + 13 * 60,
    });
    let card = render_card(&state, 1_700_000_000);
    assert!(card.contains('🌙'), "card: {card}");
    assert!(card.contains("14:30"), "card: {card}");
    assert!(card.contains("resumes in ~"), "card: {card}");
    assert!(card.contains("~2h 13m"), "card: {card}");
}

#[test]
fn render_sleep_line_clamps_to_zero_when_reset_due() {
    use crate::runstate::SleepState;
    // `now` is past the target: the countdown degrades to `~0m`, not negative.
    let sleep = SleepState {
        reset: "09:00".into(),
        target_epoch: 1_700_000_000,
    };
    let line = render_sleep_line(&sleep, 1_700_000_500);
    assert!(line.contains("~0m"), "line: {line}");
    assert!(!line.contains('-'), "line should not go negative: {line}");
}

#[test]
fn derive_title_covers_all_three_branches() {
    // --title wins.
    assert_eq!(
        derive_title("repo", 3, &["AFK".into()], None, Some("Override")),
        "Override"
    );
    // --only-issue: the single title.
    assert_eq!(
        derive_title("repo", 1, &[], Some("Only one"), None),
        "Only one"
    );
    // Auto-derived with labels.
    assert_eq!(
        derive_title("myrepo", 3, &["AFK".into(), "ready".into()], None, None),
        "myrepo · 3 issues [AFK, ready]"
    );
    // A blank --title falls through to the auto form.
    assert_eq!(
        derive_title("myrepo", 1, &[], None, Some("  ")),
        "myrepo · 1 issues"
    );
}

#[test]
fn render_card_carries_the_run_skipped_reason() {
    // The `--if-idle` deferral (#222): the run processed nothing, so the terminal
    // footer must carry the folded deferral sentence rather than the generic
    // "stopped before any issue was processed".
    let reason = "skipped: run in progress since 2026-07-19 10:00:00, pid 4242";
    let mut state = RunState::new("repo · 0 issues", 0);
    state.apply(RunEvent::RunSkipped {
        reason: reason.into(),
    });
    state.finished = true;
    let card = render_card(&state, 0);
    assert!(card.contains(reason), "card: {card}");
}

#[test]
fn render_card_no_work_triad_has_no_issue_rows() {
    // The empty-queue border (#222): the full triad folds to a 0-issue card — the
    // counters read 0 and not a single per-issue row is drawn.
    let mut state = RunState::new("repo · 0 issues", 0);
    state.apply(RunEvent::QueueBuilt {
        count: 0,
        order: vec![],
        stop_before: None,
        issues: serde_json::Value::Null,
        assignee_filter: None,
        scope: Some("labels [AFK]".into()),
    });
    state.apply(RunEvent::RunStarted {
        repo: "o/r".into(),
        queue_labels: vec!["AFK".into()],
        agent: "claude".into(),
        plan_agent: "claude".into(),
        branch_mode: "new".into(),
        branch: "origin/main".into(),
        deadline_hours: None,
    });
    state.apply(RunEvent::RunFinished {
        outcome: "no_work".into(),
        issues_done: 0,
        issues_skipped: 0,
        issues_total: 0,
        issues_blocked: 0,
        issues_hitl: 0,
        issues: serde_json::Value::Null,
        up: 0,
        cr: 0,
        cw: 0,
        out: 0,
        duration_s: 0,
    });
    state.finished = true;
    let card = render_card(&state, 0);
    assert!(state.issues.is_empty(), "no per-issue rows: {card}");
    assert!(!card.contains("✅ #"), "no per-issue rows: {card}");
    // The footer must SAY the run had nothing to do — the generic fallback
    // ("stopped before any issue was processed") reads as an unexplained abort.
    assert!(
        card.contains("no open issues to process"),
        "the no_work footer must explain itself: {card}"
    );
    assert!(
        !card.contains("stopped before any issue was processed"),
        "the generic abort footer must not appear: {card}"
    );
}
