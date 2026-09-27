use super::*;
use std::path::PathBuf;
use std::time::Duration;

/// The production half of a source file: the text before its first
/// `#[cfg(test)]` line whose next non-empty line starts with `mod `. An
/// item-level `#[cfg(test)]` (a test-only helper above production code) is not
/// the cut, so a source scan still reads the code after it. The same rule as
/// `production()` in `crates/xtask/tests/user_text_cites_no_adr.rs`.
pub(crate) fn production_text(src: &str) -> &str {
    let mut offset = 0;
    let mut lines = src.split_inclusive('\n');
    while let Some(line) = lines.next() {
        if line.trim() == "#[cfg(test)]"
            && lines
                .clone()
                .find(|next| !next.trim().is_empty())
                .is_some_and(|next| next.trim_start().starts_with("mod "))
        {
            return &src[..offset];
        }
        offset += line.len();
    }
    src
}

#[test]
fn production_text_reads_past_a_test_item() {
    let src = "use a;\r\n\
               #[cfg(test)]\r\n\
               fn helper() {}\r\n\
               fn shipped() {}\r\n\
               #[cfg(test)]\r\n\
               \r\n\
               mod tests;\r\n";
    assert_eq!(
        production_text(src),
        "use a;\r\n#[cfg(test)]\r\nfn helper() {}\r\nfn shipped() {}\r\n",
        "the cut is the test module, so the code after a test item is kept"
    );
}

#[test]
fn production_text_drops_the_test_module() {
    let src = "fn shipped() {}\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}\n";
    assert_eq!(
        production_text(src),
        "fn shipped() {}\n",
        "the inline test module is not production"
    );
    let no_tests = "fn shipped() {}\n#[cfg(unix)]\nmod unix;\n";
    assert_eq!(
        production_text(no_tests),
        no_tests,
        "a file without a test module is read whole"
    );
}

#[test]
fn copilot_agent_is_a_dyn_agent() {
    let agent = CopilotAgent::new(None, PathBuf::from("/run"));
    let _as_dyn: &dyn Agent = &agent;
}

#[test]
fn copilot_honours_max_minutes_per_issue() {
    assert_eq!(
        CopilotAgent::new(None, PathBuf::from("/run"))
            .budget
            .max_minutes_per_issue,
        ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE
    );
    let a = CopilotAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(120);
    assert_eq!(a.budget.max_minutes_per_issue, 120);
    let short = CopilotAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1);
    let long = CopilotAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1000);
    assert!(long.issue_deadline() > short.issue_deadline());
    let rd = Instant::now() + Duration::from_secs(1);
    let clamped = CopilotAgent::new(None, PathBuf::from("/run"))
        .with_max_minutes_per_issue(1000)
        .with_run_deadline(Some(rd));
    assert!(clamped.issue_deadline() <= rd);
}

#[test]
fn copilot_zero_minutes_disables_the_per_issue_cap() {
    let uncapped = CopilotAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(0);
    let capped = CopilotAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1000);
    assert!(uncapped.issue_deadline() > capped.issue_deadline());

    let rd = Instant::now() + Duration::from_secs(1);
    let bounded = CopilotAgent::new(None, PathBuf::from("/run"))
        .with_max_minutes_per_issue(0)
        .with_run_deadline(Some(rd));
    assert!(bounded.issue_deadline() <= rd);
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

#[test]
fn prompt_plan_copilot_has_no_execution_model_line() {
    assert!(
        !PROMPT_PLAN_COPILOT.contains("## Execution model"),
        "the Copilot plan prompt must drop the complexity tier line (D6)"
    );
}

#[test]
fn prompt_plan_copilot_carries_finalize_trailer() {
    assert!(
        PROMPT_PLAN_COPILOT.contains("<!-- ralphy-plan: issue=<N> -->"),
        "planning prompt must instruct writing the exact finalized-plan trailer"
    );
}

/// The hatch is the ONLY thing that turns a connected builtin server from a
/// run-failing violation into a pass — the same stream, the two agents.
#[test]
fn escape_hatch_suppresses_the_connected_failure() {
    let stream = concat!(
        r#"{"type":"session.mcp_servers_loaded","data":{"servers":[{"name":"github-mcp-server","status":"connected","source":"builtin","transport":"http"}]},"ephemeral":true}"#,
        "\n"
    );
    let strict = CopilotAgent::new(None, PathBuf::from("/run"));
    let err = strict
        .check_builtin_mcps(stream, true)
        .expect_err("a connected builtin must fail the run by default");
    assert!(err.to_string().contains("github-mcp-server"), "{err}");

    let permissive = CopilotAgent::new(None, PathBuf::from("/run")).with_allow_builtin_mcps(true);
    assert!(
        permissive.check_builtin_mcps(stream, true).is_ok(),
        "the operator's explicit hatch must suppress the failure"
    );
    // …and the hatch does not blanket-suppress: it is not a "skip all checks"
    // switch for a stream that never carried a receipt either way.
    assert!(permissive.check_builtin_mcps("", true).is_ok());
    assert!(
        strict.check_builtin_mcps("", true).is_err(),
        "an absent receipt still fails closed by default"
    );
}

