use super::derived::{repo_relative_plan_path, resolve_verify_timeout_minutes};
use super::*;

/// The document carries the plan's REPO-RELATIVE path, forward-slashed —
/// the confined `file.read` verb rejects an absolute path and a
/// backslash-separated one alike, so on Windows a regression here breaks
/// the panel's plan viewer while every Rust test stays green.
#[test]
fn repo_relative_plan_path_is_forward_slashed_and_relative() {
    let root = std::env::temp_dir().join("ralphy-planpath-test");
    let ws = Workspace::new(&root);
    assert_eq!(repo_relative_plan_path(&root, &ws), ".ralphy/plan.md");
}

#[test]
fn run_resolves_both_efforts_and_passes_them_to_both_agent_builds() {
    // Production text with whitespace removed: the calls, not their layout.
    let code: String = include_str!("../run.rs").split_whitespace().collect();
    let code = code.split("#[cfg(test)]mod").next().unwrap_or_default();
    // The argument list of the call that opens at `at` (just past its `(`),
    // without a trailing comma.
    let args_at = |open: usize| -> &str {
        let mut depth = 1;
        let len = code
            .split_at(open)
            .1
            .char_indices()
            .find_map(|(j, c)| {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                (depth == 0).then_some(j)
            })
            .expect("a balanced call");
        code.split_at(open).1.split_at(len).0.trim_end_matches(',')
    };
    // Each phase resolves from its OWN flag and its own setting.
    for phase in ["plan", "exec"] {
        let head = format!("{phase}:config::resolve_effort(");
        let at = code
            .find(&head)
            .unwrap_or_else(|| panic!("the {phase} effort is resolved"));
        assert_eq!(
            args_at(at + head.len()),
            format!("args.{phase}_effort,adapters.claude.{phase}_effort.clone(),None"),
            "the {phase} effort resolves from --{phase}-effort and claude.{phase}_effort"
        );
    }
    // The executor and the split planner are each built with it.
    let builds: Vec<&str> = code
        .match_indices("build_agent(")
        .filter(|(i, _)| !code.split_at(*i).0.ends_with("fn"))
        .map(|(i, m)| args_at(i + m.len()))
        .collect();
    assert!(
        builds.len() >= 2,
        "the executor and the planner: {builds:?}"
    );
    for args in builds {
        assert!(
            args.split(',').any(|arg| arg == "resolved_effort"),
            "every agent build must receive the resolved effort: {args}"
        );
    }
}

#[test]
fn unset_max_minutes_resolves_to_uncapped() {
    // The per-issue cap is opt-in (docs/adr/0038): an unset flag AND unset
    // setting resolve to `0`, which `issue_deadline` reads as "no per-issue
    // cap" — the issue is bounded only by `--deadline-hours`. Liveness is
    // the idle watchdog's job, not this knob's; a finite default here would
    // cut healthy long issues to catch a hang it cannot recognize.
    let resolved = config::resolve_u64(None, None, ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE);
    assert_eq!(resolved, 0);
}
#[test]
fn verify_timeout_no_longer_derives_from_the_per_issue_cap() {
    // The gate owns its own clock (docs/adr/0038): with the cap uncapped
    // (`0`, the default) verify must still get a finite window, and it must
    // not silently inherit an unrelated per-issue cap either.
    assert_eq!(
        resolve_verify_timeout_minutes(None),
        ralphy_core::VERIFY_GATE_FALLBACK_MINUTES
    );
    assert_eq!(resolve_verify_timeout_minutes(Some(15)), 15);
    assert!(resolve_verify_timeout_minutes(None) > 0);
}

fn agy_with_exec(model: &str) -> wiring::ResolvedAntigravity {
    wiring::ResolvedAntigravity {
        plan_model: None,
        exec_model: Some(model.to_owned()),
        plan_effort: None,
        exec_effort: None,
        run_without_guard_proof: false,
    }
}

#[test]
fn the_run_consolidator_of_an_agy_run_carries_its_model() {
    let agy = agy_with_exec("gemini-3.8-flash-low");
    let c = run_consolidator(CliAgent::Antigravity, &agy);
    assert!(matches!(c.agent, CliAgent::Antigravity));
    assert_eq!(c.model, Some("gemini-3.8-flash-low"));
}

#[test]
fn the_run_consolidator_of_a_claude_run_keeps_opus_medium() {
    let agy = agy_with_exec("gemini-3.8-flash-low");
    let c = run_consolidator(CliAgent::Claude, &agy);
    assert_eq!((c.model, c.effort), (Some("opus"), Some("medium")));
}
