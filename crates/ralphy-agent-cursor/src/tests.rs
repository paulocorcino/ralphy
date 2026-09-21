use super::*;
use std::time::Duration;

/// Story 21: a pinned run must be distinguishable from a routed one in the run
/// report, and the routed one must not read as "not reported".
#[test]
fn the_requested_model_is_attributed_and_auto_is_named() {
    assert_eq!(
        requested_model_usage(Some("composer-2.5-fast"))
            .model
            .as_deref(),
        Some("composer-2.5")
    );
    assert_eq!(requested_model_usage(None).model.as_deref(), Some("auto"));
    // Token counts stay zero until #249, so no cost is fabricated.
    assert_eq!(requested_model_usage(None).input, 0);
}

#[test]
fn accepts_images_is_false() {
    // Read through a binding: a bare `assert!(!CONST)` is constant-folded and
    // clippy rejects it, but the invariant is still worth pinning here — the
    // CLI's onboarding gate asserts the same const from the other side.
    let accepts: bool = ACCEPTS_IMAGES;
    assert!(
        !accepts,
        "ADR-0042 D15: no attachment channel exists in the headless surface"
    );
}

#[test]
fn cursor_agent_is_a_dyn_agent() {
    let agent = CursorAgent::new(None, PathBuf::from("/run"));
    let _as_dyn: &dyn Agent = &agent;
    assert_eq!(agent.name(), "cursor");
}

#[test]
fn cursor_honours_max_minutes_per_issue() {
    assert_eq!(
        CursorAgent::new(None, PathBuf::from("/run"))
            .budget
            .max_minutes_per_issue,
        ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE
    );
    let short = CursorAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1);
    let long = CursorAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1000);
    assert!(long.issue_deadline() > short.issue_deadline());
    let rd = Instant::now() + Duration::from_secs(1);
    let clamped = CursorAgent::new(None, PathBuf::from("/run"))
        .with_max_minutes_per_issue(1000)
        .with_run_deadline(Some(rd));
    assert!(clamped.issue_deadline() <= rd);
}

/// ADR-0042 D3: this vendor opens with ~8.1 s of silence and shows inter-record
/// gaps up to ~7.4 s, so a watchdog in seconds would reap healthy runs. Unlike
/// `max_minutes_per_issue`, `IssueBudget::new` leaves `idle_minutes` at `0` —
/// the CLI wiring layer (`run/wiring.rs`) applies `DEFAULT_IDLE_MINUTES` before
/// handing a `CursorAgent` to a run, so this pins the CONSTANT the wiring
/// relies on plus the plumbing, rather than a fresh agent's own field.
#[test]
fn the_idle_watchdog_default_tolerates_the_vendor_cadence() {
    // Read through a binding: a bare constant assertion is constant-folded and
    // clippy rejects it (see `accepts_images_is_false`).
    let idle_minutes: u64 = ralphy_core::DEFAULT_IDLE_MINUTES;
    assert!(
        idle_minutes * 60 >= 60,
        "measured ~8.1s opening silence, ~7.4s inter-record gaps"
    );
    let agent = CursorAgent::new(None, PathBuf::from("/run"))
        .with_idle_minutes(ralphy_core::DEFAULT_IDLE_MINUTES);
    assert_eq!(agent.budget.idle_minutes, ralphy_core::DEFAULT_IDLE_MINUTES);
}

/// Story 33: both phases must report the stream's OWN usage, not the
/// requested-model-only attribution — deleting either call site keeps the
/// suite green unless this pin catches it (#249).
#[test]
fn both_phases_report_stream_usage() {
    let src = include_str!("lib.rs");
    let call = concat!("parse_cursor_usage(", "&r.stdout,");
    assert_eq!(
        src.matches(call).count(),
        2,
        "both phases must report the stream's own usage (#249)"
    );
}

/// Story 33: both phases must state the credit/token unit mismatch —
/// deleting either call site keeps the suite green unless this pin catches
/// it.
#[test]
fn every_run_notes_the_credit_unit_mismatch() {
    let src = include_str!("lib.rs");
    let call = concat!("note_usage_provenance(", "&self");
    assert_eq!(
        src.matches(call).count(),
        2,
        "story 33: both phases must state the credit/token unit mismatch"
    );
}

