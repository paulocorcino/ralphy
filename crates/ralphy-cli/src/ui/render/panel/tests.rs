use super::*;

fn data() -> PanelData {
    PanelData {
        branch: "afk/run-1".into(),
        orig_branch: "main".into(),
        done: 3,
        blocked: 1,
        skipped: 2,
        hitl: 1,
        review_only: vec![4],
        commits: 5,
        stop: Some(PanelStop::Limit {
            number: 7,
            reset: Some("08:10".into()),
        }),
        branch_mode: PanelBranchMode::New,
        dry_run: false,
        undo_tag: Some("ralphy/pre-run-1".into()),
        run_breakdown: UsageLite {
            input: 8_400_000,
            ..Default::default()
        },
        project_breakdown: UsageLite {
            input: 142_000_000,
            ..Default::default()
        },
        project_id: "owner/repo".into(),
        run_usd: Some(2.10),
        project_usd: None,
        run_usd_partial: true,
        project_usd_partial: false,
        consolidate_breakdown: Some(UsageLite {
            input: 1_000,
            ..Default::default()
        }),
        consolidate_usd: Some(0.5),
    }
}

const PLAIN: RenderOpts = RenderOpts {
    color: false,
    emoji: false,
};

#[test]
fn a_stopped_new_branch_run_prints_every_line_in_order() {
    let lines = render_totals_panel(&data(), PLAIN);
    assert_eq!(
        lines,
        [
            "[ok] 3 done · [blocked] 1 blocked · [skip] 2 skipped",
            "[hitl] 1 waiting on human",
            "[review] 1 issue needs your eyes: #4 — review-only criteria, delivered but not machine-checked",
            "5 commit(s) on 'afk/run-1'",
            "Stopped: usage limit on #7. Reset ~08:10; re-run to continue (or it stalled with no progress).",
            "Left repo checked out on 'afk/run-1' for inspection.",
            "[undo]  undo (pre-run tag 'ralphy/pre-run-1'): git checkout main && git branch -D afk/run-1",
            "➜  git merge afk/run-1",
            "run: in 8.4M cr 0 cw 0 out 0 · $2.10+? · consolidate: in 1.0k cr 0 cw 0 out 0 · $0.50 · project: owner/repo in 142.0M cr 0 cw 0 out 0 · $?",
        ]
    );
}

#[test]
fn a_dry_run_in_place_prints_the_closing_line_and_no_next_step() {
    let lines = render_totals_panel(
        &PanelData {
            hitl: 0,
            review_only: vec![4, 9],
            commits: 0,
            stop: Some(PanelStop::Stopped { number: None }),
            branch_mode: PanelBranchMode::Current,
            dry_run: true,
            consolidate_breakdown: None,
            ..data()
        },
        PLAIN,
    );
    assert_eq!(
        lines,
        [
            "[ok] 3 done · [blocked] 1 blocked · [skip] 2 skipped",
            "[review] 2 issues need your eyes: #4, #9 — review-only criteria, delivered but not machine-checked",
            "0 commit(s) on 'afk/run-1'",
            "Stopped: you stopped the run between issues. Branch handed back.",
            "DryRun on 'afk/run-1': no commits made.",
            "run: in 8.4M cr 0 cw 0 out 0 · $2.10+? · project: owner/repo in 142.0M cr 0 cw 0 out 0 · $?",
        ]
    );
}

#[test]
fn emoji_icons_lead_the_counts_line() {
    let lines = render_totals_panel(
        &data(),
        RenderOpts {
            color: false,
            emoji: true,
        },
    );
    assert_eq!(lines[0], "✅ 3 done · ⛔ 1 blocked · ⏭️ 2 skipped");
}
