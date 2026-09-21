use super::*;
use std::time::Duration;

#[test]
fn accepts_images_is_true() {
    // Read through a binding: a bare `assert!(CONST)` is constant-folded and
    // clippy rejects it, but the invariant is worth pinning here — the CLI's
    // onboarding gate asserts the same const from the other side.
    let accepts: bool = ACCEPTS_IMAGES;
    assert!(
        accepts,
        "ADR-0043 D14: the headless surface takes `@<path>`"
    );
}

#[test]
fn gemini_agent_is_a_dyn_agent() {
    let agent = GeminiAgent::new(None, PathBuf::from("/run"));
    let _as_dyn: &dyn Agent = &agent;
    assert_eq!(agent.name(), "gemini");
}

/// ADR-0044 D4: resolved effort is stored on the agent and discarded at
/// plan/execute — the command builder has no effort parameter (argv covered
/// in `command::tests`; this module must not call `build_gemini_command`,
/// which would break the two-site root pin).
#[test]
fn resolved_effort_is_stored_for_documented_discard() {
    let agent = GeminiAgent::new(None, PathBuf::from("/run"))
        .with_plan_effort(Some("high".into()))
        .with_exec_effort(Some("high".into()));
    assert_eq!(agent.plan_effort.as_deref(), Some("high"));
    assert_eq!(agent.exec_effort.as_deref(), Some("high"));
    let prod = include_str!("lib.rs")
        .split("\nmod tests {")
        .next()
        .expect("production half");
    assert!(
        prod.contains("let _ = self.plan_effort.as_deref();"),
        "plan must discard plan_effort before emit"
    );
    assert!(
        prod.contains("let _ = self.exec_effort.as_deref();"),
        "execute must discard exec_effort before emit"
    );
}

#[test]
fn the_phase_model_reads_the_matching_override() {
    let agent = GeminiAgent::new(Some("exec-m".into()), PathBuf::from("/run"))
        .with_plan_model(Some("plan-m".into()));
    assert_eq!(agent.phase_model(Phase::Plan), Some("plan-m"));
    assert_eq!(agent.phase_model(Phase::Execute), Some("exec-m"));
    let bare = GeminiAgent::new(None, PathBuf::from("/run"));
    assert_eq!(bare.phase_model(Phase::Plan), None);
    assert_eq!(bare.phase_model(Phase::Execute), None);
}

/// The ledger key and the price key must be ONE string (ADR-0034 amendment,
/// #257). Attributing the RAW id would cost a routed run out against another
/// vendor's `auto` row, and a `gemini-3-flash` run at a third of its price —
/// so the fold through `price_key` is asserted here, not just in the table's
/// own tests.
#[test]
fn phase_usage_attributes_the_price_key_not_the_raw_id() {
    // Unpinned: the routed sentinel, which is deliberately unpriced.
    assert_eq!(
        phase_usage(None, None).model.as_deref(),
        Some("gemini-routed")
    );
    assert_eq!(
        phase_usage(None, Some("auto")).model.as_deref(),
        Some("gemini-routed")
    );
    // The 3× trap: the CLI's constant is served by the 3.5 backend.
    assert_eq!(
        phase_usage(None, Some("gemini-3-flash")).model.as_deref(),
        Some("gemini-3.5-flash")
    );
    // A concrete id is attributed verbatim.
    assert_eq!(
        phase_usage(None, Some("gemini-2.5-pro")).model.as_deref(),
        Some("gemini-2.5-pro")
    );
}

/// D9: a fold that saw a terminal record but no `stats` key reports no
/// usage rather than zero usage — `phase_usage` must not paper over the
/// `None`/`Some(vec![])` distinction `outcome::GeminiFold.usage` carries.
#[test]
fn phase_usage_reports_no_usage_when_the_envelope_carried_none() {
    let fold = fold_gemini_stream(r#"{"type":"result","status":"success"}"#);
    let usage = phase_usage(Some(&fold), None);
    assert_eq!(usage.total(), 0);
    assert_eq!(usage.model.as_deref(), Some("gemini-routed"));
}

#[test]
fn gemini_honours_max_minutes_per_issue() {
    assert_eq!(
        GeminiAgent::new(None, PathBuf::from("/run"))
            .budget
            .max_minutes_per_issue,
        ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE
    );
    let short = GeminiAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1);
    let long = GeminiAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1000);
    assert!(long.issue_deadline() > short.issue_deadline());
    let rd = Instant::now() + Duration::from_secs(1);
    let clamped = GeminiAgent::new(None, PathBuf::from("/run"))
        .with_max_minutes_per_issue(1000)
        .with_run_deadline(Some(rd));
    assert!(clamped.issue_deadline() <= rd);
}

/// D2's reason: the charter alone is a large fraction of the Windows ~32 KB
/// argv ceiling before the issue body is appended, so stdin is the only safe
/// channel. The floor pins the ORDER of magnitude, not a byte count every
/// prompt edit would churn.
#[test]
fn plan_charter_exceeds_argv_safe_size() {
    assert!(
        PROMPT_PLAN_GEMINI.len() > 23_000,
        "charter is {} bytes",
        PROMPT_PLAN_GEMINI.len()
    );
}

#[test]
fn prompt_plan_gemini_carries_finalize_trailer() {
    assert!(
        PROMPT_PLAN_GEMINI.contains("<!-- ralphy-plan: issue=<N> -->"),
        "planning prompt must instruct writing the exact finalized-plan trailer"
    );
}

