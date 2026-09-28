# Stage 1 — Baseline measurements — Post-stage report

**Backlog items:** Phase 0 baseline
**Commit:** _filled by parent_
**Plan:** issue-485-test-suite-cleanup.md

## Files changed
- `docs/audit-tests-2026-09-27.md` — filled the **Before** column of the §7 measurement
  table (`cargo nextest run --workspace` median wall time, Rust test function count, Rust
  test binary count, `node --test crates/ralphy-daemon/ui-tests` pass count and duration),
  and added one sentence recording the host, date, and commit measured.

## Gate results
`python docs/plans/issue-485-test-suite-cleanup-verify-gate.py`: 5/5 checks passed
(`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo nextest run --workspace`, `cargo test --workspace --doc`,
`cargo run -q -p xtask -- changelog --check`).

## Acceptance criteria audit
- [x] `cargo nextest run --workspace --no-run` built once before timing runs.
- [x] `cargo nextest run --workspace` run 3 times; wall times recorded: 107.777 s, 108.221 s,
  102.288 s. Median: 107.8 s.
- [x] Rust test count via `cargo nextest list --workspace --message-format oneline | wc -l`:
  2921 (see Surprises below).
- [x] Rust test binary count via
  `cargo nextest list --workspace --list-type binaries-only --message-format oneline | wc -l`:
  101.
- [x] `node --test crates/ralphy-daemon/ui-tests`: 630 tests, 4.7086 s.
- [x] Before column of §7 filled with the four values, plus a sentence naming host, date,
  and commit measured (HEAD).
- [x] Gate script run and green.

## Deviations from plan
None. The order of operations in the stage block was followed exactly.

## Surprises / notes
- The Rust test function count was not stable across the session: the first
  `cargo nextest run --workspace` reported "2920 tests run", but the second and third runs,
  and a follow-up `cargo nextest list --workspace`, all agreed on 2921 — with no source
  change in between. No test file was touched during this stage, so the cause is outside
  this stage's scope; it is recorded in the report sentence so later stages are not
  surprised by the same drift. 2921 is the value written to the Before column because
  three independent measurements (runs 2, 3, and the standalone list) agree on it.
- HEAD moved during the session from `862900d1` (the commit named in the parent's Stage 0
  notes) to `d501049a` (`fix(ui): hide Pull while the branch has no upstream`) — a commit
  landed by something outside this stage between Stage 0 and Stage 1. The working tree was
  clean at the start of this stage regardless, so the working-tree policy (clean-required)
  was satisfied; measurements were taken and recorded against the HEAD actually present
  (`d501049a`), as instructed by "the commit measured (HEAD)".