/// A source-text pin in the style of `outcome.rs::the_gate_runs_before_any_child_is_spawned`:
/// the operator-visible degraded-tool-call note must be raised on BOTH the
/// `plan` and `execute` paths, and on `execute` only after the fold has run —
/// deleting either call keeps the suite green unless this pin catches it.
#[test]
fn execute_notes_the_degraded_calls() {
    let src = include_str!("lib.rs");
    let call = concat!("note_degraded(", "&fold);");
    assert_eq!(
        src.matches(call).count(),
        2,
        "note_degraded(&fold) must be called on both the plan and execute paths"
    );
    let fold_call = concat!("fold_cursor_stream(", "&r.stdout);");
    let last_fold = src.rfind(fold_call).expect("execute's fold call site");
    let last_note = src.rfind(call).expect("execute's note_degraded call site");
    assert!(
        last_note > last_fold,
        "execute must fold the stream before it can note the degraded calls"
    );
    // Same pin for the vendor's own stop reason: dropping it is exactly the
    // regression that made a quota refusal arrive as a mute `Stuck`.
    let vendor_call = concat!("note_vendor_error(", "&fold);");
    assert_eq!(
        src.matches(vendor_call).count(),
        2,
        "note_vendor_error(&fold) must be called on both the plan and execute paths"
    );
}

/// D17: the scratch dir is per RUN and under the run dir, never the operator's.
#[test]
fn the_config_dir_lives_under_the_run_dir() {
    let agent = CursorAgent::new(None, PathBuf::from("/run/abc"));
    assert_eq!(agent.config_dir(), PathBuf::from("/run/abc/cursor-config"));
}

/// D10: adoption is verified, not assumed. A mismatch is an error because every
/// later store lookup would otherwise address another session.
#[test]
fn a_session_id_mismatch_is_an_error() {
    let minted = "868f1553-01ac-4335-89c6-6c1f101d6009";
    assert!(CursorAgent::verify_session_adoption(minted, Some(minted)).is_ok());
    // No `system/init` at all (a truncated stream) is not a mismatch — the
    // missing envelope is what classifies that run.
    assert!(CursorAgent::verify_session_adoption(minted, None).is_ok());
    let err = CursorAgent::verify_session_adoption(minted, Some("other-id"))
        .expect_err("a different id must abort");
    assert!(err.to_string().contains("other-id"), "{err}");
}

/// The default hatch is OFF: a fresh agent protects an un-opted-out repository
/// (writes the opt-out) rather than allowing the upload.
#[test]
fn indexing_is_off_by_default_and_reachable_on_request() {
    assert!(!CursorAgent::new(None, PathBuf::from("/run")).allow_indexing);
    assert!(
        CursorAgent::new(None, PathBuf::from("/run"))
            .with_allow_indexing(true)
            .allow_indexing
    );
}

/// D2's reason: the charter alone is within ~30 % of the Windows ~32 KB argv
/// ceiling before the issue body is appended, so stdin is the only safe channel.
/// The floor pins the ORDER of magnitude, not a byte count every prompt edit
/// would churn.
#[test]
fn plan_charter_exceeds_argv_safe_size() {
    assert!(
        PROMPT_PLAN_CURSOR.len() > 23_000,
        "charter is {} bytes",
        PROMPT_PLAN_CURSOR.len()
    );
}

#[test]
fn prompt_plan_cursor_carries_finalize_trailer() {
    assert!(
        PROMPT_PLAN_CURSOR.contains("<!-- ralphy-plan: issue=<N> -->"),
        "planning prompt must instruct writing the exact finalized-plan trailer"
    );
}

/// D9: the vendor's native plan mode is hard read-only and overrides the
/// charter, so the overlay must tell the planner to write the file itself.
#[test]
fn prompt_plan_cursor_requires_the_planner_to_write_the_file() {
    assert!(
        PROMPT_PLAN_CURSOR.contains("you MUST write `.ralphy/plan.md` yourself"),
        "D9: the planner writes its own plan on this vendor"
    );
}