/// D12: the vendor's native plan mode writes into a vendor-private directory
/// regardless of instruction, so the overlay must tell the planner to write
/// the file itself.
#[test]
fn prompt_plan_gemini_requires_the_planner_to_write_the_file() {
    assert!(
        PROMPT_PLAN_GEMINI.contains("you MUST write `.ralphy/plan.md` yourself"),
        "D12: the planner writes its own plan on this vendor"
    );
}

/// The executor is PLAN-AGNOSTIC: it consumes whatever `.ralphy/plan.md` the
/// planning pass left, whichever adapter wrote it, and it bounds the commit by
/// reading HEAD around the child rather than trusting the stream (which carries
/// no file-change accounting for work done through the shell).
///
/// Pinned on the source because both properties are ABSENCES — a `_plan` never
/// inspected, and a `before_sha` read before the spawn — and an absence is what
/// a behavioural test cannot see.
#[test]
fn execute_is_plan_agnostic_and_bounds_the_commit() {
    // Split on the test module, NOT on `#[cfg(test)]`: an earlier one guards
    // `issue_deadline`, which would truncate the production half before
    // `execute` and make every assertion below vacuously unreachable.
    let prod = include_str!("lib.rs")
        .split("\nmod tests {")
        .next()
        .unwrap();
    const SIG: &str = "fn execute(&self, _plan: &Plan, ws: &Workspace)";
    // …and scope every assertion to `execute`'s own body: `plan` above it has
    // its own `let run = ||`, which a whole-file `find` reaches first.
    let start = prod
        .find(SIG)
        .unwrap_or_else(|| panic!("execute's signature must read exactly {SIG:?}"));
    let src = &prod[start..];
    // The underscore is a convention, not a compiler guarantee — `_plan.…` is
    // legal Rust. The pin is that the binding is never MENTIONED again inside
    // the body, which is the only thing that makes the executor plan-agnostic.
    let body_end = src.find("\n    }\n").unwrap_or(src.len());
    assert!(
        !src[SIG.len()..body_end].contains("_plan"),
        "the plan artifact is never read: `_plan` must not appear in execute's body"
    );
    // The shared vendor-neutral charter is the base of the stdin, built once
    // via the #275 inliner, and piped once. `PROMPT_EXECUTE` reaches the child
    // only through `context::exec_stdin` — never a second, plan-specific one.
    assert_eq!(
        src.matches("context::exec_stdin(PROMPT_EXECUTE, ws)")
            .count(),
        1,
        "the execute stdin is the shared charter, inlined once"
    );
    assert_eq!(
        src.matches("self.run_gemini(cmd, &exec_prompt, timeout)")
            .count(),
        1,
        "the inlined charter is piped once"
    );
    let at = |needle: &str| {
        src.find(needle)
            .unwrap_or_else(|| panic!("execute's body must still contain {needle:?}"))
    };
    assert!(
        at("let before_sha") < at("let run = ||"),
        "HEAD must be sampled BEFORE the child can commit anything"
    );
    assert!(
        at("run_exec_session(") < at("let after_sha"),
        "…and again only after the session has ended"
    );
    assert!(at("let after_sha") < at("let committed = before_sha != after_sha;"));
}

/// D11 (#264): Ralphy adds no retry layer of its own — a `Limit(None)` stops
/// the phase and the queue's synthetic cadence (ADR-0030) is what resumes
/// it, never a loop inside the adapter. Pinned on the source, because an
/// absent retry site is invisible to a behavioural test: one child spawn per
/// phase (plan, execute), and no loop/while/retry between it and the
/// session runner that follows.
#[test]
fn ralphy_adds_no_retry_of_its_own() {
    let prod = include_str!("lib.rs")
        .split("\nmod tests {")
        .next()
        .unwrap();
    assert_eq!(
        prod.matches("self.run_gemini(").count(),
        2,
        "one child per phase — plan and execute; a third site would be a Ralphy-side retry"
    );
    let starts: Vec<usize> = prod.match_indices("let run = ||").map(|(i, _)| i).collect();
    assert_eq!(
        starts.len(),
        2,
        "plan and execute each define their own `run` closure"
    );
    let ends = [
        prod[starts[0]..]
            .find("run_plan_session(")
            .map(|i| starts[0] + i)
            .expect("plan's closure is followed by run_plan_session"),
        prod[starts[1]..]
            .find("run_exec_session(")
            .map(|i| starts[1] + i)
            .expect("execute's closure is followed by run_exec_session"),
    ];
    for (start, end) in starts.iter().zip(ends.iter()) {
        let slice = &prod[*start..*end];
        for needle in ["loop {", "while ", "retry"] {
            assert!(
                !slice.contains(needle),
                "no {needle:?} between a phase's spawn and its session runner: found in {slice:?}"
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

/// Run accounting comes ONLY from the streamed envelope (ADR-0043 D9); the
/// vendor's session store is `ralphy-usage-scan`'s territory for
/// *interactive* usage (ADR-0043 D10, #261/#262), never the adapter's own.
/// Scoped to the run-accounting files only — `ralphy-usage-scan`'s
/// `scan_gemini` legitimately reads the store's session directory and must
/// stay green.
#[test]
fn run_accounting_never_reads_the_session_store() {
    // Built from parts so this pin does not trip on its own doc comment.
    let needle = ["chat", "s/"].concat();
    for src in [
        include_str!("lib.rs"),
        include_str!("tests.rs"),
        include_str!("usage.rs"),
        include_str!("outcome.rs"),
        include_str!("outcome/tests.rs"),
    ] {
        assert!(
            !src.contains(&needle),
            "found a session-store path reference"
        );
    }
}
