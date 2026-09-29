# Stage 12 — Gaps, flakes and closure — Post-stage report

**Backlog items:** G-1 to G-13; issue phase 7 flakes; closure (§7 complete, follow-up draft, changelog). Added by the parent: the six DELETE rows Stage 7 left, five comments that named a moved test path, and the `discardConfirm` note for the follow-up draft.
**Commit:** _filled by parent_
**Plan:** issue-485-test-suite-cleanup.md

## Files changed

Gaps (rule 5):
- `crates/ralphy-adapter-support/src/classify.rs` (test module): G-1, row (d') in `ladder_matches_adr_0023_d2`.
- `crates/ralphy-adapter-support/src/scaffold.rs` (test module): G-2, new `a_fresh_plan_call_writes_the_charter_and_drops_the_stale_plan_before_the_run`.
- `crates/ralphy-agent-codex/src/outcome.rs` (test module): G-3, new `limit_text_on_a_clean_exit_is_not_a_limit`.
- `crates/ralphy-usage-scan/src/{claude,codex,opencode}.rs` (test modules), `crates/ralphy-usage-scan/src/copilot/tests.rs`: G-4, an at-the-bound case in each `since` test.
- `crates/ralphy-usage-scan/src/kimi.rs` (test module): G-4, new `since_drops_an_older_session_and_keeps_one_ending_at_the_bound`.
- `crates/ralphy-release/src/fetch.rs` (test module): G-5, `a_status_that_is_not_retryable_is_not_retried` became the table `only_a_retryable_status_is_retried` (403 once, 429 twice).
- `crates/ralphy-daemon/ui-tests/wb-mode.test.mjs` (new) and `index.mjs` (imports it): G-8, `modeFor`/`isDemo`/`isDaemon`/`seedAllowed`.
- `crates/ralphy-daemon/ui-tests/wb-spend.test.mjs`: G-9, new test of `WBSpend.state` (the pane's view model).
- `crates/ralphy-daemon/ui-tests/app.test.mjs`: G-9, new test of `spendView()` (a document shows only for the open project).
- `crates/ralphy-cli/src/cli/tests.rs`: G-11, new `every_argv_the_daemon_spawns_parses` (argv from `ralphy_daemon::dispatch` builders, parsed by `Cli`); the hand list's comment now says it holds typed shapes.

Flakes (phase 7):
- `crates/ralphy-cli/src/events/emitter.rs` (test module): `emitter_serializes_daemon_id_only_when_present` builds `Emitter` as a struct literal.
- `crates/ralphy-daemon/src/peer/nudge/tests.rs`: `nudge_never_waits` uses `ping -n 8` and a 5 s control bound.

Parent extra 1, deletions (rule 2):
- `crates/ralphy-cli/src/init/gate.rs` (test module): `cursor_logged_in_maps_an_authenticated_status_to_true_and_anything_else_to_false` (audit `:374`), `copilot_logged_in_maps_a_catalog_to_true_and_an_error_to_false` (audit `:458`).
- `crates/ralphy-cli/src/events/config.rs` (test module): `slug_key_with_slash_round_trips_through_toml` (audit `:~200`).
- `crates/ralphy-cli/src/run/wiring/tests.rs`: `plan_agent_gemini_is_accepted` (audit `:449`).
- `crates/ralphy-cli/src/ui/tests.rs`: `bar_label_no_colour_emits_no_ansi` (audit `:932`).
- `crates/ralphy-daemon/ui-tests/app.test.mjs`: `"shell() builds the whole component off an empty document"` (audit `:24`), `"the shell-wide changes flash is a STRING, and the per-project errors are a MAP"` (audit `:96`).

Parent extra 2, comment paths (comment-only):
- `crates/ralphy-cli/src/bin/runlock_test_child.rs`: `tests/mutate.rs` → `tests/verbs/`.
- `crates/ralphy-core/src/worktree/tests.rs`: `crates/ralphy-cli/tests/worktree.rs` → `crates/ralphy-cli/tests/verbs/worktree.rs`.
- `crates/ralphy-daemon/src/bin/session_test_child.rs`: `tests/session_ws.rs` → `tests/sessions/session_ws.rs`.
- `crates/ralphy-daemon/src/fswrite/tests.rs`: `tests/workspace_write.rs` → `tests/observe/workspace_write.rs`.
- `crates/ralphy-daemon/src/session/spec/tests.rs`: `tests/session_ws_cursor.rs` → `tests/sessions/session_ws_cursor.rs`.

Closure:
- `docs/audit-tests-2026-09-27.md`: 21 §7 rows (G-1 to G-13, the six deletions in two rows plus three, the three flake rows, and the watch-sleep row).
- `docs/plans/issue-485-followup-draft.md`: `## Also out of scope in #485` filled: the issue's own out-of-scope list, the follow-up ledger rows (G-7, G-12, G-13, the emitter IP probe), the `discardConfirm` note, and the phase 6 re-audit list (report §5).
- `changelog.d/485.md`: new, `kind: internal`.

In every production `.rs` file above, all hunks are below the file's `#[cfg(test)]` line (checked per file). No test block went past 500 lines.

## Evidence

Rule 5 (gap tests). Each mutation was applied, the crate's whole suite (or the whole node suite) was run, and the mutation was reverted with the exact inverse edit. In every case only the named test failed.
- G-1: mutation `adapter-support/src/classify.rs` `classify`, `&& !s.errored` dropped | new row: FAIL (`ladder_matches_adr_0023_d2`, alone in the crate) | reverted: PASS
- G-2: mutation `adapter-support/src/scaffold.rs` `run_plan_session`, `let _ = fs::remove_file(cfg.plan_path);` deleted | FAIL (`a_fresh_plan_call_writes_the_charter…`) | reverted: PASS
- G-2: mutation same file, the `fs::write(cfg.plan_charter_path, …)` statement deleted | FAIL (`left: "old charter"`) | reverted: PASS
- G-3: mutation `agent-codex/src/outcome.rs` `classify_codex_outcome`, `if !exited_cleanly` → `if true` | FAIL (`limit_text_on_a_clean_exit_is_not_a_limit`, `Limit(Some(..))` for `Done`) | reverted: PASS
- G-4: mutation `usage-scan/src/{claude,codex,copilot,opencode,kimi}.rs`, `last >= since_dt` → `last > since_dt`, one file at a time | FAIL each time, one test only (`since_filters_interactive_by_last_ts`, `since_filters_by_last_ts`, `copilot_since_filters_by_last_ts`, `opencode_since_filters_by_last_ts`, `since_drops_an_older_session_and_keeps_one_ending_at_the_bound`) | reverted: PASS
- G-5: mutation `release/src/fetch.rs` `fetch_body`, `code == 429 ||` dropped | FAIL (`only_a_retryable_status_is_retried`, "429 is retried once", 1 for 2) | reverted: PASS
- G-8: mutation `assets/ui/wb-mode.js` `modeFor` always returns `"daemon"` | FAIL (`only a file: page is the demo…`, the only failure of 586) | reverted: PASS
- G-9: mutation `assets/ui/wb-spend.js` `state`, `floorNote: floorNote(doc, unpriced)` → `floorNote: ""` | FAIL (`the pane state: …`) | reverted: PASS
- G-9: mutation same function, the period key read from the control (`period || "all"`) | FAIL (same test) | reverted: PASS
- G-9: mutation `assets/ui/app.js` `spendView`, `doc: this.spend.slug === this.openSlug ? this.spend.doc : null` → `doc: this.spend.doc` | FAIL (`spendView shows the spend document only for the project that is open`) | reverted: PASS
- G-11: mutation `ralphy-daemon/src/dispatch/argv.rs` `blob_read_argv`, `"--revision"` → `"--rev"` | FAIL (`every_argv_the_daemon_spawns_parses`, the only failure in `ralphy-cli`'s 518) | reverted: PASS

Flakes (the test still fails against a mutation of the behavior it checks):
- emitter: mutation `cli/src/events/emitter.rs`, `#[serde(skip_serializing_if = "Option::is_none")]` on `daemon_id` deleted | FAIL (`emitter_serializes_daemon_id_only_when_present`) | reverted: PASS
- nudge: mutation `daemon/src/peer/nudge.rs` `spawn_detached`, waits on the child | FAIL after 7.1 s (spawn bound) | reverted: PASS in 7.1 s (was about 30 s)

Rule 2 (deletions):
- `init/gate.rs` ×2: category std/identity (`cursor_logged_in` returns its argument; `copilot_logged_in` is `Result::is_ok`).
- `events/config.rs` `slug_key_with_slash_round_trips_through_toml`: category `toml` behavior; the real save/load of slug `o/r` is `round_trips_slug_entry_and_env_override_wins`.
- `run/wiring/tests.rs` `plan_agent_gemini_is_accepted`: clap `value_enum`; mutation `resolve_plan_agent` ignores the plan agent | covering `plan_agent_defaults_to_the_executor_when_omitted`: FAIL | reverted: PASS
- `ui/tests.rs` `bar_label_no_colour_emits_no_ansi`: mutation `ui.rs` `queue_bar_label`, the head gets a `\u{1b}[1m` prefix | covering `queue_bar_label_advances_through_all_terminal_outcomes_to_n_over_n`: FAIL | reverted: PASS
- `app.test.mjs` `:24`: mutation `app.js` `shell()` reads the DOM first (`document.getElementById("x").id`) | the whole node run fails at the import of `app.test.mjs` | reverted: PASS
- `app.test.mjs` `:96`: category constant (initial values of the state literal). Mutation `changesError: ""` → `null` passes the whole suite after the deletion; no behavior reads the initial type. The duplicate-key collision the test was written for is owned by `"the state literal declares no key twice"`.

G-13 check: `grep` over `crates/ralphy-agent-*/src` finds the nine `emit::planning`/`emit::executing` call sites and no test that captures an emitted event. `runstate/capture/tests.rs` `adapter_emit_sites_pass_the_right_arguments` stays.

## Gate results

`python docs/plans/issue-485-test-suite-cleanup-verify-gate.py --ui --ids G-1,…,G-13`: 8/8 passed.
- §7 rows for 13 IDs: pass
- `cargo fmt --all --check`: pass
- `cargo clippy --workspace --all-targets -- -D warnings`: pass
- `cargo nextest run --workspace`: pass
- `cargo test --workspace --doc`: pass
- `cargo run -q -p xtask -- changelog --check`: pass
- `node --test crates/ralphy-daemon/ui-tests`: pass
- `cargo run -q -p xtask -- ui-copy --check`: pass

`python docs/plans/issue-485-test-suite-cleanup-verify-e2e.py --ledger-only`: 1/1 passed. `npx oxlint@1.85.0 --deny-warnings crates/ralphy-daemon/assets/ui crates/ralphy-daemon/ui-tests`: exit 0.

## Acceptance criteria audit

- [x] One test per behavior gap, each seen red under a mutation of the existing behavior (G-1 to G-5, G-8, G-9, G-11).
- [x] G-6, G-7, G-12, G-13 marked `follow-up` with the reason; G-10 points to D-3 (stage 8).
- [x] The two flake changes fail against a mutation; the rest of phase 7 is `kept (no flake evidence)`.
- [x] `verify-e2e.py --ledger-only` passes.
- [x] Follow-up draft filled; `changelog.d/485.md` exists and `xtask changelog --check` passes.
- [x] Gate with `--ui --ids G-1..G-13` passes; oxlint (1.85.0 through `npx`, the CI command) passes.

## Deviations from plan

- G-9: besides the `wb-spend.test.mjs` test, one `app.test.mjs` test for `spendView()` itself, since the audit's `spendView` is the `app.js` method and it has its own decision (the stale-slug check).
- G-11: the test is in `ralphy-cli` and calls the daemon's public `dispatch` builders; `ralphy-cli` already depends on `ralphy-daemon`. No production change.
- G-5: the new case became a row of the existing 403 test (rule 6), so the test name changed.
- The emitter tests `detect_yields_non_empty_core_fields` and `detect_reads_daemon_id_env` test `detect()` itself and still probe the network; a literal would test nothing. Kept, with the probe seam in the follow-up draft.
- The `:96` deletion has no covering test for the initial type (see Evidence); it is deleted as a constant.

## Surprises / notes

- Only `scaffold.rs` and `cli/tests.rs` were formatted, with `rustfmt` on the two files, not `cargo fmt --all`, so no other file on the shared branch was touched.
- The ledger now holds two rows for G-1 (stage 6 `follow-up`, stage 12 `fixed`) and three for G-8/G-9 across stages; the latest row is the status.
- Every mutation in this stage was applied and reverted by a script that fails if the mutated text changed while the test ran; `git diff` after the last run shows no production hunk outside a test module.
