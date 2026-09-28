use super::*;

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

/// Production code of `src` without comment lines and with all whitespace
/// removed, so a pin matches the call and not its layout.
pub(crate) fn code_of(src: &str) -> String {
    production_text(src)
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .flat_map(str::split_whitespace)
        .collect()
}

/// The body of the first function whose header starts with `header` (for
/// example `"fnexecute("` in [`code_of`] text), braces matched.
pub(crate) fn fn_body<'a>(code: &'a str, header: &str) -> &'a str {
    let start = code
        .find(header)
        .unwrap_or_else(|| panic!("no function starts with {header:?}"));
    let open = start + code[start..].find('{').expect("a function body");
    let mut depth = 0;
    for (i, c) in code[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &code[open..=open + i];
                }
            }
            _ => {}
        }
    }
    panic!("the body after {header:?} is not balanced")
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
    let code = code_of(include_str!("lib.rs"));
    let body = fn_body(&code, "fnexecute(");
    // The underscore is a convention, not a compiler guarantee — `_plan.…` is
    // legal Rust. The pin is that the plan binding is never MENTIONED again
    // inside the body, which is the only thing that makes the executor
    // plan-agnostic.
    let param = code
        .split_once("fnexecute(&self,")
        .and_then(|(_, rest)| rest.split_once(":&Plan"))
        .map(|(name, _)| name.to_string())
        .expect("execute takes the plan as its first argument");
    assert!(
        !body.contains(&format!("{param}.")) && !body.contains(&format!("({param}")),
        "the plan artifact is never read: `{param}` must not appear in execute's body"
    );
    // The shared vendor-neutral charter is the base of the stdin, built once
    // via the #275 inliner, and piped once. `PROMPT_EXECUTE` reaches the child
    // only through `context::exec_stdin` — never a second, plan-specific one.
    assert_eq!(
        body.matches("context::exec_stdin(PROMPT_EXECUTE,").count(),
        1,
        "the execute stdin is the shared charter, inlined once"
    );
    assert_eq!(
        body.matches("self.run_gemini(").count(),
        1,
        "the inlined charter is piped once"
    );
    // HEAD is sampled BEFORE the child can commit anything, and again only
    // after the session has ended; the two samples decide `committed`.
    let samples: Vec<usize> = body.match_indices("head_sha(").map(|(i, _)| i).collect();
    let session = body
        .find("run_exec_session(")
        .expect("execute runs the shared session");
    assert_eq!(samples.len(), 2, "HEAD is sampled twice");
    assert!(samples[0] < session && session < samples[1]);
    assert!(
        body.contains("letcommitted=before_sha!=after_sha;")
            || body.contains("letcommitted=after_sha!=before_sha;"),
        "committed compares the two HEAD samples"
    );
}

/// D11 (#264): Ralphy adds no retry layer of its own — a `Limit(None)` stops
/// the phase and the queue's synthetic cadence (ADR-0030) is what resumes
/// it, never a loop inside the adapter. Pinned on the source, because an
/// absent retry site is invisible to a behavioural test: one child spawn per
/// phase (plan, execute), and no loop/while/retry between it and the
/// session runner that follows.
#[test]
fn ralphy_adds_no_retry_of_its_own() {
    let code = code_of(include_str!("lib.rs"));
    assert_eq!(
        code.matches("self.run_gemini(").count(),
        2,
        "one child per phase — plan and execute; a third site would be a Ralphy-side retry"
    );
    for header in ["fnplan(", "fnexecute("] {
        let body = fn_body(&code, header);
        for needle in ["loop{", "while", "retry"] {
            assert!(
                !body.contains(needle),
                "no {needle:?} in {header}: a phase spawns its child once"
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

/// The per-issue setter reaches the budget, and the run deadline clamps it.
#[test]
fn budget_setters_reach_the_issue_deadline() {
    let run_deadline = Instant::now() + std::time::Duration::from_secs(1);
    let agent = GeminiAgent::new(None, std::path::PathBuf::from("/run"))
        .with_max_minutes_per_issue(120)
        .with_run_deadline(Some(run_deadline));
    assert_eq!(agent.budget.max_minutes_per_issue, 120);
    assert_eq!(
        agent.budget.deadline(ralphy_core::UNBOUNDED_ISSUE_HORIZON),
        run_deadline
    );
}
