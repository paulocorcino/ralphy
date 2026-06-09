# Plan for #17: Harden the interactive PTY execute() session lifecycle

## Feasible: yes
Both fixes are localized to `crates/ralphy-agent-claude/src/lib.rs` and the
acceptance criteria are directly test-verifiable (a unit test for the split DSR
sequence; the kill-before-propagate reorder is a small, mechanical change).

## Execution model: sonnet
Two small, well-understood, localized changes in one file — a rolling-tail scan
helper with a unit test and a 3-line reorder on the error path. No tricky
concurrency, lifetimes, or design ambiguity.

## Done when
- `cargo test --workspace` passes, including a new unit test proving a
  `CURSOR_POSITION_REQUEST` (`ESC[6n`) split across two consecutive chunks is
  detected by the pure scan helper (and that an unsplit/absent sequence behaves
  as expected).
- `cargo clippy --workspace --all-targets` is clean with no new warnings.
- Review-only: in `execute()`, the PTY child is killed even when
  `drive_session` returns an `Err` — verified by reading the reordered code in
  the PR (the result is captured, `session.kill()` runs, then the result is
  propagated).

## Decisions
- Decision: model the rolling tail as a pure free function
  `scan_dsr_request(carry: &mut Vec<u8>, chunk: &[u8]) -> bool` that appends the
  chunk to `carry`, searches the combined buffer with the existing
  `find_subslice`, then truncates `carry` to its last
  `CURSOR_POSITION_REQUEST.len() - 1` bytes (so a future split match is still
  possible but the just-matched bytes can't re-fire). Why: keeps the scan pure
  and unit-testable without a live PTY while owning the carry-over state in one
  place, matching the issue's "factor the scan into a pure helper" instruction.

## Steps
- [x] In `crates/ralphy-agent-claude/src/lib.rs`, add the pure helper
      `fn scan_dsr_request(carry: &mut Vec<u8>, chunk: &[u8]) -> bool` near
      `find_subslice` (line ~912): extend `carry` with `chunk`, compute
      `found = find_subslice(carry, CURSOR_POSITION_REQUEST).is_some()`, then
      drain `carry` down to its last `CURSOR_POSITION_REQUEST.len() - 1` bytes;
      return `found`.
- [x] In `drive_session` (line ~545), declare a `let mut dsr_carry: Vec<u8> =
      Vec::new();` before the poll `loop`, and in the `while let Ok(chunk) =
      rx.try_recv()` block (line ~564) replace the per-chunk
      `find_subslice(&chunk, CURSOR_POSITION_REQUEST).is_some()` check with
      `if scan_dsr_request(&mut dsr_carry, &chunk) { let _ =
      session.write_all(CURSOR_POSITION_REPLY); }` (keep the existing log tee
      untouched).
- [x] In `execute()` (line ~534), change
      `let outcome = self.drive_session(&mut session, &flag_file)?;` to capture
      the result without `?`: `let result = self.drive_session(&mut session,
      &flag_file);`, then `let _ = session.kill();`, then `result` (i.e.
      `result` is the function's return, killing first). Remove the now-duplicate
      trailing `Ok(outcome)`.
- [x] In the `#[cfg(test)] mod tests` block (line ~918), add a test
      `scan_dsr_request_detects_split_sequence` that feeds the 4-byte
      `CURSOR_POSITION_REQUEST` across two chunks (e.g. `b"\x1b["` then
      `b"6n"`), asserts the second call returns `true` and the first returns
      `false`; also assert an unsplit chunk containing the sequence returns
      `true` and a chunk with no sequence returns `false`. This FAILS before the
      helper exists / with the old per-chunk logic and PASSES after.
- [x] Self-review: no HIGH findings. Kill-before-propagate reorder is correct;
      `scan_dsr_request` drain logic is verified by the unit test.
- [x] `cargo fmt` && `cargo test --workspace` && `cargo clippy --workspace
      --all-targets` pass with no new warnings.