/// The guard is only worth its tests if it is actually WIRED. Every D7 test
/// calls the predicate directly, and no test here builds a `Workspace`, so
/// `plan`/`execute` are invisible to the suite — deleting both call sites
/// would leave everything green and silently turn ADR-0041 D7 into a no-op.
/// This pins the call sites in the source, the same mechanism
/// `no_direct_command_new` and `runstate/capture.rs` use. Fragments are
/// assembled with `concat!` so the assertion cannot match ITSELF.
#[test]
fn the_receipt_guard_is_wired_into_both_phases() {
    let src = include_str!("lib.rs");
    let call = concat!("self.check_builtin_mcps(", "&r.stdout, r.exited_cleanly)");
    assert_eq!(
        src.matches(call).count(),
        2,
        "D7's guard must be called on BOTH the plan and the execute path"
    );
}

/// The D9 seam itself, not just its source-text pin: replacing
/// `check_skills_loaded`'s body with `Ok(())` must RED something. Mirrors the
/// D7 seam test above — a pin alone counts substrings and cannot see a gutted
/// body. Also proves D9 carries NO escape hatch: the D7 hatch must not
/// suppress a missing skill.
#[test]
fn check_skills_loaded_fails_a_run_missing_a_ralphy_skill() {
    let required = vec!["reviewer".to_string(), "staged-plan".to_string()];
    let missing = concat!(
        r#"{"type":"session.skills_loaded","data":{"skills":[{"name":"reviewer"}]},"ephemeral":true}"#,
        "\n"
    );
    let agent = CopilotAgent::new(None, PathBuf::from("/run"));
    let err = agent
        .check_skills_loaded(missing, &required, true)
        .expect_err("a missing ralphy skill must fail the run");
    assert!(err.to_string().contains("staged-plan"), "{err}");

    // The D7 hatch is scoped to D7: it must NOT suppress the capability guard.
    let permissive = CopilotAgent::new(None, PathBuf::from("/run")).with_allow_builtin_mcps(true);
    assert!(
        permissive
            .check_skills_loaded(missing, &required, true)
            .is_err(),
        "D9 has no escape hatch; the D7 hatch must not suppress it"
    );

    // A receipt listing both passes through the seam.
    let complete = concat!(
        r#"{"type":"session.skills_loaded","data":{"skills":[{"name":"reviewer"},{"name":"staged-plan"}]},"ephemeral":true}"#,
        "\n"
    );
    assert!(agent.check_skills_loaded(complete, &required, true).is_ok());
}

/// Same reasoning as D7's pin, for D9: no test here constructs a `Workspace`,
/// so deleting either call site would leave the suite green and ADR-0041 D9 a
/// silent no-op. Pins both the materialization and the receipt assertion.
#[test]
fn the_skills_guard_is_wired_into_both_phases() {
    let src = include_str!("lib.rs");
    let call = concat!(
        "self.check_skills_loaded(",
        "&r.stdout, &required, r.exited_cleanly)"
    );
    assert_eq!(
        src.matches(call).count(),
        2,
        "D9's guard must be called on BOTH the plan and the execute path"
    );
    assert_eq!(
        src.matches(concat!("materialize_copilot", "_skills(ws)?"))
            .count(),
        2,
        "skills must be materialized on BOTH the plan and the execute path"
    );
}