/// #266: the plan path routes a quota stop to `PlanLimit`, and a hard
/// `--model` refusal is checked FIRST — it will not heal on a retry, so
/// scheduling a wait for it would burn the issue's budget re-asking an
/// already-answered question.
#[test]
fn the_plan_path_routes_a_quota_stop_to_plan_limit() {
    let src = include_str!("lib.rs");
    let refusal = concat!("model_refusal_stop(", "log, model)");
    let limit = concat!("PlanLimit { reset: ", "None }");
    let at_refusal = src
        .find(refusal)
        .expect("plan()'s on_missing must check the model refusal");
    let at_limit = src
        .find(limit)
        .expect("plan()'s on_missing must route a quota stop to PlanLimit");
    assert!(
        at_refusal < at_limit,
        "a hard refusal must be checked before the limit"
    );
}

/// #271: `plan()` must gate the resume/plan-reuse decision on a fresh login
/// verdict, BEFORE `run_plan_session` — otherwise a leftover finalized plan.md
/// resumes without spawning a child and the in-flight auth matcher never runs,
/// serving a logged-out operator a stale plan instead of the `agent login`
/// stop. The probe is short-circuited on `plan_is_finalized_for` so a fresh plan
/// pays no extra spawn. Needles assembled from fragments so this cannot match
/// itself.
#[test]
fn the_resume_path_is_gated_on_a_fresh_login_verdict() {
    let src = include_str!("lib.rs");
    let plan = src
        .split_once("fn plan(")
        .expect("plan()")
        .1
        .split_once("fn execute(")
        .map(|(p, _)| p)
        .expect("plan body ends before execute()");
    let gate = concat!("resume_requires_", "login(");
    let finalized = concat!("plan_is_finalized_", "for(&plan_path, issue.number)");
    let spawn = concat!("run_plan_", "session(");
    let at_gate = plan.find(gate).expect("plan() must gate the resume path");
    let at_spawn = plan.find(spawn).expect("plan() must call run_plan_session");
    assert!(
        at_gate < at_spawn,
        "the login gate must precede the resume/plan-reuse decision"
    );
    assert!(
        plan.contains(finalized),
        "the probe must be short-circuited on a finalized plan (zero cost on a fresh plan)"
    );
}

/// #266: whatever reaches Ralphy already exhausted the vendor's own retries —
/// no production path may re-spawn on a quota stop. Pins BOTH halves: the
/// crate's single `HeadlessCall` site stays singular, and no production
/// source loops around a limit check.
#[test]
fn no_adapter_side_retry_of_a_quota_stop() {
    fn sources(dir: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("readable src dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.file_stem().and_then(|s| s.to_str()) == Some("tests") {
                // A sibling test module (ADR-0022 §3) carries no `#[cfg(test)]`
                // marker of its own; it is test code, not production.
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                let body = std::fs::read_to_string(&path).expect("read source");
                out.push(body.split("#[cfg(test)]").next().unwrap_or("").to_string());
            }
        }
    }
    let mut production = Vec::new();
    sources(
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")),
        &mut production,
    );
    let spawn = concat!("HeadlessCall::", "new(cmd,");
    let spawn_count: usize = production.iter().map(|s| s.matches(spawn).count()).sum();
    assert_eq!(
        spawn_count, 1,
        "the crate's single HeadlessCall site must stay singular"
    );
    // Scoped to the files that call `cursor_limit_note`, not the whole crate:
    // `model.rs` has its own unrelated fixpoint `loop {}` (decoration
    // stripping), which is not a retry and must not trip this pin.
    let limit_call = "cursor_limit_note(";
    for body in &production {
        if body.contains(limit_call) {
            assert!(
                !body.contains("loop {") && !body.contains("while "),
                "no production path may loop around a limit check — a quota \
                     stop already exhausted the vendor's own retries"
            );
        }
    }
}

/// ADR-0040 Tier 1: adapter tests are inline `#[cfg(test)] mod tests`, never a
/// `tests/` directory — an integration dir would re-link the crate and lose
/// access to the `pub(crate)` seams every test here asserts on.
#[test]
fn no_tests_directory() {
    assert!(
        !std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests")).exists(),
        "adapter tests stay inline (ADR-0040 Tier 1)"
    );
}
