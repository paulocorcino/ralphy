# Stage 6 — Deletions in the adapters — Post-stage report

**Backlog items:** §3.1 DELETE rows, cross-adapter duplicates (§3.1 table rows 1–3 and `plan_pointer`), A-2, A-3, A-4, A-6
**Commit:** _filled by parent in the End-to-end summary table_
**Plan:** issue-485-test-suite-cleanup.md

## Files changed

Per-crate commits (one gate run before each, per Global conventions "Stages 6
and 7... one commit per crate"):

- `crates/ralphy-adapter-support/src/detect.rs`, `budget.rs` — deleted
  `detect_limit_maps_the_three_states`, `issue_budget_new_seeds_the_default_cap`.
- `crates/ralphy-agent-claude/src/lib.rs`, `plan.rs` — deleted
  `issue_deadline_zero_minutes_disables_the_cap` (+ its now-unused
  `agent_with_minutes` helper), `plan_pointer_is_a_pointer_not_the_charter`,
  `plan_prompts_carry_finalize_trailer`.
- `crates/ralphy-agent-codex/src/tests.rs`, `command.rs`, `outcome.rs`, `lib.rs`
  — deleted `codex_honours_max_minutes_per_issue`,
  `codex_zero_minutes_disables_the_per_issue_cap` (+ the now-unused
  `#[cfg(test)]`-only `issue_deadline()` oracle), `codex_agent_is_a_dyn_agent`,
  `plan_charter_file_carries_full_prompt` (A-2),
  `prompt_plan_codex_carries_finalize_trailer`,
  `xhigh_tier_effort_is_a_codex_accepted_word`,
  `effort_does_not_alter_the_tier_routed_model`, and 5 pass-through
  `classify_*` tests.
- `crates/ralphy-agent-kimi/src/lib.rs`, `outcome.rs` — deleted
  `kimi_agent_is_a_dyn_agent`, `resolved_effort_never_appears_on_argv` (A-4),
  `kimi_honours_max_minutes_per_issue`,
  `kimi_zero_minutes_disables_the_per_issue_cap` (+ the now-unused
  `issue_deadline()` oracle), `plan_charter_file_carries_full_prompt` (A-2),
  `prompt_plan_kimi_carries_finalize_trailer`, and 4 pass-through `classify_*`
  tests.
- `crates/ralphy-agent-opencode/src/lib.rs`, `command.rs`, `events.rs`,
  `outcome.rs`, `usage.rs` — deleted `opencode_honours_max_minutes_per_issue`,
  `opencode_zero_minutes_disables_the_per_issue_cap` (+ the now-unused
  `issue_deadline()` oracle), `opencode_agent_is_a_dyn_agent`,
  `adr_0005_d3_amendment_separates_variant_from_effort`,
  `resolved_effort_does_not_become_variant_on_argv` (A-4),
  `resolved_effort_never_becomes_variant` (A-4),
  `plan_charter_file_carries_full_prompt` (A-2),
  `prompt_plan_opencode_carries_finalize_trailer`,
  `resolved_model_label_returns_model_or_unknown`,
  `is_opencode_auth_error_takes_precedence_over_done_sentinel`, and 9
  pass-through `classify_*` tests.
- `crates/ralphy-agent-gemini/src/lib.rs`, `tests.rs`, `command/tests.rs`,
  `outcome/tests.rs`, `settings.rs`, `auth.rs` — deleted
  `gemini_agent_is_a_dyn_agent`, `mint_session_id_is_a_fresh_uuid`,
  `plan_charter_exceeds_argv_safe_size`,
  `the_roundtrip_fixture_carries_the_whole_charter`,
  `the_limit_stance_is_documented_as_the_one_most_likely_to_be_revised`,
  `the_two_phase_pins_round_trip`, `accepts_images_is_true`,
  `the_auth_message_reproduces_the_vendor_sentence`,
  `gemini_honours_max_minutes_per_issue` (+ the now-unused `issue_deadline()`
  oracle), `prompt_plan_gemini_carries_finalize_trailer`.
- `crates/ralphy-agent-cursor/src/lib.rs`, `tests.rs`, `command/tests.rs`,
  `outcome/tests.rs`, `settings.rs`, `usage.rs` — deleted
  `cursor_agent_is_a_dyn_agent`, `mint_session_id_is_a_fresh_uuid`,
  `plan_charter_exceeds_argv_safe_size`, `the_limit_stance_is_documented`,
  `cursor_settings_round_trips_json`, `the_credit_note_names_both_units`,
  `the_pinned_path_is_a_shape_the_vendor_classifier_accepts`,
  `cursor_honours_max_minutes_per_issue` (+ the now-unused `issue_deadline()`
  oracle), `prompt_plan_cursor_carries_finalize_trailer`.
- `crates/ralphy-agent-copilot/src/tests.rs`, `lib.rs`, `command.rs`,
  `settings.rs`, `usage.rs`, `effort.rs`, `outcome.rs`, `tasks.rs` — deleted
  `copilot_agent_is_a_dyn_agent`, `copilot_honours_max_minutes_per_issue`,
  `copilot_zero_minutes_disables_the_per_issue_cap` (+ the now-unused
  `issue_deadline()` oracle), `prompt_plan_copilot_carries_finalize_trailer`,
  `exec_charter_exceeds_argv_safe_size`, `mint_session_id_is_a_fresh_uuid`,
  `copilot_settings_defaults_are_all_none`, `copilot_settings_round_trips_json`,
  `copilot_usage_maps_session_rows_to_usage`,
  `copilot_usage_unknown_session_is_zero`,
  `copilot_usage_reads_no_premium_requests` (+ now-unused `seed_p2`/`usage_of`/
  `CREATE_USAGE` helpers), `every_effort_model_supports_low_medium_high` (+
  reworded the dangling doc-comment reference to it),
  `classify_done_ignores_zeroed_code_changes` (A-3),
  `preflight_or_bail_rejects_continue_on_auto_mode` (A-6), and 3 pass-through
  `classify_*` tests.
- `docs/audit-tests-2026-09-27.md` — §7 rows for every group above, plus G-1
  (follow-up).

## Gate results

Full gate (`python docs/plans/issue-485-test-suite-cleanup-verify-gate.py`)
run twice: once before the first commit (adapter-support) and once before the
last commit (copilot). Both passed 5/5 (fmt, clippy --all-targets -D warnings,
nextest --workspace, doctest, changelog --check). Between those two runs each
crate's targeted `cargo nextest run -p <crate>` and
`cargo clippy -p <crate> --all-targets -- -D warnings` were run and passed
before that crate's commit.

`cargo nextest list --workspace --message-format oneline | wc -l`: Stage 1
baseline was 2921; after this stage it is 2846 (down 75).

## Acceptance criteria audit

- [x] Every deleted test's covering test (or category) named and confirmed to
      exist/run before deleting.
- [x] Rule-2 red runs done for the three grouped mutations (budget.rs,
      classify.rs, the plan-prompt trailer) — see evidence lines below.
- [x] One commit per crate, gate before each.
- [x] §7 rows written, including one `rejected`-style disclosure (none needed;
      no finding rejected this stage) and one `follow-up` (G-1).
- [x] `cargo nextest list --workspace` count is lower than Stage 1's baseline.
- [x] `git diff --stat` per commit shows only the declared files (confirmed
      with `git status --short` before each commit).

**Evidence lines:**
- max_minutes row: mutation `budget.rs` `issue_deadline` forced to always use
  `unbounded` (dropping the `max_minutes_per_issue == 0` branch) | before fix:
  PASS (`budget.rs`'s own tests AND codex's
  `codex_honours_max_minutes_per_issue`/`codex_zero_minutes_disables_the_per_issue_cap`)
  | after fix: FAIL (both) | reverted: PASS.
- classify row: mutation `classify.rs` `classify()` dropped `&& !s.errored` |
  before fix: PASS (`ladder_matches_adr_0023_d2` AND opencode's
  `classify_stuck_on_error_event`) | after fix: `ladder_matches_adr_0023_d2`
  still PASSED (no case for `errored` — recorded as G-1), opencode's
  `classify_stuck_on_error_event` FAILED | reverted: PASS.
- prompt-trailer row: mutation deleted the `## Finalize` section (the trailer
  instruction) from `assets/prompts/plan/template.md` | before fix: PASS
  (`ralphy-core/tests/prompt_assembly.rs`'s
  `plan_prompt_artifacts_match_template_plus_overlays` AND claude's
  `plan_prompts_carry_finalize_trailer`) | after fix:
  `plan_prompt_artifacts_match_template_plus_overlays` FAILED (template/artifact
  drift detected), claude's own test still PASSED (it only pins the stored
  artifact file, untouched by the mutation) | reverted: PASS.
