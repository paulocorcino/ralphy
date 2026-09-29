pass-with-pending

# Reviewer gate 001 — issue-485-test-suite-cleanup

Diff range: `78628fe4..d6fb71df`, without the maintainer's own commits (d501049a, 1c55c481, f786a884,
54441616, e97da4b8). Level: deep. The re-review ran because the plan has a `critical` stage (Stage 2).

## Reviewer verdict

**pass-with-notes.** The P0 and LIES-FP fixes hold. The reviewer found no test that would still pass
with the behavior it names broken. The Stage 10 binary merges share environment state only through
`static Once` helpers with identical values, so no race. Findings:

1. medium — Stage 6 left the adapter budget setters (`with_max_minutes_per_issue`,
   `with_run_deadline`) with no test. A setter that does nothing passed the whole suite. The §7
   covering test (`budget.rs`) does not reach the adapter setter.
2. medium — S-4 (`crates/ralphy-proc-util/src/cursor.rs:248`): the replacement check, an exact
   folder listing, never landed. A gate that also writes `.cursorignore` passed.
3. medium — `docs/plans/issue-485-followup-draft.md:40,48-49` named three tests that Stage 9 merged.
4. low — `crates/ralphy-daemon/src/routes/presence.rs:16`: the `/ws` presence avatar had no assert
   after a Stage 7 deletion.
5. low — `crates/ralphy-agent-copilot/src/usage.rs:33`: `usage_from` had no test after a Stage 6
   deletion.
6. low — two stale comments: `crates/ralphy-daemon/tests/ws_peer/auth_ws.rs:6` and
   `crates/ralphy-cli/src/cli/tests.rs:364-366`.
7. low — §7 ledger: some rows named merged tests, two rows used a status outside the closed set,
   and the After column is the end-of-Stage-10 state, not HEAD.

Sampled and found clean: more than 45 covering-test names checked at HEAD across Stages 6, 7, 9 and
12; the table merges (`cmd_hazard` 14 rows, the guard tables 67 rows, the lock-refusal table 11
rows); every P0 fix; `production_text` against every mid-file `#[cfg(test)] mod`; the Stage 10
environment state; cross-platform (`nudge_never_waits` is `#[cfg(windows)]`); `changelog.d/485.md`;
the rest of the follow-up draft.

## Arbiter classification

| # | file:line | Severity | Defect? | Mechanical and in scope? | Class | Reason |
|---|---|---|---|---|---|---|
| 1 | `crates/ralphy-agent-*/src/lib.rs` setters | medium | yes | yes | must-fix | One test per adapter crate, test code only. |
| 2 | `crates/ralphy-proc-util/src/cursor.rs:248` | medium | yes | yes | must-fix | One assert in an existing test. |
| 3 | `docs/plans/issue-485-followup-draft.md:40` | medium | yes | yes | must-fix | Name changes in a draft this plan owns. |
| 4 | `crates/ralphy-daemon/tests/ws_peer/auth_ws.rs:78` | low | yes | yes | must-fix | One assert. |
| 5 | `crates/ralphy-agent-copilot/src/usage.rs:33` | low | yes | yes | must-fix | One unit test. |
| 6 | `auth_ws.rs:6`, `cli/tests.rs:364` | low | yes | yes | must-fix | Comment-only edits. |
| 7 | `docs/audit-tests-2026-09-27.md` §7 | low | yes | yes | must-fix | Ledger text only. |

## Fixes applied

All seven are in commit `d6fb71df` ("test: close the reviewer's coverage findings (#485)"). The full
gate passed after the round.

1. Each adapter crate has one test, `budget_setters_reach_the_issue_deadline`. Claude's is
   `exec_budget_setters_reach_the_issue_deadline`, because claude takes the minutes through
   `with_exec_config`. Each test fails when the setter does nothing (codex, copilot, cursor, gemini,
   kimi, opencode), or when `with_run_deadline` does nothing (claude).
2. `indexing_gate_creates_the_optout_in_a_repo_without_one` asserts the listing is exactly
   `[".cursorindexingignore", ".git"]`. It fails when the gate also writes `.cursorignore`.
3. The follow-up draft cites `headless_reason_maps_onto_a_core_outcome` and
   `headless_loop_decides_each_call_sequence`, and the new `tests/observe/`, `tests/ws_peer/`,
   `tests/fleet_session/` paths.
4. `auth_ws.rs` asserts `p.avatar`. It fails when `build_presence` sets `avatar: None`.
5. `usage_from_maps_each_store_count_to_its_own_field` fails when `cache_read` and
   `cache_creation` are swapped.
6. The `auth_ws.rs` line is deleted, and the `open-code` claim is removed from `cli/tests.rs`.
7. §7 has seven renamed covering tests, the two flake rows are `kept`, there is a HEAD-count
   sentence under the measurement table, the S-4 row is corrected, and there are four `review-*`
   rows.

## Re-review

**pass-with-notes**, with no high or medium findings. All seven fixes do what they claim. The new
findings below go to Pending. The plan allows one fix round only.

## Pending

- `crates/ralphy-agent-{codex,copilot,cursor,gemini}/src/tests.rs`, `crates/ralphy-agent-{kimi,opencode}/src/lib.rs`, `crates/ralphy-agent-claude/src/lib.rs:369` (the new budget tests, clamp half).
  - Reviewer: "`<=` would still pass if the deadline came back too early."
  - Arbiter: real but low. The finding asked to catch a setter that does nothing, and the test does.
  - Suggested action: change the clamp half to `assert_eq!(…, run_deadline)`; the arithmetic is exact.
- The `d6fb71df` commit body, evidence block.
  - Reviewer: "The clamp half was never seen red there; only claude's run made `with_run_deadline` do nothing."
  - Arbiter: an evidence gap, not a code defect. The reasoning shows the clamp half fails: the default cap is 0, so the result would be `now + 120 min`.
  - Suggested action: decide whether to record a red run for the clamp half.
- `crates/ralphy-cli/src/cli/tests.rs:364`.
  - Reviewer: "still opens with 'Guard the CLI-def move:'", which describes an earlier change.
  - Arbiter: a comment-rule nit.
  - Suggested action: start the comment at "Render the `run` subcommand's help…".
- Out of scope, noted by the re-review: `crates/ralphy-agent-cursor/src/guards.rs:61` has an older test with the same name as the S-4 test, and it does not have the listing check.
  - Suggested action: decide whether to add the same assert there.

## Pending resolution

- Clamp half: fixed in `e2648966`. All seven budget tests use `assert_eq!(…, run_deadline)`.
- Red run for the clamp half: recorded in the `e2648966` commit body. `with_run_deadline` was
  changed to store `None` in each of the seven adapters, and each test failed.
- `cli/tests.rs:364`: fixed in `e2648966`. The comment starts at "Render the `run` subcommand's help…".
- `crates/ralphy-agent-cursor/src/guards.rs:61`: closed with no change. `guards::indexing_gate` only
  calls `ralphy_proc_util::cursor::indexing_gate`. The proc-util test already fails when the gate
  also writes `.cursorignore`, so a second assert would fail under the same mutation and add nothing.
