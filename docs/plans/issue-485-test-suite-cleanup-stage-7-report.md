# Stage 7 — Deletions in daemon, cli, core, small crates and JS — Post-stage report

**Backlog items:** §3.2–§3.9 DELETE rows; C-A1, C-A2, C-A3, C-A5, C-A8, C-A9, C-A10, C-A11, C-A13; C-B1, C-B2, C-B3, C-B4, C-B5, C-B11; K-1, K-2; S-4; J-2, J-3, J-7, J-8
**Commit:** _filled by parent in the End-to-end summary table_
**Plan:** issue-485-test-suite-cleanup.md

## Files changed

One commit per crate (Global convention "Stages 6 and 7 ... one commit per
crate"), gate run before each. The small crates (proc-util, pricing, release,
run-snapshot, usage-scan) each committed separately even though only a couple
of lines changed in most, to keep the per-crate audit trail; xtask carries the
accumulated §7 ledger rows for that whole group in its own commit (matching
Stage 6's pattern of touching the ledger doc only at grouped points, not every
commit).

- `crates/ralphy-daemon/` (commit `test(daemon): ...`): §3.2 `src/tests.rs`
  DELETE list (9 tests), §3.3 other-modules DELETE list across
  `autostart/tests.rs`, `dispatch/spawn/tests.rs`, `peer/nudge/tests.rs`,
  `usage/tests.rs`, `desk/tests.rs`, `cookie.rs`, `roster.rs`, `epoch.rs`,
  `release.rs`, §3.4 single-test deletions in `tests/observe_read.rs`,
  `tests/workspace_write.rs`, `tests/tree_watch.rs`.
- `crates/ralphy-cli/` (commit `test(cli): ...`): C-A1, C-A2, C-A3, C-A5, C-A8,
  C-A9, C-A10, C-A11, C-A13; C-B1, C-B2, C-B3, C-B4, C-B5, C-B11; the remaining
  §3.5/§3.6 DELETE rows (decoder tests duplicating `runstate/roundtrip.rs`,
  help-listing tests subsumed by `every_registry_key_is_handled_by_all_subcommands`,
  clap-registration-only tests, telegram serde round-trips, and other named
  duplicates).
- `crates/ralphy-core/` (commit `test(core): ...`): K-1 (deleted
  `tests/release_profile.rs`), K-2, and the remaining §3.7 DELETE rows
  (`stop.rs`'s process-global-leak-risk test, `types.rs`'s 3 trivial
  path-getter tests, 3 `serde_round_trip`s, a live-network e2e test, and
  `github/issues` `from_ghissue_maps_all_fields` after confirming `parse_issue`
  routes through `From<GhIssue>`). Also corrects the prior cli commit's §7
  entry for `init_state_path_is_under_gitignored_ralphy_dir` — see Surprises.
- `crates/ralphy-proc-util/` (commit `test(proc-util): ...`): S-4 and
  `indexing_gate_allows_with_the_optout_file`.
- `crates/ralphy-pricing/` (commit `test(pricing): ...`):
  `unknown_model_never_returns_some_zero` and the `rows.len() == 39` line in
  `floor.rs`.
- `crates/ralphy-release/` (commit `test(release): ...`):
  `the_default_endpoint_is_the_list_not_latest`.
- `crates/ralphy-run-snapshot/` (commit `test(run-snapshot): ...`):
  `snapshot_path_is_runid_keyed_under_runstate`'s production-expression-repeat
  assertion replaced with a literal-path pin (test-only strengthening).
- `crates/ralphy-usage-scan/` (commit `test(usage-scan): ...`):
  `missing_codex_dir_contributes_zero`.
- `crates/xtask/` (commit `test(xtask): ...`): `a_kind_is_never_silently_widened`
  (+ its now-unused import) and `the_history_round_trips_through_json` trimmed
  to `an_absent_topic_field_stays_absent_in_json`. Also carries the §7 ledger
  rows for S-4 and the whole §3.8 small-crates DELETE group.
- `crates/ralphy-daemon/ui-tests/` (commit `test(daemon-ui): ...`): J-2, J-3,
  J-7, and the remaining §3.9 DELETE rows across `app.test.mjs`,
  `wb-detach-link.test.mjs`, `wb-changes.test.mjs`, `wb-console.test.mjs`
  (the 4 `resumeDecision` tests duplicating `wb-daemon.test.mjs`). J-8 kept —
  see Deviations.
- `docs/audit-tests-2026-09-27.md` — §7 rows for every ID above.

## Gate results

Full gate (`python docs/plans/issue-485-test-suite-cleanup-verify-gate.py`,
`--ui` added for the JS commit) run twice: once before the first commit
(daemon) and once before the last commit (daemon-ui, `--ui --ids <all 22
declared IDs>`), both 5/5 and 8/8 respectively. Between those two runs, each
crate's targeted `cargo nextest run -p <crate>` and
`cargo clippy -p <crate> --all-targets -- -D warnings` were run and passed
before that crate's commit, plus `node --test crates/ralphy-daemon/ui-tests`
(626/626) and `cargo run -q -p xtask -- ui-copy --check` for the JS commit.

`cargo nextest list --workspace --message-format oneline | wc -l`: prior
(post-Stage-6) count was 2846; after this stage it should be lower by the
number of Rust tests deleted this stage (JS tests are not in this count).

## Acceptance criteria audit

- [x] Every deleted test's covering test (or category) named and confirmed to
      exist/run before deleting — see the §7 rows.