- A-2: no mutation — the report and the plan both say the covering test is
  Stage 12's G-2 (no scaffold-write test exists today). Recorded as such in §7.

## Deviations from plan

- The dyn_agent count in the report says "×7"; only 6 such tests exist
  (codex, copilot, cursor, gemini, kimi, opencode — claude has none). Deleted
  all 6 found; noted the discrepancy in the §7 row.
- The max_minutes cross-adapter row says "13 in 7 crates"; only 11 distinct
  test functions were found (claude and gemini/cursor fold both cases into one
  test each, rather than a separate `honours`/`zero_minutes` pair). Deleted
  all 11 found; noted the discrepancy in the §7 row.
- Several crates carried a `#[cfg(test)]`-only `issue_deadline()` "oracle"
  method (already test-only production code, per the file's own doc comment:
  "the deadline oracle the budget tests assert against"). Deleting the two
  `max_minutes` tests per crate left this method with zero callers, which
  clippy's `-D dead-code` caught. Deleted it too in claude, codex, kimi,
  opencode, gemini, cursor, copilot — it is `#[cfg(test)]`-gated test-only
  code, not a production change (falls under the plan's hard constraint: a
  test module/item inside a production file is test code).
- Copilot's `exec_charter_exceeds_argv_safe_size`,
  `every_effort_model_supports_low_medium_high`, and the three `usage.rs`
  tests each left a comment or helper referencing them; those comments were
  reworded (not deleted) to state the invariant directly instead of dangling
  on a deleted test name, per the Global convention "Comments state
  invariants, not history."
