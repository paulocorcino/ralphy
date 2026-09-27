# Stage 5 — P0 Rust wiring tests and K-3 — Post-stage report

**Backlog items:** P0-10, P0-11, P0-12, P0-13, P0-14, P0-15, K-3
**Commit:** _filled by parent_
**Plan:** issue-485-test-suite-cleanup.md

## Files changed
- `crates/ralphy-agent-claude/src/headless.rs` (test module only) - P0-10: deleted
  `loop_exhaustion_yields_maxcalls` and `maxcalls_outcome_is_stuck`. `run_headless_steps`
  stays: `stuck_fires_after_two_consecutive_no_commit_calls` and
  `commit_resets_no_commit_streak` use it. `headless_reason_maxcalls_maps_to_stuck` stays as
  the Stuck mapping test. Section comment no longer says "MaxCalls".
- `crates/ralphy-agent-copilot/src/tests.rs` - P0-11: new helper `method_body` and new test
  `each_phase_reads_its_own_model_and_effort` (body slice of `fn plan(` / `fn execute(` in
  `lib.rs`, whitespace removed). Five argv tests renamed:
  `plan_phase_uses_plan_model_in_argv` -> `the_builder_puts_the_given_model_in_argv`,
  `execute_phase_uses_exec_model_in_argv` -> `phase_model_gives_the_exec_pin_to_the_builder`,
  `both_phases_omit_model_when_unpinned` -> `the_builder_omits_model_when_unpinned`,
  `plan_phase_clamps_its_effort_in_argv` -> `the_builder_carries_the_clamped_effort_in_argv`,
  `both_phases_omit_effort_when_unset` -> `the_builder_omits_effort_when_unset`.
- `crates/ralphy-cli/tests/mutate.rs` - P0-12: helpers `hold_run_lock`, `ralphy_config`,
  `assert_config_refused_under_held_lock`; tests `config_set_refuses_under_held_lock` and
  `config_unset_refuses_under_held_lock` (real binary, lock held by `runlock_test_child`,
  lock-free `config set` as the control, settings bytes compared).
- `crates/ralphy-cli/src/config/tests.rs` - P0-12: deleted the unit test
  `config_set_refuses_under_held_lock` that called `guard_run_lock` directly.
- `crates/ralphy-cli/src/run/wiring/tests.rs` - P0-13: renamed
  `check_agents_present_probes_cursor_by_agent_not_by_selector_name` ->
  `check_agents_present_uses_the_given_locator` (doc reworded); new
  `preflight_agents_probes_cursor_with_its_adapter_locator` (body slice of
  `pub(crate) fn preflight_agents(`).
- `crates/ralphy-cli/src/schedule/spec.rs` (test module only) - P0-14: deleted
  `timer_spec_run_with_triage_chains_triage_first`; new
  `triage_prelude_is_triage_yes_without_if_idle`; the `# ralphy-schedule:run:` tag assert
  moved into `timer_spec_run_names_task_and_args`.
- `crates/ralphy-cli/src/init/run.rs` (test module only) - P0-15: deleted
  `init_git_safety_branch_and_scaffold_end_to_end`; its scaffold asserts kept as
  `write_scaffold_writes_the_agent_docs_and_no_instruction_file` (no git calls). The decision
  functions keep their tests in `init/run/decisions/tests.rs`.
- `crates/ralphy-core/src/model_recovery.rs` (test module only) - K-3: `locked_merge_child` is
  `#[ignore = "..."]`; the parent's spawn args start with `"--ignored"`.
- `docs/plans/issue-485-followup-draft.md` - new: one section per seam (P0-3, P0-10, P0-11,
  P0-13, P0-14, P0-15, P0-8/P0-9), and an empty `## Also out of scope in #485` heading for
  Stage 12.
- `docs/audit-tests-2026-09-27.md` - §7 rows for P0-10 (deleted and follow-up), P0-11, P0-12,
  P0-13, P0-14, P0-15, K-3.