- [x] One commit per crate, gate before each (daemon, cli, core, proc-util,
      pricing, release, run-snapshot, usage-scan, xtask, daemon-ui = 10
      commits).
- [x] §7 rows written for every declared ID, verified by the gate script's
      `--ids` check (22/22 passed).
- [x] `git diff --stat`/`git status --short` per commit shows only the
      declared files (confirmed before each commit).
- [x] `crates/ralphy-core/tests/release_profile.rs` is absent (K-1).
- [ ] Not every DELETE row named in §3.2–§3.9 was deleted — several could not
      be identified with confidence after cumulative line drift and were left
      in place with the discrepancy recorded in §7 (see Deviations).

## Deviations from plan

- **Several line-numbered DELETE items could not be identified with
  confidence and were left in place**, each documented in §7 rather than
  guessed:
  - `crates/ralphy-cli/src/init/gate.rs:374`/`:458` — this file's own line
    numbers had already shifted twice within this stage (from the C-A8
    deletion), on top of Stage 5's edits. No candidate in the 420–470 range
    showed a clear duplicate.
  - `crates/ralphy-cli/src/events/config.rs:~200` — the report itself flagged
    this line number as approximate; the only candidate,
    `round_trips_slug_entry_and_env_override_wins`, is a legitimate
    multi-behavior test (round trip, env override precedence, `clear`
    semantics), not a plausible duplicate.
  - `crates/ralphy-cli/src/run/wiring/tests.rs:449` — `plan_agent_gemini_is_accepted`
    uniquely exercises clap's acceptance of `--plan-agent gemini`; no other
    test covers that.
  - `crates/ralphy-cli/src/ui/tests.rs:932` — `bar_label_no_colour_emits_no_ansi`
    is the only `bar()`-based test that also asserts no stray ANSI byte.
  - `crates/ralphy-daemon/ui-tests/wb-window-state.test.mjs:485` (J-8) — the
    file has shrunk to 158 lines (11 tests) since the report was written; no
    test sits anywhere near that offset, and the closest match by the report's
    description would be a net loss of coverage if deleted (see §7).
  - `crates/ralphy-daemon/ui-tests/app.test.mjs:24`/`:96` — the report grouped
    these with `:67` (`projectBadge`, deleted) under one "duplicate of
    wb-changes.test.mjs:465-497" note, but neither tests anything
    `wb-changes.test.mjs` covers.
- **Corrected a reasoning error from the cli commit, in the core commit.** The
  cli commit's §7 row for `init/wizard.rs`'s
  `init_state_path_is_under_gitignored_ralphy_dir` named
  `ralphy-core/src/types.rs`'s `init_state_path_is_under_ralphy_dir` as the
  covering test — but that core test is itself one of K's "3 path getters",
  deleted in the very next (core) commit of this same stage. The core commit's
  §7 entry corrects this: the right category for both deletions is "trivial
  getter" (a one-line `Path::join`), not cross-crate duplication. Caught before
  the stage finished, so no test coverage was actually lost by the mistake —
  only the stated reason was wrong for one commit.
- **`ralphy-run-snapshot/src/document.rs`'s deletion is a test-only
  strengthening, not a pure deletion.** The report's note ("repeats the
  production expression. Pin the literal path.") asked for more than removal;
  the redundant `assert_eq!(p, snapshot_dir(...).join(...))` line (which
  depended on the same production function under test) was replaced with a
  literal `Path::new("/repo/.ralphy/runstate/01ABC.json")` pin. No production
  code changed.
- **S-4's suggested follow-up not done.** The report additionally suggests
  "assert the exact folder listing in the 'creates' test instead" after
  deleting `no_cursorignore_in_proc_util`. That is a rule-1 (fix) action, not
  a rule-2 (delete) action, and out of this deletion-only stage's scope; only
  the deletion was done.
- **The small crates (proc-util, pricing, release, run-snapshot, usage-scan)
  each got their own commit** even where the diff was a single test or a
  single line, to preserve one-commit-per-crate; xtask's commit carries the
  accumulated §7 ledger text for the whole §3.8 group (mirroring how Stage 6's
  first and last commits were the only ones to touch the ledger doc).

## Surprises / notes

- **A concurrent maintainer edit to `crates/ralphy-daemon/ui-tests/wb-settings.test.mjs`
  appeared and then disappeared during this stage's work**, per the hand-off's
  warning that the maintainer works on the same branch in another session. It
  was never part of this stage's declared file list, was never staged or
  committed by this stage, and by the time of the final `--ui` gate run and
  commit, `git status`/`git diff` showed it clean (no pending change). Flagging
  per the hand-off instruction ("mention it in your return") — no action was
  taken on it.
- **§3.6's DELETE candidates in `main.rs`** (`copilot_one_shots_are_wired`,
  `cursor_one_shots_are_wired`, C-B5) leave copilot and cursor with no
  arm-scoped wiring test, while `gemini_one_shots_are_wired` (kept, not in
  scope) already has the stronger arm-scoped pattern described in that file's
  own comment. The report explicitly says "Delete" rather than "strengthen",
  so no replacement test was added — flagging for anyone doing a later
  strengthening pass.
- **`ralphy-usage-scan/src/codex.rs`'s `missing_codex_dir_contributes_zero`**
  was deleted on the "std-equivalent" category (an absent directory yields no
  entries through `fs::read_dir`'s own error path) rather than a named
  covering test, since no other test in the file asserts the same fact — the
  report listed it in the plain DELETE list with no further explanation.
