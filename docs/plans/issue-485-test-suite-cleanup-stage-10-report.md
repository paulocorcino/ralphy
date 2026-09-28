# Stage 10 — Integration binaries and re-measure — Post-stage report

**Backlog items:** §3.4 binary consolidation, §3.6 integration binaries, §3.7 binaries, Phase 3 re-measure
**Commit:** _filled by parent_
**Plan:** issue-485-test-suite-cleanup.md

## Files changed
- `crates/ralphy-core/src/effort.rs` — new `#[cfg(test)] mod tests` with the two tests of the deleted `tests/effort.rs`, bodies unchanged.
- `crates/ralphy-core/tests/prompt_assembly.rs` — `tests/prompt_ledger.rs` appended unchanged (imports moved to the top, one module-doc paragraph). `tests/effort.rs`, `tests/prompt_ledger.rs` deleted.
- `crates/ralphy-cli/tests/verbs/` — `main.rs`, `support.rs` (shared git/`ralphy`/lock helpers and the `LockRow` type), `lock_refusal.rs` (the 11-row table), and the nine old files moved with `git mv` as modules. `worktree.rs`, `sync.rs`, `checkouts.rs`, `mutate.rs` each lost their lock-refusal tests and gained a `lock_rows()` builder; duplicated helpers removed from all modules.
- `crates/ralphy-daemon/tests/command_checkout_cwd.rs` — exact argv asserts for blob, branch, config and worktree legs, a new `board.list` leg. `command_{blob,branch,config,worktree,board,changes}.rs` deleted.
- `crates/ralphy-daemon/tests/command_mutate_git.rs` — `config.set` leg (test renamed); `command_config_mutate.rs` deleted.
- `crates/ralphy-daemon/tests/command_changes_mutate.rs` — `worktree.add` clean-exit leg (test renamed); `command_mutate_ok_message.rs` deleted.
- `crates/ralphy-daemon/tests/session_transport_free.rs` — the codec guard appended; `codec_transport_free.rs` deleted.
- `crates/ralphy-daemon/tests/sessions/` — `main.rs` with a `static Once` launcher override + 8 moved modules (their own `set_var` replaced by the helper call, stale "sole setter" comments removed).
- `crates/ralphy-daemon/tests/fleet_session/` — old `fleet_session.rs` as `main.rs` + `fleet_console.rs` module using the parent's `Once`-guarded `prepare_environment`.
- `crates/ralphy-daemon/tests/{observe,watch,ws_peer}/` — `main.rs` + moved env-free modules, no body changes.
- Doc comments in the touched files no longer point at deleted `command_config.rs`.
- `docs/audit-tests-2026-09-27.md` — §7 After column, measurement note, 13 ledger rows.

## Gate results
`python docs/plans/issue-485-test-suite-cleanup-verify-gate.py`: 5/5 passed (fmt, clippy `--all-targets -D warnings`, nextest 2488 passed, doctests, changelog check). The regrouped binaries were also run under `cargo test` (threads in one process): sessions 8, fleet_session 7, observe 44, watch 11, ws_peer 14, verbs 30 — all green.

## Acceptance criteria audit
- [x] Name list saved before (`docs/plans/logs/stage-10-names-before.txt`, 2506 tests, 100 binaries) and after (2488, 62).
- [x] Name diff (binary and module prefix ignored) shows only the intended changes: −11 cli lock tests +1 table; −6 daemon `command_*` argv tests; −2 daemon tests renamed into their legs (+2 new names) and −2 moved legs; the 2 effort tests moved from integration to lib.
- [x] Binary count lower: 100 → 62 (daemon integration 55 → 27, cli 9 → 1, core −2).
- [x] Measured as in Stage 1; After column filled.

Evidence (rules 2 and 3):
- `§3.4 command_* argv files`: mutation `dispatch/argv.rs` board_argv drop `--board` | before fix: n/a (file deletion) | covering test FAIL :255 | reverted: PASS. Same for branch/worktree/changes list `json`→`text` (FAIL :208/:240/:170), ConfigGet drop `--json` (FAIL :225), blob `--revision`→`--rev` (FAIL :187), `oneshot.rs` board field `board`→`boards` (FAIL :76).
- `§3.4 command_config_mutate`: mutation `argv.rs` ConfigSet drops `--` | after merge: FAIL `command_mutate_git.rs:104` | reverted: PASS.
- `§3.4 command_mutate_ok_message`: mutation `oneshot.rs:481` `if true || …` | after merge: FAIL `command_changes_mutate.rs:271` | reverted: PASS.
- `§3.6 lock-refusal ×11`: mutation `config.rs:85` drop `config unset` guard | table FAIL "config unset: must refuse" | reverted: PASS. Also `mutate.rs:217` (worktree add) and `sync.rs:106` → FAIL on their rows.

## Deviations from plan
- `command_run_params.rs` + `command_ws.rs` were NOT merged: they set `RALPHY_TEST_EXIT_CODE` to 0 and 7. The parent's rule (different env values stay separate) wins over the stage block. Daemon ends at 27 binaries, not 26.
- The codec guard went into the existing `session_transport_free.rs` (not a new file name), because `src/session.rs` cites that path.
- Five comments outside the declared file list now name an old path. The file names still exist under the new directory, but the path prefix is stale: `crates/ralphy-cli/src/bin/runlock_test_child.rs:1` (`tests/mutate.rs`), `crates/ralphy-core/src/worktree/tests.rs:294` (`crates/ralphy-cli/tests/worktree.rs`), `crates/ralphy-daemon/src/bin/session_test_child.rs:2` (`tests/session_ws.rs`), `crates/ralphy-daemon/src/fswrite/tests.rs:418` (`tests/workspace_write.rs`), `crates/ralphy-daemon/src/session/spec/tests.rs:186` (`tests/session_ws_cursor.rs`). Left alone (outside scope) for the parent to decide.
- `support::release` and the table's `Holder` log a failed kill/wait instead of the old bare `.ok()`.

## Surprises / notes
- nextest runs each test in its own process, so the merge saves link and `--list` time only; env sharing matters only under `cargo test`.
- Suite time 107.8 s → 98.9 s median (runs 99.2/98.7/98.9 s wall; nextest summary 97.3/96.8/96.9 s). The spread is under 1 %, so the host was not visibly shared. The gain mixes binary consolidation with test deletions from stages 6–9; they were not measured separately.
- Node suite: 577 tests, 5.0 s (Stage 1: 630, 4.7 s). Stage 10 did not touch JS; the count change is from stages 7 and 9.
