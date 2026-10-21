use super::*;

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

fn panel_base() -> PanelData {
    PanelData {
        branch: "afk/run-20260610-120000".to_string(),
        orig_branch: "main".to_string(),
        done: 3,
        blocked: 1,
        skipped: 2,
        hitl: 0,
        review_only: Vec::new(),
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
        consolidate_breakdown: None,
        consolidate_usd: None,
    }
}

#[test]
fn totals_panel_shows_review_debt_only_when_nonzero() {
    let opts = RenderOpts {
        color: false,
        emoji: false,
    };

    let clean = render_totals_panel(&panel_base(), opts);
    assert!(
        !clean.iter().any(|l| l.contains("need your eyes")),
        "zero review debt prints nothing: {clean:?}"
    );

    let with_debt = render_totals_panel(
        &PanelData {
            review_only: vec![305, 308, 311],
            ..panel_base()
        },
        opts,
    );
    let line = with_debt
        .iter()
        .find(|l| l.contains("need your eyes"))
        .expect("a review-debt line");
    assert!(
        line.contains("3 issues need your eyes: #305, #308, #311"),
        "count + numbers: {line}"
    );
    // AC 4: review-only is attention debt on a DELIVERED issue, not unfinished work.
    assert!(
        line.contains("delivered but not machine-checked"),
        "wording: {line}"
    );
    assert!(
        !with_debt.join("\n").contains("unfinished"),
        "panel never calls review debt unfinished: {with_debt:?}"
    );

    let single = render_totals_panel(
        &PanelData {
            review_only: vec![305],
            ..panel_base()
        },
        opts,
    );
    assert!(
        single
            .iter()
            .any(|l| l.contains("1 issue needs your eyes: #305")),
        "singular form: {single:?}"
    );
}

#[test]
fn totals_panel_review_debt_never_moves_the_counts_line() {
    let opts = RenderOpts {
        color: false,
        emoji: false,
    };
    let with_debt = render_totals_panel(
        &PanelData {
            review_only: vec![305, 308, 311],
            ..panel_base()
        },
        opts,
    );
    // AC 5: done/blocked/skipped stay byte-identical, review debt or not.
    assert_eq!(with_debt[0], render_totals_panel(&panel_base(), opts)[0]);
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
fn render_totals_panel_footer_shows_consolidation_segment_when_present() {
    let opts = RenderOpts {
        color: false,
        emoji: true,
    };
    // Issue #269: a run that consolidated shows a distinct `consolidate:` segment
    // between the run total and the project balance; a run that did not omits it.
    let data = PanelData {
        consolidate_breakdown: Some(UsageLite {
            input: 33_398,
            output: 5_444,
            cache_read: 337_152,
            ..Default::default()
        }),
        consolidate_usd: Some(0.42),
        ..panel_base()
    };
    let lines = render_totals_panel(&data, opts);
    let footer = lines
        .iter()
        .find(|l| l.contains("run:") && l.contains("project:"))
        .expect("a token footer line");
    assert!(footer.contains("consolidate:"), "segment label: {footer}");
    assert!(footer.contains("↑33.4k"), "consolidation input: {footer}");
    assert!(footer.contains("$0.42"), "consolidation usd: {footer}");
    // It sits between the run total and the project balance.
    let ci = footer.find("consolidate:").unwrap();
    assert!(
        footer.find("run:").unwrap() < ci && ci < footer.find("project:").unwrap(),
        "consolidate segment must sit between run and project: {footer}"
    );

    // No consolidation this run → no segment (panel_base carries None).
    let plain = render_totals_panel(&panel_base(), opts);
    let plain_footer = plain
        .iter()
        .find(|l| l.contains("run:") && l.contains("project:"))
        .expect("a token footer line");
    assert!(
        !plain_footer.contains("consolidate:"),
        "a run that did not consolidate shows no segment: {plain_footer}"
    );
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

/// #225: a run with both phases priced on the same model must not carry the
/// `+?` partial-residue suffix — the bug was the runner dropping the exec
/// phase's model, which left it unpriced and forced `partial = true`.
#[test]
fn meter_for_prices_both_phases_without_partial_residue() {
    let pt = PriceTable::defaults();
    let plan = UsageLite {
        input: 1_000_000,
        model: Some("claude-opus-4-8".into()),
        ..Default::default()
    };
    let exec = UsageLite {
        input: 1_000_000,
        model: Some("claude-opus-4-8".into()),
        ..Default::default()
    };

    let m = meter_for(&pt, Some(&plan), &exec);

    // Each phase priced by the table itself: the rate is the table's, the sum
    // is the meter's.
    let one_phase = pt
        .cost_usd(
            "claude-opus-4-8",
            &ralphy_pricing::TokenCounts {
                input: 1_000_000,
                ..Default::default()
            },
        )
        .expect("the floor prices opus");
    assert!(
        (m.usd.unwrap() - 2.0 * one_phase).abs() < 1e-9,
        "usd: {:?}, one phase: {one_phase}",
        m.usd
    );
    assert!(!m.partial, "no phase should be unpriced");
    assert_eq!(m.usage.input, 2_000_000);
}