## Evidence
- P0-11: mutation `crates/ralphy-agent-copilot/src/lib.rs` `fn plan`, `self.phase_model(Phase::Plan)` -> `Phase::Execute` | before fix: PASS (the five argv tests, and all 93 other crate tests) | after fix: FAIL (`each_phase_reads_its_own_model_and_effort`) | reverted: PASS
- P0-12: mutation `crates/ralphy-cli/src/config.rs` `run()`, deleted `runlock::guard_run_lock(&ws, "config set", ..)?;` | before fix: PASS (unit `config::tests::config_set_refuses_under_held_lock`) | after fix: FAIL (`mutate::config_set_refuses_under_held_lock`) | reverted: PASS
- P0-12: mutation `crates/ralphy-cli/src/config.rs` `run()`, deleted `runlock::guard_run_lock(&ws, "config unset", ..)?;` | before fix: PASS (same unit test) | after fix: FAIL (`mutate::config_unset_refuses_under_held_lock`) | reverted: PASS
- P0-13: mutation `crates/ralphy-cli/src/run/wiring.rs` `preflight_agents`, `CliAgent::Cursor => ralphy_agent_cursor::locate_cursor().is_some()` -> `ralphy_adapter_support::locate_program(a.cli_name()).is_some()` | before fix: PASS (`check_agents_present_uses_the_given_locator` and the other three `check_agents_present_*`) | after fix: FAIL (`preflight_agents_probes_cursor_with_its_adapter_locator`) | reverted: PASS
- K-3: mutation `crates/ralphy-core/src/model_recovery.rs` `locked_merge_child`, `if true { return; }` as the first statement | before fix: the child always passed and cost a process | after fix: FAIL (`separate_process_transactions_are_serialized`, "timed out waiting for ...session-a.ready") | reverted: PASS; `cargo nextest run -p ralphy-core -E 'test(model_recovery)'` runs 6 tests and skips the ignored child.
- P0-10, P0-14, P0-15: deletions, category "tests a copy of production". No mutation: no test reaches that code today; the follow-up draft owns the seam.

## Gate results
`python docs/plans/issue-485-test-suite-cleanup-verify-gate.py --ids P0-10,P0-11,P0-12,P0-13,P0-14,P0-15,K-3`: 6/6 checks passed.
- section 7 rows for 7 IDs: PASS
- `cargo fmt --all --check`: PASS
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS
- `cargo nextest run --workspace`: PASS
- `cargo test --workspace --doc`: PASS
- `cargo run -q -p xtask -- changelog --check`: PASS

## Acceptance criteria audit
- [x] `--ids P0-10,P0-11,P0-12,P0-13,P0-14,P0-15,K-3` all have §7 rows.
- [x] `docs/plans/issue-485-followup-draft.md` exists.
- [x] Commit body has evidence lines for P0-11, P0-12, P0-13 and K-3.
- [x] No production mutation left (`git diff` shows only test modules, test files and docs).

## Deviations from plan
- P0-14: besides keeping `triage_prelude()`, the deleted test's `# ralphy-schedule:run:` cron
  tag assert moved into `timer_spec_run_names_task_and_args` (same file), because no other
  test checked the run target's tag.
- P0-15: the kept scaffold test drops the PRD-absence asserts;
  `init/scaffold.rs` `write_scaffold_prd_opt_in_controls_prd_docs` already owns them.
- P0-12: the two integration tests share one helper function; each test is one call site,
  because `set` and `unset` have separate guard calls.

## Surprises / notes
- `ralphy config` takes `--repo` before the subcommand (`ralphy config --repo <dir> set k v`);
  after it, clap refuses the flag.
- The K-3 mutation would also fail without `--ignored` (the filtered child would be skipped
  and never write its ready file). The clean PASS with the child listed as ignored is what
  shows `--ignored` runs it.
- Mutations were applied and reverted by exact string replacement (a script under the
  gitignored `.ralphy/`), never by git.