fn argv(cmd: &std::process::Command) -> Vec<String> {
    cmd.get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

/// The body of the method whose signature starts with `sig`, with all
/// whitespace removed: from the signature to the first line that is only the
/// method's closing brace at impl indentation (rustfmt layout).
fn method_body(src: &str, sig: &str) -> String {
    let start = src
        .find(sig)
        .unwrap_or_else(|| panic!("`{sig}` is not in the source"));
    let mut body = String::new();
    for line in src[start..].split_inclusive('\n') {
        body.push_str(line);
        if line.trim_end() == "    }" {
            break;
        }
    }
    body.split_whitespace().collect()
}

/// The argv tests below prove `phase_model`/`phase_effort` and the builder.
/// This pins that each phase asks for its own model and effort, so `plan`
/// cannot run with the execute pins or the reverse.
#[test]
fn each_phase_reads_its_own_model_and_effort() {
    let src = production_text(include_str!("lib.rs"));
    let plan = method_body(src, "fn plan(");
    let execute = method_body(src, "fn execute(");
    for (name, body, own, other) in [
        ("plan", &plan, "Phase::Plan", "Phase::Execute"),
        ("execute", &execute, "Phase::Execute", "Phase::Plan"),
    ] {
        for call in ["self.phase_model(", "self.phase_effort("] {
            let expected = format!("{call}{own})");
            assert!(body.contains(&expected), "`{name}` must call `{expected}`");
        }
        assert!(!body.contains(other), "`{name}` must not read `{other}`");
    }
}

#[test]
fn the_builder_puts_the_given_model_in_argv() {
    let agent = CopilotAgent::new(Some("exec-pin".into()), PathBuf::from("/run"))
        .with_plan_model(Some("plan-pin".into()));
    let cmd = build_copilot_command(
        "s1",
        agent.phase_model(Phase::Plan),
        None,
        std::path::Path::new("/repo"),
        false,
        &[],
    );
    let args = argv(&cmd);
    let i = args.iter().position(|a| a == "--model").unwrap();
    assert_eq!(args[i + 1], "plan-pin");
}

#[test]
fn phase_model_gives_the_exec_pin_to_the_builder() {
    let agent = CopilotAgent::new(Some("exec-pin".into()), PathBuf::from("/run"))
        .with_plan_model(Some("plan-pin".into()));
    let cmd = build_copilot_command(
        "s1",
        agent.phase_model(Phase::Execute),
        None,
        std::path::Path::new("/repo"),
        false,
        &[],
    );
    let args = argv(&cmd);
    let i = args.iter().position(|a| a == "--model").unwrap();
    assert_eq!(args[i + 1], "exec-pin");
}

#[test]
fn the_builder_omits_model_when_unpinned() {
    let agent = CopilotAgent::new(None, PathBuf::from("/run"));
    for phase in [Phase::Plan, Phase::Execute] {
        let cmd = build_copilot_command(
            "s1",
            agent.phase_model(phase),
            None,
            std::path::Path::new("/repo"),
            false,
            &[],
        );
        let args = argv(&cmd);
        assert!(!args.iter().any(|a| a == "--model"), "argv: {args:?}");
    }
}

fn fixture_catalog() -> CopilotCatalog {
    parse_catalog(
        include_str!("../fixtures/capi-models-2026-07-20.log"),
        "probe-1",
    )
    .expect("the fixture parses")
}

/// The end-to-end shape of D5a on the plan phase: an `xhigh` request against a
/// model that publishes only `low/medium/high` rides the argv as `high`.
#[test]
fn the_builder_carries_the_clamped_effort_in_argv() {
    let agent = CopilotAgent::new(None, PathBuf::from("/run"))
        .with_plan_model(Some("gpt-5-mini".into()))
        .with_plan_effort(Some("xhigh".into()));
    let cat = fixture_catalog();
    let model = agent.phase_model(Phase::Plan);
    let effort = agent
        .phase_effort(Phase::Plan)
        .and_then(|e| effort::resolve_effort(Some(e), model, Some(&cat)));
    let cmd = build_copilot_command(
        "s1",
        model,
        effort.as_deref(),
        std::path::Path::new("/repo"),
        false,
        &[],
    );
    let args = argv(&cmd);
    let i = args
        .iter()
        .position(|a| a == "--effort")
        .unwrap_or_else(|| panic!("--effort missing: {args:?}"));
    assert_eq!(args[i + 1], "high");
}

/// The default run: no effort requested, no `--effort` token, and the catalog
/// is never consulted (`phase_effort` short-circuits before `and_then`).
#[test]
fn the_builder_omits_effort_when_unset() {
    let agent = CopilotAgent::new(None, PathBuf::from("/run"));
    for phase in [Phase::Plan, Phase::Execute] {
        let effort = agent
            .phase_effort(phase)
            .and_then(|e| effort::resolve_effort(Some(e), None, None));
        assert_eq!(effort, None);
        let cmd = build_copilot_command(
            "s1",
            agent.phase_model(phase),
            effort.as_deref(),
            std::path::Path::new("/repo"),
            false,
            &[],
        );
        let args = argv(&cmd);
        assert!(!args.iter().any(|a| a == "--effort"), "argv: {args:?}");
    }
}

/// A default run must not touch the vendor's session store for effort: the
/// reader COPIES the whole database, and `effort_mismatch`'s own `?` cannot
/// prevent it — Rust evaluates arguments before the call. The counter is what
/// makes "reads nothing" observable; without it the eager form passes too.
#[test]
fn no_effort_requested_reads_no_session_store() {
    use std::cell::Cell;
    let reads = Cell::new(0);
    warn_effort_mismatch_with(None, "s1", || {
        reads.set(reads.get() + 1);
        Some("high".into())
    });
    assert_eq!(reads.get(), 0, "a default run must read no store");

    warn_effort_mismatch_with(Some("high"), "s1", || {
        reads.set(reads.get() + 1);
        Some("medium".into())
    });
    assert_eq!(reads.get(), 1, "a requested effort IS verified post-hoc");
}

/// The reason the charter goes on stdin and never on argv (D2): at 23 884 bytes
/// it alone is within ~30 % of the Windows ~32 KB argv ceiling, before the issue
/// body is even appended. The floor is 23 000 — a real margin under today's
/// size, so the test pins the ORDER of magnitude rather than the exact byte
/// count, which every prompt edit would otherwise churn.
#[test]
fn exec_charter_exceeds_argv_safe_size() {
    assert!(
        ralphy_adapter_support::PROMPT_EXECUTE.len() > 23_000,
        "charter is {} bytes",
        ralphy_adapter_support::PROMPT_EXECUTE.len()
    );
}