- Copilot's `usage.rs` test-only helpers `seed_p2`, `usage_of`, and the
  `CREATE_USAGE` constant became unused once all three tests that called them
  were deleted; removed them too (same "test code, not production" reasoning
  as the `issue_deadline()` oracles above).

## Surprises / notes

- The exact settings.rs line numbers cited in the report (gemini `:37`,
  cursor `:47`, copilot `:51`/`:64`) matched the *round-trip*/*defaults* tests
  precisely on today's tree, resolving what looked like an ambiguity between
  two candidate tests per crate (a `defaults`-only test and a
  `round_trip`/`defaults+round_trip` test). Kept gemini's
  `an_untouched_section_serializes_to_nothing` and cursor's
  `cursor_settings_defaults_are_false` (not named by the report); deleted the
  round-trip tests the report's line numbers actually pointed to.
- opencode's classify tests needed the most judgment: the report's
  "opencode 9" count only matches if 4 tests beyond the 5 obviously-named
  ones (`classify_timeout_upgrades_to_limit_when_seen`,
  `classify_timeout_stays_timeout_without_limit`,
  `classify_stuck_upgrades_to_limit_when_seen`,
  `classify_done_run_with_limit_event_resumes`) are also counted as
  pass-through duplicates of the shared ladder's limit-outranks-timeout/done
  precedence (already covered by `classify.rs`'s `ladder_matches_adr_0023_d2`
  cases (a)/(b)). Verified this reading arithmetically (5 + 4 = 9) before
  deleting, and kept only `classify_stuck_on_non_zero_exit` and
  `classify_stuck_on_error_event`, both of which exercise opencode-specific
  extraction rather than shared precedence.
- No finding needed a `rejected` status this stage — every named mutation
  behaved as the report predicted.
